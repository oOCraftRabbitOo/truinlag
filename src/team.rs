use std::{collections::HashMap, rc::Rc};

use crate::{runtime::RuntimeRequest, InternEngineCommand, SessionContext, TimerHook};

use super::{
    challenge::{ChallengeEntry, InOpenChallenge},
    Config, DBEntry,
};
use chrono::{self, Duration as Dur};
use geo::Distance;
use rand::{prelude::*, rng};
use serde::{Deserialize, Serialize};
use truinlag::{commands::Error, *};

/// The representation of a team in a running or future game in the db
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamEntry {
    pub name: String,
    #[serde(default)]
    pub picture: Option<u64>,
    pub players: Vec<u64>,
    #[serde(default)]
    pub player_location_counts: HashMap<u64, (u32, u32)>,
    pub discord_channel: Option<u64>,
    pub role: TeamRole,
    pub colour: Colour,
    pub points: u64,
    pub bounty: u64,
    #[serde(default)]
    pub current_zone_id: u64,
    pub current_location: Option<DetailedLocation>,
    pub locations: Vec<MinimalLocation>,
    #[serde(default)]
    pub location_sending_player: Option<u64>,
    pub challenges: Vec<InOpenChallenge>,
    pub periods: Vec<Period>,
    #[serde(default)]
    pub grace_period_end: Option<TimerHook>,
}

/// The representation of a thing that a team did in a running game in the db
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Period {
    #[serde(default)]
    pub pictures: Vec<u64>,
    pub context: PeriodContext,
    pub location_start_index: usize,
    pub location_end_index: usize,
    pub end_time: chrono::NaiveTime,
}

/// Context for what a team did in a period
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeriodContext {
    Trophy {
        trophies: u64,     // the number of trophies bought at the end of the period
        points_spent: u64, // the amount of points spent on the trophies
    },
    Caught {
        catcher_team: usize, // the id of the team that was the catcher
        bounty: u64,         // the bounty handed over to said team
    },
    Catcher {
        caught_team: usize, // the id of the team that was caught
        bounty: u64,        // the bounty collected from said team
    },
    CompletedChallenge {
        title: String,       // the title of the completed challenge
        description: String, // the description of the completed challenge
        zone: Option<u64>,   // the zone of the completed challenge
        points: u64,         // the points earned by completing the challenge
        id: u64,
    },
}

// this is implemented to be able to sort periods by end time
impl PartialEq for Period {
    fn eq(&self, other: &Self) -> bool {
        self.end_time.eq(&other.end_time)
    }
}

// this is implemented to be able to sort periods by end time
impl Eq for Period {}

// this is implemented to be able to sort periods by end time
impl PartialOrd for Period {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// this is implemented to be able to sort periods by end time
impl Ord for Period {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.end_time.cmp(&other.end_time)
    }
}

impl TeamEntry {
    pub fn new(
        name: String,
        players: Vec<u64>,
        discord_channel: Option<u64>,
        colour: Colour,
        config: Config,
    ) -> Self {
        TeamEntry {
            name,
            picture: None,
            players,
            player_location_counts: HashMap::new(),
            discord_channel,
            colour,
            challenges: Vec::with_capacity(config.num_challenges as usize),
            role: TeamRole::Runner,
            points: 0,
            bounty: 0,
            current_location: None,
            locations: Vec::new(),
            location_sending_player: None,
            periods: Vec::new(),
            current_zone_id: config.start_zone,
            grace_period_end: None,
        }
    }

