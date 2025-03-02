use crate::DBMirror;

use super::{
    challenge::{ChallengeEntry, InOpenChallenge},
    Config, DBEntry, PlayerEntry, ZoneEntry,
};
use chrono::{self, Duration as Dur, NaiveTime};
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::min;
use truinlag::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamEntry {
    pub name: String,
    pub players: Vec<u64>,
    pub discord_channel: Option<u64>,
    pub role: TeamRole,
    pub colour: Colour,
    pub points: u64,
    pub bounty: u64,
    pub zone_id: u64,
    pub locations: Vec<(f64, f64, NaiveTime)>,
    pub challenges: Vec<InOpenChallenge>,
    pub periods: Vec<Period>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Period {
    pub context: PeriodContext,
    pub position_start_index: u64,
    pub position_end_index: u64,
    pub end_time: chrono::NaiveTime,
}

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
        photo: u64,          // the id of the challenge photo
        id: u64,
    },
}

impl PartialEq for Period {
    fn eq(&self, other: &Self) -> bool {
        self.end_time.eq(&other.end_time)
    }
}

impl Eq for Period {}

impl PartialOrd for Period {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

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
            players,
            discord_channel,
            colour,
            challenges: Vec::with_capacity(config.num_challenges as usize),
            role: TeamRole::Runner,
            points: 0,
            bounty: 0,
            locations: Vec::new(),
            periods: Vec::new(),
            zone_id: config.start_zone,
        }
    }
    pub fn to_sendable(
        &self,
        player_entries: &DBMirror<PlayerEntry>,
        index: usize,
    ) -> truinlag::Team {
        truinlag::Team {
            colour: self.colour,
            role: self.role,
            name: self.name.clone(),
            id: index,
            bounty: self.bounty,
            points: self.points,
            players: self
                .players
                .iter()
                .map(|p| {
                    player_entries
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
                        photo: _,
                    } => Some(CompletedChallenge {
                        title: title.clone(),
                        description: description.clone(),
                        points: *points,
                        time: p.end_time,
                        id: *id,
                    }),
                    _ => None,
                })
                .collect(),
            location: self.locations.last().map(|loc| (loc.0, loc.1)),
        }
    }

    pub fn location(&self) -> Option<(f64, f64)> {
        self.locations
            .last()
            .map(|location| (location.0, location.1))
    }

    pub fn start_runner(
        &mut self,
        config: &Config,
        challenge_db: &[DBEntry<ChallengeEntry>],
        zone_db: &DBMirror<ZoneEntry>,
    ) {
        self.reset(config.start_zone);
        self.generate_challenges(config, challenge_db, zone_db);
    }

    pub fn start_catcher(&mut self, config: &Config) {
        self.reset(config.start_zone);
        self.role = TeamRole::Catcher;
    }

    fn reset(&mut self, start_zone_id: u64) {
        self.challenges = Vec::new();
        self.periods = Vec::new();
        self.locations = Vec::new();
        self.role = TeamRole::Runner;
        self.zone_id = start_zone_id;
        self.points = 0;
        self.bounty = 0;
    }

    pub fn generate_challenges(
        &mut self,
        config: &Config,
        raw_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
    ) {
        // Challenge selection is the perhaps most complicated part of the game, so I think it
        // warrants some good commenting. The process for challenge generation differs mainly based
        // on the generation period. There are (currently) 5 of them. In this function, the current
        // period is obtained in the form of an enum from a top level function and a match
        // expression is used to perform different actions depending on the period. Note that the
        // challenge generation process is completely different from period to period, so the match
        // expression actually makes up almost the entire function body.
        let mut filtered_raw_challenges: Vec<&DBEntry<ChallengeEntry>> = raw_challenges
            .iter()
            .filter(|c| {
                config
                    .challenge_sets
                    .iter()
                    .any(|s| c.contents.sets.contains(s))
                    && self
                        .periods
                        .iter()
                        .filter_map(|p| match &p.context {
                            PeriodContext::CompletedChallenge {
                                title: _,
                                description: _,
                                zone: _,
                                points: _,
                                photo: _,
                                id,
                            } => Some(id),
                            _ => None,
                        })
                        .all(|i| *i != c.id)
            })
            .collect();
        if filtered_raw_challenges.is_empty() {
            filtered_raw_challenges = raw_challenges.iter().collect();
            eprintln!(
                "Engine: there are no more challenges that can be generated \
                for team {} with the currently selected challenge sets. \
                using all challenges",
                self.name
            );
        }
        let team_zone = zone_entries
            .get(self.zone_id)
            .expect("team is in a zone that is not in db");
        self.challenges = match get_period(config) {
            GenerationPeriod::Specific => {
                self.generate_specific_challenges(config, &filtered_raw_challenges, zone_entries)
            }
            GenerationPeriod::Normal => self.generate_normal_challenges(
                config,
                &filtered_raw_challenges,
                raw_challenges,
                zone_entries,
                &team_zone,
            ),
            GenerationPeriod::Perimeter(ratio) => self.generate_perimeter_challenges(
                config,
                &filtered_raw_challenges,
                raw_challenges,
                zone_entries,
                ratio,
            ),
            GenerationPeriod::ZKaff(ratio) => self.generate_zkaff_challenges(
                config,
                &filtered_raw_challenges,
                raw_challenges,
                zone_entries,
                ratio,
            ),
            GenerationPeriod::EndGame => self.generate_end_game_challenges(
                config,
                &filtered_raw_challenges,
                raw_challenges,
                zone_entries,
            ),
        };
        self.challenges.shuffle(&mut thread_rng());
    }

    pub(self) fn generate_end_game_challenges(
        &self,
        config: &Config,
        raw_challenges: &[&DBEntry<ChallengeEntry>],
        backup_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
    ) -> Vec<InOpenChallenge> {
        // End game challenge generation is relatively simple. 1 Challenge is ZKaff, the others are
        // unspecific. No further restrictions apply.

        let mut unspecific_challenges = raw_challenges
            .iter()
            .filter(|c| {
                matches!(
                    c.contents.kind,
                    ChallengeType::Zoneable | ChallengeType::Unspezifisch
                )
            })
            .collect();

        let mut ret = Vec::new();

        for _ in 1..config.num_challenges - 1 {
            ret.push(smart_choose(
                &mut unspecific_challenges,
                raw_challenges,
                backup_challenges,
                "generating an unspecific challenge during end game period",
                config,
                false,
                zone_entries,
            ));
        }

        let mut zkaff_challenges = raw_challenges
            .iter()
            .filter(|c| matches!(c.contents.kind, ChallengeType::ZKaff))
            .collect();

        ret.push(smart_choose(
            &mut zkaff_challenges,
            raw_challenges,
            backup_challenges,
            "generating ZKaff challenge in end game period",
            config,
            true,
            zone_entries,
        ));

        ret
    }

    pub(self) fn generate_zkaff_challenges(
        &self,
        config: &Config,
        raw_challenges: &[&DBEntry<ChallengeEntry>],
        backup_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        ratio: f64,
    ) -> Vec<InOpenChallenge> {
        // ZKaff challenge generation works the same as perimeter generation, except that ratio of
        // the generated specific challenges will be ZKaff challenges. For increased consistency,
        // the probability is applied sequentially, first to the far challenge, then to the near
        // one. So if ratio = 0.5, the far challenge will always be a ZKaff challenge and the near
        // one never. If ratio = 0.75, the far challenge will always ZKaff and the near one has a
        // 50/50 chance.
        // Also, if there are not 3 challenges, we use fallback generation here as well, mostly
        // because I'm lazy.

        let ratio =
            (1.0 - ratio) * config.zkaff_ratio_range.start + ratio * config.zkaff_ratio_range.end;

        let mut zkaff_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
            .iter()
            .filter(|c| matches!(c.contents.kind, ChallengeType::ZKaff))
            .collect();

        let mut generated = self.generate_perimeter_challenges(
            config,
            raw_challenges,
            backup_challenges,
            zone_entries,
            1_f64,
        );

        if config.num_challenges == 3 {
            if ratio < 0.5 {
                if ratio >= 0.0 && thread_rng().gen_bool(ratio * 2_f64) {
                    generated[0] = smart_choose(
                        &mut zkaff_challenges,
                        raw_challenges,
                        backup_challenges,
                        "generating first ZKaff challenge",
                        config,
                        true,
                        zone_entries,
                    );
                }
                generated
            } else {
                generated[0] = smart_choose(
                    &mut zkaff_challenges,
                    raw_challenges,
                    backup_challenges,
                    "generating first ZKaff challenge",
                    config,
                    true,
                    zone_entries,
                );
                if ratio > 1.0 || thread_rng().gen_bool((ratio - 0.5) * 2.0) {
                    generated[1] = smart_choose(
                        &mut zkaff_challenges,
                        raw_challenges,
                        backup_challenges,
                        "generating second ZKaff challenge",
                        config,
                        true,
                        zone_entries,
                    );
                }
                generated
            }
        } else {
            self.generate_fallback_challenges(
                config,
                raw_challenges,
                backup_challenges,
                zone_entries,
            )
        }
    }

    pub(self) fn generate_specific_challenges(
        &self,
        config: &Config,
        raw_challenges: &[&DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
    ) -> Vec<InOpenChallenge> {
        // The specific period challenge generation is used at the very start of the game
        // and it is the most simple. Three challenges are generated and they must all be
        // either kaff challenges or ortsspezifisch.
        raw_challenges
            .iter()
            .filter(|c| {
                matches!(
                    c.contents.kind,
                    ChallengeType::Kaff | ChallengeType::Ortsspezifisch | ChallengeType::Zoneable
                )
            })
            .choose_multiple(&mut thread_rng(), config.num_challenges as usize)
            // There is no extensive fallback system here for if there aren't enough challenges
            // available, but it should be fine, since this generation type is only used at the
            // start of the game.
            .iter()
            .map(|r| r.contents.challenge(config, true, zone_entries, r.id))
            .collect()
    }

    pub(self) fn generate_perimeter_challenges(
        &self,
        config: &Config,
        raw_challenges: &[&DBEntry<ChallengeEntry>],
        backup_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        ratio: f64,
    ) -> Vec<InOpenChallenge> {
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

        match zone_entries.find(|z| z.zone == config.centre_zone) {
            Some(centre_zone) => {
                if config.num_challenges == 3 {
                    let current_max_perim = lerp(
                        config.perim_distance_range.start,
                        config.perim_distance_range.end,
                        ratio,
                    );

                    let mut far_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                        .iter()
                        .filter(|c| {
                            (matches!(
                                c.contents.kind,
                                ChallengeType::Ortsspezifisch | ChallengeType::Kaff
                            ) && (c.contents.kaffskala.is_none()
                                || matches!(
                                    c.contents.kaffskala,
                                    Some(x) if x as u64 <= config.perim_max_kaff))
                                && (0..current_max_perim / 2)
                                    .contains(&c.contents.distance(&centre_zone)))
                                || matches!(c.contents.kind, ChallengeType::Regionsspezifisch)
                        })
                        .collect();

                    let mut near_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                        .iter()
                        .filter(|c| {
                            (matches!(
                                c.contents.kind,
                                ChallengeType::Ortsspezifisch | ChallengeType::Kaff
                            ) && (c.contents.kaffskala.is_none()
                                || matches!(
                                    c.contents.kaffskala,
                                    Some(x) if x as u64 <= config.perim_max_kaff
                                ))
                                && (current_max_perim / 2..current_max_perim)
                                    .contains(&c.contents.distance(&centre_zone)))
                                || matches!(c.contents.kind, ChallengeType::Zoneable)
                        })
                        .collect();

                    let mut unspecific_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                        .iter()
                        .filter(|c| {
                            matches!(
                                c.contents.kind,
                                ChallengeType::Unspezifisch | ChallengeType::Zoneable
                            )
                        })
                        .collect();

                    vec![
                        smart_choose(
                            &mut far_challenges,
                            raw_challenges,
                            backup_challenges,
                            "generating the far perimeter challenge",
                            config,
                            true,
                            zone_entries,
                        ),
                        smart_choose(
                            &mut near_challenges,
                            raw_challenges,
                            backup_challenges,
                            "generating the near perimeter challenge",
                            config,
                            true,
                            zone_entries,
                        ),
                        smart_choose(
                            &mut unspecific_challenges,
                            raw_challenges,
                            backup_challenges,
                            "generating the unspecific perimeter challenge",
                            config,
                            false,
                            zone_entries,
                        ),
                    ]
                } else {
                    self.generate_fallback_challenges(
                        config,
                        raw_challenges,
                        backup_challenges,
                        zone_entries,
                    )
                }
            }
            None => {
                eprintln!(
                    "Engine: VERY BAD ERROR: \
                    couldn't find zone 110 while generating perimeter challenge, \
                    generating fallback challenges instead"
                );
                self.generate_fallback_challenges(
                    config,
                    raw_challenges,
                    backup_challenges,
                    zone_entries,
                )
            }
        }
    }

    pub(self) fn generate_normal_challenges(
        &self,
        config: &Config,
        raw_challenges: &[&DBEntry<ChallengeEntry>],
        backup_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        team_zone: &DBEntry<ZoneEntry>,
    ) -> Vec<InOpenChallenge> {
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
            let mut far_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                .iter()
                .filter(|c| {
                    (matches!(
                        c.contents.kind,
                        ChallengeType::Kaff | ChallengeType::Ortsspezifisch
                    ) && config
                        .normal_period_far_distance_range
                        .contains(&c.contents.distance(team_zone)))
                        || (matches!(c.contents.kind, ChallengeType::Zoneable))
                })
                .collect();

            let mut near_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                .iter()
                .filter(|c| {
                    (matches!(
                        c.contents.kind,
                        ChallengeType::Kaff | ChallengeType::Ortsspezifisch
                    ) && config
                        .normal_period_near_distance_range
                        .contains(&c.contents.distance(team_zone)))
                        || (matches!(c.contents.kind, ChallengeType::Zoneable))
                })
                .collect();

            let mut regio_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                .iter()
                .filter(|c| matches!(c.contents.kind, ChallengeType::Regionsspezifisch))
                .collect();

            let mut unspecific_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
                .iter()
                .filter(|c| {
                    matches!(
                        c.contents.kind,
                        ChallengeType::Unspezifisch | ChallengeType::Zoneable
                    )
                })
                .collect();
            vec![
                if thread_rng().gen_bool(config.regio_ratio) {
                    smart_choose(
                        &mut far_challenges,
                        raw_challenges,
                        backup_challenges,
                        "choosing the far away challenge during normal period",
                        config,
                        true,
                        zone_entries,
                    )
                } else {
                    smart_choose(
                        &mut regio_challenges,
                        raw_challenges,
                        backup_challenges,
                        "choosing the regio challenge during normal period",
                        config,
                        false,
                        zone_entries,
                    )
                },
                smart_choose(
                    &mut near_challenges,
                    raw_challenges,
                    backup_challenges,
                    "choosing the near challenge during normal period",
                    config,
                    true,
                    zone_entries,
                ),
                smart_choose(
                    &mut unspecific_challenges,
                    raw_challenges,
                    backup_challenges,
                    "choosing the unspecdific challenge during normal period",
                    config,
                    false,
                    zone_entries,
                ),
            ]
        } else {
            self.generate_fallback_challenges(
                config,
                raw_challenges,
                backup_challenges,
                zone_entries,
            )
        }
    }

    pub(self) fn generate_fallback_challenges(
        &self,
        config: &Config,
        raw_challenges: &[&DBEntry<ChallengeEntry>],
        backup_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
    ) -> Vec<InOpenChallenge> {
        // This function is called by other challenge generation functions if they for some reason
        // cannot generate their challenges as expected and, in particular, if there is an unexpected
        // amount of challenges. This system generates all challenges the same way as
        // during the specific period save one which will be unspecific.

        let challenges = raw_challenges
            .iter()
            .filter(|c| {
                matches!(
                    c.contents.kind,
                    ChallengeType::Kaff | ChallengeType::Ortsspezifisch | ChallengeType::Zoneable
                )
            })
            .choose_multiple(&mut thread_rng(), config.num_challenges as usize - 1);
        let mut challenges: Vec<InOpenChallenge> = challenges
            .iter()
            .map(|c| c.contents.challenge(config, true, zone_entries, c.id))
            .collect();
        let mut unspecific_challenges: Vec<&&DBEntry<ChallengeEntry>> = raw_challenges
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
            backup_challenges,
            "choosing the unspecific challenge during backup normal period",
            config,
            false,
            zone_entries,
        ));
        challenges
    }

    fn new_period(&mut self, context: PeriodContext) {
        self.periods.push(Period {
            context,
            position_start_index: min(
                self.periods
                    .last()
                    .map(|p| p.position_end_index + 1)
                    .unwrap_or(0),
                self.locations.len() as u64 - 1,
            ),
            position_end_index: self.locations.len() as u64 - 1,
            end_time: chrono::Local::now().time(),
        });
    }

    pub fn be_caught(&mut self, catcher_id: usize) {
        self.challenges.clear();
        self.role = TeamRole::Catcher;
        self.new_period(PeriodContext::Caught {
            catcher_team: catcher_id,
            bounty: self.bounty,
        });
        self.bounty = 0;
    }

    pub fn have_caught(
        &mut self,
        bounty: u64,
        caught_id: usize,
        config: &Config,
        challenge_db: &[DBEntry<ChallengeEntry>],
        zone_db: &DBMirror<ZoneEntry>,
    ) {
        self.generate_challenges(config, challenge_db, zone_db);
        self.role = TeamRole::Runner;
        self.points += bounty;
        self.bounty = 0;
        self.new_period(PeriodContext::Catcher {
            caught_team: caught_id,
            bounty,
        });
    }

    pub fn complete_challenge(
        &mut self,
        id: usize,
        config: &Config,
        challenge_db: &[DBEntry<ChallengeEntry>],
        zone_db: &DBMirror<ZoneEntry>,
    ) -> Result<InOpenChallenge, commands::Error> {
        match self.challenges.get(id).cloned() {
            None => Err(commands::Error::NotFound(format!(
                "challenge with id/index {}",
                id
            ))),
            Some(completed) => {
                self.points += completed.points;
                self.bounty += (completed.points as f64 * config.bounty_percentage) as u64;
                self.new_period(PeriodContext::CompletedChallenge {
                    title: completed.title.clone(),
                    description: completed.description.clone(),
                    zone: completed.zone,
                    points: completed.points,
                    photo: 0,
                    id: completed.id,
                });
                if let Some(zone) = completed.zone {
                    self.zone_id = zone;
                }
                self.generate_challenges(config, challenge_db, zone_db);
                Ok(completed)
            }
        }
    }
}

