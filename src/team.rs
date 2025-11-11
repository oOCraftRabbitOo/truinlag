use std::collections::HashMap;

use crate::{runtime::RuntimeRequest, InternEngineCommand, SessionContext, TimerHook};

use super::{
    challenge::{ChallengeEntry, InOpenChallenge},
    Config, DBEntry, ZoneEntry,
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
        }
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

        let zone_entries = context.engine_context.zone_db;
        let config = &context.config;
        let raw_challenges = context.engine_context.challenge_db.get_all();
        let team_zone = zone_entries
            .get(self.current_zone_id)
            .expect("team is in a zone that is not in db");
        let points_to_top = context
            .top_team_points
            .map(|p| p.saturating_sub(self.points));

        let challenge_set_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            config
                .challenge_sets
                .iter()
                .any(|s| c.contents.sets.contains(s))
        };
        let approved_status_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            matches!(
                c.contents.status,
                ChallengeStatus::Approved | ChallengeStatus::Refactor
            )
        };
        let not_completed_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            self.periods
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
        };
        let specific_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            matches!(
                c.contents.kind,
                ChallengeType::Kaff | ChallengeType::Ortsspezifisch | ChallengeType::Zoneable
            )
        };
        let unspecific_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            matches!(
                c.contents.kind,
                ChallengeType::Zoneable | ChallengeType::Unspezifisch
            )
        };
        let zkaff_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            matches!(c.contents.kind, ChallengeType::ZKaff)
        };
        let max_kaff_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            c.contents.kaffskala.is_none()
                || matches!(
                c.contents.kaffskala,
                Some(x) if x as u64 <= config.perim_max_kaff)
        };
        let regio_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            matches!(c.contents.kind, ChallengeType::Regionsspezifisch)
        };
        let far_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            config
                .normal_period_far_distance_range
                .contains(&c.contents.distance(&team_zone))
        };
        let near_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
            config
                .normal_period_near_distance_range
                .contains(&c.contents.distance(&team_zone))
        };

        let mut filtered_raw_challenges: Vec<DBEntry<ChallengeEntry>> = raw_challenges
            .iter()
            .filter(approved_status_filter)
            .filter(challenge_set_filter)
            .filter(not_completed_filter)
            .cloned()
            .collect();
        if filtered_raw_challenges.is_empty() {
            filtered_raw_challenges = raw_challenges.to_vec();
            eprintln!(
                "Engine: there are no more challenges that can be generated \
                for team {} with the currently selected challenge sets. \
                using all challenges",
                self.name
            );
        }

        let fallback_challenges = self.generate_fallback_challenges(
            &filtered_raw_challenges,
            team_zone.clone(),
            team_zone.clone(),
            points_to_top,
            context,
        );

        let centre_zone = match zone_entries.find(|z| z.zone == config.centre_zone) {
            Some(centre_zone) => centre_zone,
            None => {
                eprintln!(
                    "Engine: VERY BAD ERROR: \
                    couldn't find centre zone during challenge selection, \
                    generating fallback challenges instead"
                );
                self.challenges = fallback_challenges;
                return;
            }
        };

        let challenge_or =
            |chosen_challenges: [Option<&DBEntry<ChallengeEntry>>; 3]| -> Vec<InOpenChallenge> {
                if chosen_challenges.iter().any(|c| c.is_none()) {
                    eprintln!(
                        "not enough challenges for challenge generation, \
                            using fallback instead"
                    );
                    fallback_challenges.clone()
                } else {
                    chosen_challenges
                        .iter()
                        .enumerate()
                        .map(|(index, c)| {
                            let c = c.expect("I just checked that all elements are Some");
                            // this is a bit of a hack, but the last challenge is always the one
                            // that shouldn't be zoned. It's okay since this closure only works
                            // with three challenges anyways.
                            if index == 2 {
                                c.contents.challenge(
                                    false,
                                    centre_zone.clone(),
                                    team_zone.clone(),
                                    c.id,
                                    points_to_top,
                                    context,
                                )
                            } else {
                                c.contents.challenge(
                                    true,
                                    centre_zone.clone(),
                                    team_zone.clone(),
                                    c.id,
                                    points_to_top,
                                    context,
                                )
                            }
                        })
                        .collect()
                }
            };

        self.challenges = match get_period(config) {
            GenerationPeriod::Specific => {
                // The specific period challenge generation is used at the very start of the game
                // and it is the most simple. Three challenges are generated and they must all be
                // specific.
                filtered_raw_challenges
                    .iter()
                    .filter(specific_filter)
                    .choose_multiple(&mut rng(), config.num_challenges as usize)
                    .iter()
                    .map(|c| {
                        c.contents.challenge(
                            true,
                            centre_zone.clone(),
                            team_zone.clone(),
                            c.id,
                            points_to_top,
                            context,
                        )
                    })
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
                    let regio_probability = match config.regio_ratio {
                        ..=0.0 => 0.0,
                        1.0.. => 1.0,
                        other => other,
                    };
                    let chosen_challenges = [
                        if rng().random_bool(regio_probability) {
                            filtered_raw_challenges
                                .iter()
                                .filter(regio_filter)
                                .choose(&mut rng())
                        } else {
                            filtered_raw_challenges
                                .iter()
                                .filter(specific_filter)
                                .filter(near_filter)
                                .choose(&mut rng())
                        },
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(far_filter)
                            .choose(&mut rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(unspecific_filter)
                            .choose(&mut rng()),
                    ];
                    challenge_or(chosen_challenges)
                } else {
                    fallback_challenges
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
                        config.perim_distance_range.start,
                        config.perim_distance_range.end,
                        ratio,
                    );
                    let perim_near_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
                        (0..max_perim / 2).contains(&c.contents.distance(&centre_zone))
                            && specific_filter(c)
                            && max_kaff_filter(c)
                    };
                    let perim_far_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
                        specific_filter(c)
                            && max_kaff_filter(c)
                            && (max_perim / 2..max_perim)
                                .contains(&c.contents.distance(&centre_zone))
                    };
                    let chosen_challenges = [
                        filtered_raw_challenges
                            .iter()
                            .filter(perim_far_filter)
                            .choose(&mut rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(perim_near_filter)
                            .choose(&mut rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(unspecific_filter)
                            .choose(&mut rng()),
                    ];
                    challenge_or(chosen_challenges)
                } else {
                    fallback_challenges
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
                    let max_perim = config.perim_distance_range.end;
                    let perim_near_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
                        (0..max_perim / 2).contains(&c.contents.distance(&centre_zone))
                            && specific_filter(c)
                            && max_kaff_filter(c)
                    };
                    let perim_far_filter = |c: &&DBEntry<ChallengeEntry>| -> bool {
                        specific_filter(c)
                            && max_kaff_filter(c)
                            && (max_perim / 2..max_perim)
                                .contains(&c.contents.distance(&centre_zone))
                    };
                    let mut chosen_challenges = [
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(perim_far_filter)
                            .choose(&mut rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(perim_near_filter)
                            .choose(&mut rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(unspecific_filter)
                            .choose(&mut rng()),
                    ];
                    // changing 2 challenges to zkaff
                    let ratio = (1.0 - ratio) * config.zkaff_ratio_range.start
                        + ratio * config.zkaff_ratio_range.end;
                    let pool = match ratio {
                        ..=0.0 => false,
                        0.0..0.5 => rng().random_bool(ratio * 2.0),
                        0.5..1.0 => rng().random_bool((ratio - 0.5) * 2.0),
                        _ => true,
                    };
                    if (ratio < 0.5 && pool) || (ratio >= 0.5 && !pool) {
                        chosen_challenges[0] = filtered_raw_challenges
                            .iter()
                            .filter(zkaff_filter)
                            .choose(&mut rng());
                    } else if ratio >= 0.5 && pool {
                        let kaff_challenges = filtered_raw_challenges
                            .iter()
                            .filter(zkaff_filter)
                            .choose_multiple(&mut rng(), 2);
                        chosen_challenges[0] = kaff_challenges.first().cloned();
                        chosen_challenges[1] = kaff_challenges.get(1).cloned();
                    }
                    challenge_or(chosen_challenges)
                } else {
                    let mut chosen_challenges = filtered_raw_challenges
                        .iter()
                        .filter(zkaff_filter)
                        .choose_multiple(&mut rng(), config.num_challenges as usize - 1);
                    if let Some(c) = filtered_raw_challenges
                        .iter()
                        .filter(unspecific_filter)
                        .choose(&mut rng())
                    {
                        chosen_challenges.push(c);
                    }
                    if chosen_challenges.len() < config.num_challenges as usize {
                        eprintln!("Not enough challenges for zkaff selection, using fallback");
                        fallback_challenges
                    } else {
                        chosen_challenges
                            .iter()
                            .map(|c| {
                                c.contents.challenge(
                                    false,
                                    centre_zone.clone(),
                                    team_zone.clone(),
                                    c.id,
                                    points_to_top,
                                    context,
                                )
                            })
                            .collect()
                    }
                }
            }
            GenerationPeriod::EndGame => {
                // End game challenge generation is relatively simple. 1 Challenge is ZKaff, the others are
                // unspecific. No further restrictions apply.
                let mut chosen_challenges = filtered_raw_challenges
                    .iter()
                    .filter(unspecific_filter)
                    .choose_multiple(&mut rng(), config.num_challenges as usize - 1);
                if let Some(c) = filtered_raw_challenges
                    .iter()
                    .filter(zkaff_filter)
                    .choose(&mut rng())
                {
                    chosen_challenges.push(c);
                }
                if chosen_challenges.len() == config.num_challenges as usize {
                    chosen_challenges
                        .iter()
                        .map(|c| {
                            c.contents.challenge(
                                false,
                                centre_zone.clone(),
                                team_zone.clone(),
                                c.id,
                                points_to_top,
                                context,
                            )
                        })
                        .collect()
                } else {
                    fallback_challenges
                }
            }
        };
        self.challenges.shuffle(&mut rng());
    }

    /// Some weird bad code that still works and is used in challenge generation. Will try to
    /// generate some fallback challenges at any cost, and if things go wrong, those challenges may
    /// be kind of broken (but that is to be expected).
    pub(self) fn generate_fallback_challenges(
        &self,
        raw_challenges: &[DBEntry<ChallengeEntry>],
        centre_zone: DBEntry<ZoneEntry>,
        team_zone: DBEntry<ZoneEntry>,
        points_to_top: Option<u64>,
        context: &SessionContext,
    ) -> Vec<InOpenChallenge> {
        // this function is old, but it still generates the fallback challenges during challenge
        // selection. It is old and ugly, but it's not worth changing (yet), especially since the
        // fallback challenge system will probably never change much.

        let challenges = raw_challenges
            .iter()
            .filter(|c| {
                matches!(
                    c.contents.kind,
                    ChallengeType::Kaff | ChallengeType::Ortsspezifisch | ChallengeType::Zoneable
                )
            })
            .choose_multiple(&mut rng(), context.config.num_challenges as usize - 1);
        let mut challenges: Vec<InOpenChallenge> = challenges
            .iter()
            .map(|c| {
                c.contents.challenge(
                    true,
                    centre_zone.clone(),
                    team_zone.clone(),
                    c.id,
                    points_to_top,
                    context,
                )
            })
            .collect();
        let mut unspecific_challenges: Vec<&DBEntry<ChallengeEntry>> = raw_challenges
            .iter()
            .filter(|c| {
                matches!(
                    c.contents.kind,
                    ChallengeType::Unspezifisch | ChallengeType::Zoneable
                )
            })
            .collect();
        challenges.push(smart_choose(
            &mut unspecific_challenges,
            raw_challenges,
            "choosing the unspecific challenge during fallback generation",
            false,
            centre_zone,
            team_zone,
            points_to_top,
            context,
        ));
        challenges
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

/// Some weird bad code that still works and is used in challenge generation. Will try to
/// generate some fallback challenges at any cost, and if things go wrong, those challenges may
/// be kind of broken (but that is to be expected).
#[allow(clippy::too_many_arguments)]
fn smart_choose(
    challenges: &mut Vec<&DBEntry<ChallengeEntry>>,
    raw_challenges: &[DBEntry<ChallengeEntry>],
    context_message: &str,
    zone_zoneables: bool,
    centre_zone: DBEntry<ZoneEntry>,
    team_zone: DBEntry<ZoneEntry>,
    points_to_top: Option<u64>,
    context: &SessionContext,
) -> InOpenChallenge {
    match challenges.iter().enumerate().choose(&mut rng()) {
        None => {
            eprintln!(
                "Engine: {} Not enough challenges, context: {}. \
                generatig from all uncompleted challenges within the currents sets",
                chrono::Local::now(),
                context_message
            );
            let selected = raw_challenges
                .choose(&mut rng())
                .cloned()
                .unwrap_or_else(|| {
                    eprintln!(
                        "Engine: {} Not enough challenges, context: {}. \
                        generating from all challenges",
                        chrono::Local::now(),
                        context_message
                    );
                    context
                        .engine_context
                        .challenge_db
                        .get_all()
                        .choose(&mut rng())
                        .expect(
                            "There were no raw challenges, \
                            that should never happen",
                        )
                        .clone()
                });
            selected.contents.challenge(
                zone_zoneables,
                centre_zone,
                team_zone,
                selected.id,
                points_to_top,
                context,
            )
        }
        Some((i, selected)) => {
            let ret = selected.contents.challenge(
                zone_zoneables,
                centre_zone,
                team_zone,
                selected.id,
                points_to_top,
                context,
            );
            challenges.remove(i);
            ret
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
            let ratio = (zürich_time - now).num_minutes() as f64 / config.zkaff_minutes as f64;
            GenerationPeriod::ZKaff(ratio)
        } else if now >= perimeter_time {
            let ratio =
                (perimeter_time - now).num_minutes() as f64 / config.perimeter_minutes as f64;
            GenerationPeriod::Perimeter(ratio)
        } else {
            GenerationPeriod::Normal
        }
    }
}

/// It lerps
fn lerp(min: u64, max: u64, t: f64) -> u64 {
    (min as f64 * (1_f64 - t) + max as f64) as u64
}