    /// Converts the engine-internal `TeamEntry` type into a sendable truinlag `Team` type
    pub fn to_sendable(&self, index: usize, context: &SessionContext) -> truinlag::Team {
        truinlag::Team {
            colour: self.colour,
            role: self.role,
            name: self.name.clone(),
            picture_id: self.picture,
            id: index,
            bounty: self.bounty,
            points: self.points,
            players: self
                .players
                .iter()
                .map(|p| {
                    context
                        .engine_context
                        .player_db
                        .get(*p)
                        .expect("PlayerEntry not found in db while making team sendable")
                        .contents
                        .to_sendable(*p)
                })
                .collect(),
            challenges: self.challenges.iter().map(|c| c.to_sendable()).collect(),
            completed_challenges: self
                .periods
                .iter()
                .filter_map(|p| match &p.context {
                    PeriodContext::CompletedChallenge {
                        title,
                        description,
                        points,
                        id,
                        zone: _,
                    } => Some(CompletedChallenge {
                        title: title.clone(),
                        description: description.clone(),
                        points: *points,
                        time: p.end_time,
                        picture_ids: p.pictures.clone(),
                        id: *id,
                    }),
                    _ => None,
                })
                .collect(),
            location: self.current_location.clone(),
            in_grace_period: match &self.grace_period_end {
                None => false,
                Some(timer) => chrono::Local::now() <= timer.end_time,
            },
            period_id: self.period_id(),
        }
    }

    pub fn period_id(&self) -> usize {
        self.periods.len()
    }

    pub fn add_location(
        &mut self,
        location: DetailedLocation,
        by_player: u64,
    ) -> Option<DetailedLocation> {
        let (total_count, accepted_count) = self
            .player_location_counts
            .get(&by_player)
            .cloned()
            .unwrap_or((0, 0));
        self.player_location_counts
            .insert(by_player, (total_count + 1, accepted_count));
        match self.current_location.clone() {
            None => Some(self.definitely_add_location(location, by_player)),
            Some(old_location) => {
                //// Was not very effective in practice
                // match self.location_sending_player {
                //     None => {
                //         self.location_sending_player = Some(by_player);
                //         return Some(self.definitely_add_location(location, by_player));
                //     }
                //     Some(player) => {
                //         if by_player == player {
                //             return Some(self.definitely_add_location(location, by_player));
                //         }
                //     }
                // }

                let time_since_last_location = location.timestamp - old_location.timestamp;

                if location.accuracy < 20
                    || (location.accuracy as f32 / old_location.accuracy as f32) < 1.0
                    || (time_since_last_location > 15
                        && (location.accuracy as f32 / old_location.accuracy as f32) < 1.5)
                    || time_since_last_location > 30
                {
                    self.location_sending_player = Some(by_player);
                    self.player_location_counts
                        .insert(by_player, (total_count + 1, accepted_count + 1));
                    return Some(self.definitely_add_location(location, by_player));
                }

                None
            }
        }
    }

    fn definitely_add_location(
        &mut self,
        location: DetailedLocation,
        by_player: u64,
    ) -> DetailedLocation {
        self.current_location = Some(location.clone());
        self.location_sending_player = Some(by_player);
        match self.locations.last() {
            None => self.locations.push(location.clone().into()),
            Some(old_loc) => {
                let new_loc = MinimalLocation::from(location.clone());
                if geo::Geodesic.distance(old_loc.as_point(), new_loc.as_point()) > 20.0
                    && new_loc.timestamp - old_loc.timestamp > 10
                {
                    self.locations.push(new_loc);
                }
            }
        }
        location
    }

    /// Returns the team's current location as a geo `Point`.
    pub fn location_as_point(&self) -> Option<geo::Point<f64>> {
        self.current_location
            .clone()
            .map(|l| geo::Point::from((l.latitude as f64, l.longitude as f64)))
    }

    /// Returns the team's current location
    pub fn location(&self) -> Option<(f32, f32)> {
        self.current_location
            .clone()
            .map(|l| (l.latitude, l.longitude))
    }

    /// Resets the team and has it start out as a gatherer team
    ///
    /// # Errors
    ///
    /// The team's zone has to be the starting zone. If that can't be found, a `NotFound` error is
    /// returned.
    pub fn start_runner(&mut self, context: &SessionContext) -> Result<(), commands::Error> {
        self.reset(context)?;
        self.generate_challenges(context);
        Ok(())
    }

    /// Resets the team and has it start out as a hunter team
    ///
    /// # Errors
    ///
    /// The team's zone has to be the starting zone. If that can't be found, a `NotFound` error is
    /// returned.
    pub fn start_catcher(&mut self, context: &SessionContext) -> Result<(), commands::Error> {
        self.reset(context)?;
        self.role = TeamRole::Catcher;
        self.points = context.config.bounty_start_points;
        Ok(())
    }