fn smart_choose(
    challenges: &mut Vec<&&DBEntry<ChallengeEntry>>,
    raw_challenges: &[&DBEntry<ChallengeEntry>],
    backup_challenges: &[DBEntry<ChallengeEntry>],
    context: &str,
    config: &Config,
    zone_zoneables: bool,
    zone_db: &DBMirror<ZoneEntry>,
) -> InOpenChallenge {
    match challenges.iter().enumerate().choose(&mut thread_rng()) {
        None => {
            eprintln!(
                "Engine: {} Not enough challenges, context: {}. \
                generatig from all uncompleted challenges within the currents sets",
                chrono::Local::now(),
                context
            );
            let selected = raw_challenges
                .choose(&mut thread_rng())
                .cloned()
                .unwrap_or_else(|| {
                    eprintln!(
                        "Engine: {} Not enough challenges, context: {}. \
                        generating from all challenges",
                        chrono::Local::now(),
                        context
                    );
                    backup_challenges.choose(&mut thread_rng()).expect(
                        "There were no raw challenges, \
                            that should never happen",
                    )
                });
            selected
                .contents
                .challenge(config, zone_zoneables, zone_db, selected.id)
        }
        Some((i, selected)) => {
            let ret = selected
                .contents
                .challenge(config, zone_zoneables, zone_db, selected.id);
            challenges.remove(i);
            ret
        }
    }
}

enum GenerationPeriod {
    Specific,
    Normal,
    Perimeter(f64),
    ZKaff(f64),
    EndGame,
}

fn get_period(config: &Config) -> GenerationPeriod {
    let mut now = chrono::Local::now().time();
    if now <= config.start_time + chrono::Duration::minutes(config.specific_minutes as i64) {
        GenerationPeriod::Specific
    } else if now >= config.end_time {
        GenerationPeriod::EndGame
    } else {
        let wiggle = chrono::Duration::seconds(thread_rng().gen_range(
            -60 * config.time_wiggle_minutes as i64..=60 * config.time_wiggle_minutes as i64,
        ));
        now += wiggle;
        let end_game_time = config.end_time - Dur::minutes(config.end_game_minutes as i64);
        let zurich_time = end_game_time - Dur::minutes(config.zkaff_minutes as i64);
        let perimeter_time = zurich_time - Dur::minutes(config.perimeter_minutes as i64);
        if now >= end_game_time {
            GenerationPeriod::EndGame
        } else if now >= zurich_time {
            let ratio = (now - zurich_time).num_minutes() as f64 / config.zkaff_minutes as f64;
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

fn lerp(min: u64, max: u64, t: f64) -> u64 {
    (min as f64 * (1_f64 - t) + max as f64) as u64
}