    /// Resets the team's points, bounty and other in game attributes.
    ///
    /// # Errors
    ///
    /// The team's zone has to be the starting zone. If that can't be found, a `NotFound` error is
    /// returned.
    pub fn reset(&mut self, context: &SessionContext) -> Result<(), commands::Error> {
        self.challenges = Vec::new();
        self.periods = Vec::new();
        self.locations = Vec::new();
        self.role = TeamRole::Runner;
        self.points = 0;
        self.bounty = 0;
        self.current_zone_id = context
            .engine_context
            .zone_db
            .find(|z| z.zone == context.config.start_zone)
            .ok_or(Error::NotFound("config start zone".into()))?
            .id;
        Ok(())
    }

    /// Generates new challenges for the team
    pub fn generate_challenges(&mut self, context: &SessionContext) {
        // Challenge selection is the perhaps most complicated part of the game, so I think it
        // warrants some good commenting. The process for challenge generation differs mainly based
        // on the generation period. There are (currently) 5 of them. In this function, the current
        // period is obtained in the form of an enum from a top level function and a match
        // expression is used to perform different actions depending on the period. Note that the
        // challenge generation process is completely different from period to period, so the match
        // expression actually makes up almost the entire function body.

        type Filter = Rc<dyn Fn(&DBEntry<'_, ChallengeEntry>) -> bool>;
        type GenInfo = (Vec<Filter>, GC);

        /// Generation Context
        ///
        /// is used locally to conveniently represent some info about how the challenge should be
        /// generated.
        struct GC {
            pub context: &'static str,
            pub zone_zoneables: bool,
            pub allowed_zones: Option<Vec<u64>>,
        }

        impl GC {
            pub fn new(context: &'static str) -> Self {
                GC {
                    context,
                    zone_zoneables: true,
                    allowed_zones: None,
                }
            }

            pub fn allow_zones(mut self, allowed_zones: Vec<u64>) -> Self {
                self.allowed_zones = Some(allowed_zones);
                self
            }

            pub fn dont_zone(mut self) -> Self {
                self.zone_zoneables = false;
                self
            }
        }

        let zone_entries = context.engine_context.zone_db;
        let config = &context.config;
        let raw_challenges = context.engine_context.challenge_db.get_all();
        let team_zone = zone_entries
            .get(self.current_zone_id)
            .expect("team is in a zone that is not in db");
        let points_to_top = context
            .top_team_points
            .map(|p| p.saturating_sub(self.points));

        let challenge_sets = config.challenge_sets.clone();
        let challenge_set_filter: Filter = Rc::new(move |c| -> bool {
            challenge_sets.iter().any(|s| c.contents.sets.contains(s))
        });

        let approved_status_filter: Filter = Rc::new(|c| -> bool {
            matches!(
                c.contents.status,
                ChallengeStatus::Approved | ChallengeStatus::Refactor
            )
        });

        let periods = self.periods.clone();

        let not_completed_filter: Filter = Rc::new(move |c| -> bool {
            periods
                .iter()
                .filter_map(|p| match &p.context {
                    PeriodContext::CompletedChallenge {
                        title: _,
                        description: _,
                        zone: _,
                        points: _,
                        id,
                    } => Some(id),
                    _ => None,
                })
                .all(|i| *i != c.id)
        });

        let specific_filter: Filter = Rc::new(|c| -> bool {
            matches!(
                c.contents.kind,
                ChallengeType::Kaff | ChallengeType::Ortsspezifisch | ChallengeType::Zoneable
            )
        });

        let unspecific_filter: Filter = Rc::new(|c| -> bool {
            matches!(
                c.contents.kind,
                ChallengeType::Zoneable | ChallengeType::Unspezifisch
            )
        });

        let zkaff_filter: Filter =
            Rc::new(|c| -> bool { matches!(c.contents.kind, ChallengeType::ZKaff) });

        let perim_max_kaff = config.perim_max_kaff;
        let max_kaff_filter: Filter = Rc::new(move |c| -> bool {
            c.contents.kaffskala.is_none()
                || matches!(
                c.contents.kaffskala,
                Some(x) if x as u64 <= perim_max_kaff)
        });

        let regio_filter: Filter =
            Rc::new(|c| -> bool { matches!(c.contents.kind, ChallengeType::Regionsspezifisch) });

        let team_zone_2 = team_zone.clone_contents();
        let dist_range = config.normal_period_far_distance_range.clone();
        let far_filter: Filter = Rc::new(move |c| -> bool {
            dist_range.contains(&c.contents.distance(&team_zone_2.as_borrowed()))
        });

        let team_zone_2 = team_zone.clone_contents();
        let dist_range = config.normal_period_near_distance_range.clone();
        let near_filter: Filter = Rc::new(move |c| -> bool {
            dist_range.contains(&c.contents.distance(&team_zone_2.as_borrowed()))
        });

        let centre_zone = match zone_entries.find(|z| z.zone == config.centre_zone) {
            Some(centre_zone) => centre_zone,
            None => {
                eprintln!(
                    "Engine: EXTREMELY BAD ERROR: \
                    couldn't find centre zone during challenge selection, \
                    giving up"
                );
                return;
            }
        };

        let select_challenge =
            |filters: Vec<Filter>, already_in_filter: Filter, gc: GC| -> Option<InOpenChallenge> {
                let combined_filter: Filter = Rc::new(move |c| filters.iter().all(|f| f(c)));
                let filter_list_list: Vec<Vec<Filter>> = vec![
                    vec![
                        already_in_filter.clone(),
                        approved_status_filter.clone(),
                        challenge_set_filter.clone(),
                        not_completed_filter.clone(),
                        combined_filter.clone(),
                    ],
                    vec![
                        approved_status_filter.clone(),
                        combined_filter.clone(),
                        already_in_filter.clone(),
                    ],
                    vec![approved_status_filter.clone()],
                    vec![Rc::new(|_| true)],
                ];
                filter_list_list.into_iter().enumerate().fold(
                    None,
                    |prev: Option<InOpenChallenge>, (index, filter_list)| match prev {
                        Some(c) => Some(c),
                        None => {
                            if index >= 1 {
                                eprintln!(
                                    "Engine: Failed to select {} \
                                        challenge for team {}, \
                                        trying with filter list {} instead",
                                    gc.context, self.name, index
                                )
                            }
                            raw_challenges
                                .iter()
                                .filter(|&c: &_| filter_list.iter().all(|f| f(c)))
                                .collect::<Vec<_>>()
                                .choose(&mut rng())
                                .map(|c| {
                                    c.contents.challenge(
                                        gc.zone_zoneables,
                                        centre_zone.clone(),
                                        team_zone.clone(),
                                        gc.allowed_zones.clone(),
                                        c.id,
                                        points_to_top,
                                        context,
                                    )
                                })
                        }
                    },
                )
            };

        let gen_infos: Vec<GenInfo> = match get_period(config) {
            GenerationPeriod::Specific => {
                // The specific period challenge generation is used at the very start of the game
                // and it is the most simple. Three challenges are generated and they must all be
                // specific.
                (0..config.num_challenges)
                    .map(|_| (vec![specific_filter.clone()], GC::new("specific")))
                    .collect()
            }
            GenerationPeriod::Normal => {
                // Normal period challenge generation generates three different kinds of
                // challenges.
                // 1. A specific challenge that is close by
                // 2. either a specific challenge that is far away or a region-specific challenge
                // 3. an unspecific challenge.
                //
                // This system is designed for exactly 3 challenges, but for completeness, there
                // should be a fallback for different challenge amounts. If more or fewer than 3
                // challenges are requested, all challenges will be generated the same way as
                // during the specific period save one which will be unspecific.

                if config.num_challenges == 3 {
                    let is_regio = rng().random_bool(match config.regio_ratio {
                        ..=0.0 => 0.0,
                        1.0.. => 1.0,
                        other => other,
                    });
                    vec![
                        if is_regio {
                            (vec![regio_filter], GC::new("regionspecific").dont_zone())
                        } else {
                            (
                                vec![specific_filter.clone(), near_filter],
                                GC::new("normal near").allow_zones(
                                    team_zone.contents.zones_with_distance(
                                        config.normal_period_near_distance_range.clone(),
                                    ),
                                ),
                            )
                        },
                        (
                            vec![specific_filter.clone(), far_filter],
                            GC::new("normal far").allow_zones(
                                team_zone.contents.zones_with_distance(
                                    config.normal_period_far_distance_range.clone(),
                                ),
                            ),
                        ),
                        (vec![unspecific_filter], GC::new("unspecific").dont_zone()),
                    ]
                } else {
                    (1..config.num_challenges)
                        .map(|_| (vec![specific_filter.clone()], GC::new("specific")))
                        .chain(vec![(
                            vec![unspecific_filter],
                            GC::new("unspecific").dont_zone(),
                        )])
                        .collect()
                }
            }
            GenerationPeriod::Perimeter(ratio) => {
                // Perimeter period challenge generation generates three different kinds of challenges
                // based on perim distance. The current perim distance changes fluidly with time passing.
                // It lerps from the maximum distance to the minimum distance as defined by the config
                // using the ratio. The perim distance describes the distance from zone 110 in minutes.
                // 1. A far challenge, i.e. a challenge with perim distance in the upper half of the
                //    current perim distance. It is specific and may not be zoneable, but it may be region-specific.
                // 2. A near challenge, i.e. a challenge with perim distance in the lower half of the
                //    current perim distance. It is specific, may be zoneable, but not region-specific.
                // 3. An unspecific challenge, i.e... Well it's just an unspecific challenge.
                //
                // Also, all of these challenges have a maximum kaffness value which is constant because I
                // can't be bothered.
                //
                // This system is designed for exactly 3 challenges, but for completeness, there
                // should be a fallback for different challenge amounts. If more or fewer than 3
                // challenges are requested, all challenges will be generated the same way as
                // during the specific period save one which will be unspecific.

                if config.num_challenges == 3 {
                    let max_perim = lerp(
                        config.perim_distance_range.end,
                        config.perim_distance_range.start,
                        ratio,
                    );
                    let perim_near_zones =
                        centre_zone.contents.zones_with_distance(0..max_perim / 2);
                    let perim_near_zones_2 = perim_near_zones.clone();
                    let perim_near_filter: Filter = Rc::new(move |c| -> bool {
                        c.contents
                            .zone
                            .iter()
                            .any(|z| perim_near_zones_2.contains(z))
                    });
                    let perim_far_zones = centre_zone
                        .contents
                        .zones_with_distance(max_perim / 2..max_perim);
                    let perim_far_zones_2 = perim_far_zones.clone();
                    let perim_far_filter: Filter = Rc::new(move |c| -> bool {
                        c.contents
                            .zone
                            .iter()
                            .any(|z| perim_far_zones_2.contains(z))
                    });
                    vec![
                        (
                            vec![
                                specific_filter.clone(),
                                perim_far_filter,
                                max_kaff_filter.clone(),
                            ],
                            GC::new("perim far").allow_zones(perim_far_zones),
                        ),
                        (
                            vec![perim_near_filter, specific_filter, max_kaff_filter],
                            GC::new("perim near").allow_zones(perim_near_zones),
                        ),
                        (vec![unspecific_filter], GC::new("unspecific").dont_zone()),
                    ]
                } else {
                    (1..config.num_challenges)
                        .map(|_| (vec![specific_filter.clone()], GC::new("specific")))
                        .chain(vec![(
                            vec![unspecific_filter],
                            GC::new("unspecific").dont_zone(),
                        )])
                        .collect()
                }
            }
            GenerationPeriod::ZKaff(ratio) => {
                // ZKaff challenge generation works the same as perimeter generation, except that ratio of
                // the generated specific challenges will be ZKaff challenges. For increased consistency,
                // the probability is applied sequentially, first to the far challenge, then to the near
                // one. So if ratio = 0.5, the far challenge will always be a ZKaff challenge and the near
                // one never. If ratio = 0.75, the far challenge will always ZKaff and the near one has a
                // 50/50 chance.
                // Also, if there are not 3 challenges, we just generate all ZKaff save one unspecific.

                if config.num_challenges == 3 {
                    // generating perim challenges with max max_perim
                    let max_perim = config.perim_distance_range.start;
                    let perim_near_zones =
                        centre_zone.contents.zones_with_distance(0..max_perim / 2);
                    let perim_near_zones_2 = perim_near_zones.clone();
                    let perim_near_filter: Filter = Rc::new(move |c| -> bool {
                        c.contents
                            .zone
                            .iter()
                            .any(|z| perim_near_zones_2.contains(z))
                    });
                    let perim_far_zones = centre_zone
                        .contents
                        .zones_with_distance(max_perim / 2..max_perim);
                    let perim_far_zones_2 = perim_far_zones.clone();
                    let perim_far_filter: Filter = Rc::new(move |c| -> bool {
                        c.contents
                            .zone
                            .iter()
                            .any(|z| perim_far_zones_2.contains(z))
                    });
                    let (first, second) = if ratio < 0.0 {
                        (false, false)
                    } else if ratio < 0.5 {
                        (rng().random_bool(ratio * 2.0), false)
                    } else if ratio < 1.0 {
                        (true, rng().random_bool((ratio - 0.5) * 2.0))
                    } else {
                        (true, true)
                    };
                    vec![
                        if first {
                            (vec![zkaff_filter.clone()], GC::new("zkaff"))
                        } else {
                            (
                                vec![
                                    specific_filter.clone(),
                                    perim_far_filter,
                                    max_kaff_filter.clone(),
                                ],
                                GC::new("perim far").allow_zones(perim_far_zones),
                            )
                        },
                        if second {
                            (vec![zkaff_filter], GC::new("zkaff"))
                        } else {
                            (
                                vec![specific_filter, perim_near_filter, max_kaff_filter],
                                GC::new("perim near").allow_zones(perim_near_zones),
                            )
                        },
                        (vec![unspecific_filter], GC::new("unspecific").dont_zone()),
                    ]
                } else {
                    (1..config.num_challenges)
                        .map(|_| (vec![zkaff_filter.clone()], GC::new("zkaff")))
                        .chain(vec![(
                            vec![unspecific_filter],
                            GC::new("unspecific").dont_zone(),
                        )])
                        .collect()
                }
            }
            GenerationPeriod::EndGame => {
                // End game challenge generation is relatively simple. 1 Challenge is ZKaff, the others are
                // unspecific. No further restrictions apply.

                (1..config.num_challenges)
                    .map(|_| {
                        (
                            vec![unspecific_filter.clone()],
                            GC::new("unspecific").dont_zone(),
                        )
                    })
                    .chain(vec![(vec![zkaff_filter], GC::new("zkaff"))])
                    .collect()
            }
        };

        let mut challenges = Vec::new();
        for (filters, gc) in gen_infos {
            let already_ids: Vec<u64> = challenges.iter().map(|c: &InOpenChallenge| c.id).collect();
            let not_generated_filter: Filter = Rc::new(move |c| !already_ids.contains(&c.id));
            challenges.push(match select_challenge(filters, not_generated_filter, gc) {
                Some(c) => c,
                None => {
                    eprintln!(
                        "Engine: EXTREMELY BAD ERROR: couldn't generate any challenges for team {}, giving up.",
                        self.name
                    );
                    return;
                }
            });
        }
        challenges.shuffle(&mut rng());
        self.challenges = challenges;
    }

    /// Adds a new period with the provided context and pictures to the team with the end time
    /// being now.
    fn new_period(&mut self, context: PeriodContext) {
        if let Some(current_location) = &self.current_location {
            self.locations.push(current_location.clone().into());
        }
        if self.locations.is_empty() {
            self.locations.push(MinimalLocation {
                latitude: 0.0,
                longitude: 0.0,
                timestamp: 0,
            });
        }
        self.periods.push(Period {
            context,
            pictures: Vec::new(),
            location_start_index: self
                .periods
                .last()
                .map(|p| p.location_end_index + 1)
                .unwrap_or(0),
            location_end_index: self.locations.len() - 1,
            end_time: chrono::Local::now().time(),
        });
    }

    /// Sets all the values inside that team that need to be set when it gets caught
    pub fn be_caught(&mut self, catcher_id: usize) {
        self.grace_period_end = None;
        self.challenges.clear();
        self.role = TeamRole::Catcher;
        self.new_period(PeriodContext::Caught {
            catcher_team: catcher_id,
            bounty: self.bounty,
        });
        self.bounty = 0;
    }

    /// Sets all the values inside that team that need to be set when it has caught another team.
    /// Returns a `RuntimeRequest` that starts the grace period timer.
    pub fn have_caught(
        &mut self,
        bounty: u64,
        caught_id: usize,
        catcher_id: usize,
        context: &mut SessionContext,
    ) -> RuntimeRequest {
        self.generate_challenges(context);
        self.role = TeamRole::Runner;
        let (request, timer) = context.engine_context.timer_tracker.timer(
            context.config.grace_period_duration,
            InternEngineCommand::TeamLeftGracePeriod {
                session_id: context.session_id,
                team_id: catcher_id,
            },
        );
        self.grace_period_end = Some(timer);
        self.points += bounty;
        self.bounty = 0;
        self.new_period(PeriodContext::Catcher {
            caught_team: caught_id,
            bounty,
        });
        request
    }

    /// Sets all the values inside the team that need to be set if it just completed a challenge.
    /// Returns the completed challenge and, optionally, a `RuntimeRequest` that cencels the grace
    /// period end timer.
    pub fn complete_challenge(
        &mut self,
        id: usize,
        context: &SessionContext,
    ) -> Result<(InOpenChallenge, Option<RuntimeRequest>), commands::Error> {
        match self.challenges.get(id).cloned() {
            None => Err(commands::Error::NotFound(format!(
                "challenge with id/index {}",
                id
            ))),
            Some(completed) => {
                let request = self
                    .grace_period_end
                    .take()
                    .map(|hook| hook.cancel_request());
                self.points += completed.points;
                self.bounty += (completed.points as f64 * context.config.bounty_percentage) as u64;
                self.new_period(PeriodContext::CompletedChallenge {
                    title: completed.title.clone(),
                    description: completed.description.clone(),
                    zone: completed.zone,
                    points: completed.points,
                    id: completed.id,
                });
                if let Some(zone) = completed.zone {
                    self.current_zone_id = zone;
                }
                self.generate_challenges(context);
                Ok((completed, request))
            }
        }
    }
}

/// Challenge generation period
enum GenerationPeriod {
    Specific,
    Normal,
    Perimeter(f64),
    ZKaff(f64),
    EndGame,
}

/// Gets the currently applicable `GenerationPeriod` from the current time and a config.
fn get_period(config: &Config) -> GenerationPeriod {
    let mut now = chrono::Local::now().time();
    if now <= config.start_time + chrono::Duration::minutes(config.specific_minutes as i64) {
        GenerationPeriod::Specific
    } else if now >= config.end_time {
        GenerationPeriod::EndGame
    } else {
        let wiggle = chrono::Duration::seconds(rng().random_range(
            -60 * config.time_wiggle_minutes as i64..=60 * config.time_wiggle_minutes as i64,
        ));
        now += wiggle;
        let end_game_time = config.end_time - Dur::minutes(config.end_game_minutes as i64);
        let zürich_time = end_game_time - Dur::minutes(config.zkaff_minutes as i64);
        let perimeter_time = zürich_time - Dur::minutes(config.perimeter_minutes as i64);
        if now >= end_game_time {
            GenerationPeriod::EndGame
        } else if now >= zürich_time {
            let ratio = (now - zürich_time).num_minutes() as f64 / config.zkaff_minutes as f64;
            GenerationPeriod::ZKaff(ratio)
        } else if now >= perimeter_time {
            let ratio =
                (now - perimeter_time).num_minutes() as f64 / config.perimeter_minutes as f64;
            GenerationPeriod::Perimeter(ratio)
        } else {
            GenerationPeriod::Normal
        }
    }
}

/// It lerps
fn lerp(a: u64, b: u64, t: f64) -> u64 {
    (a as f64 + (b as f64 - a as f64) * t) as u64
}
