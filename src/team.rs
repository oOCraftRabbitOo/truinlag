use crate::{
    runtime::{InternEngineCommand, RuntimeRequest},
    DBMirror,
};

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
    pub grace_period_end: Option<chrono::NaiveTime>,
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
            grace_period_end: None,
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
            in_grace_period: match self.grace_period_end {
                None => false,
                Some(time) => chrono::Local::now().time() <= time,
            },
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

        let team_zone = zone_entries
            .get(self.zone_id)
            .expect("team is in a zone that is not in db");

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
                        photo: _,
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
            config,
            &filtered_raw_challenges,
            raw_challenges,
            zone_entries,
            team_zone.clone(),
            team_zone.clone(),
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
                                    config,
                                    false,
                                    zone_entries,
                                    centre_zone.clone(),
                                    team_zone.clone(),
                                    c.id,
                                )
                            } else {
                                c.contents.challenge(
                                    config,
                                    true,
                                    zone_entries,
                                    centre_zone.clone(),
                                    team_zone.clone(),
                                    c.id,
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
                    .choose_multiple(&mut thread_rng(), config.num_challenges as usize)
                    .iter()
                    .map(|c| {
                        c.contents.challenge(
                            config,
                            true,
                            zone_entries,
                            centre_zone.clone(),
                            team_zone.clone(),
                            c.id,
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
                    let chosen_challenges = [
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(near_filter)
                            .choose(&mut thread_rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(far_filter)
                            .choose(&mut thread_rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(unspecific_filter)
                            .choose(&mut thread_rng()),
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
                        ((max_perim / 2..max_perim).contains(&c.contents.distance(&centre_zone))
                            && specific_filter(c)
                            && max_kaff_filter(c))
                            || regio_filter(c)
                    };
                    let chosen_challenges = [
                        filtered_raw_challenges
                            .iter()
                            .filter(perim_far_filter)
                            .choose(&mut thread_rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(perim_near_filter)
                            .choose(&mut thread_rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(unspecific_filter)
                            .choose(&mut thread_rng()),
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
                        ((max_perim / 2..max_perim).contains(&c.contents.distance(&centre_zone))
                            && specific_filter(c)
                            && max_kaff_filter(c))
                            || regio_filter(c)
                    };
                    let mut chosen_challenges = [
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(perim_far_filter)
                            .choose(&mut thread_rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(specific_filter)
                            .filter(perim_near_filter)
                            .choose(&mut thread_rng()),
                        filtered_raw_challenges
                            .iter()
                            .filter(unspecific_filter)
                            .choose(&mut thread_rng()),
                    ];
                    // changing 2 challenges to zkaff
                    let ratio = (1.0 - ratio) * config.zkaff_ratio_range.start
                        + ratio * config.zkaff_ratio_range.end;
                    let pool = match ratio {
                        ..=0.0 => false,
                        0.0..0.5 => thread_rng().gen_bool(ratio * 2.0),
                        0.5..1.0 => thread_rng().gen_bool((ratio - 0.5) * 2.0),
                        _ => true,
                    };
                    if (ratio < 0.5 && pool) || (ratio >= 0.5 && !pool) {
                        chosen_challenges[0] = filtered_raw_challenges
                            .iter()
                            .filter(zkaff_filter)
                            .choose(&mut thread_rng());
                    } else if ratio >= 0.5 && pool {
                        let kaff_challenges = filtered_raw_challenges
                            .iter()
                            .filter(zkaff_filter)
                            .choose_multiple(&mut thread_rng(), 2);
                        chosen_challenges[0] = kaff_challenges.first().cloned();
                        chosen_challenges[1] = kaff_challenges.get(1).cloned();
                    }
                    challenge_or(chosen_challenges)
                } else {
                    let mut chosen_challenges = filtered_raw_challenges
                        .iter()
                        .filter(zkaff_filter)
                        .choose_multiple(&mut thread_rng(), config.num_challenges as usize - 1);
                    if let Some(c) = filtered_raw_challenges
                        .iter()
                        .filter(unspecific_filter)
                        .choose(&mut thread_rng())
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
                                    config,
                                    false,
                                    zone_entries,
                                    centre_zone.clone(),
                                    team_zone.clone(),
                                    c.id,
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
                    .choose_multiple(&mut thread_rng(), config.num_challenges as usize - 1);
                if let Some(c) = filtered_raw_challenges
                    .iter()
                    .filter(zkaff_filter)
                    .choose(&mut thread_rng())
                {
                    chosen_challenges.push(c);
                }
                if chosen_challenges.len() == config.num_challenges as usize {
                    chosen_challenges
                        .iter()
                        .map(|c| {
                            c.contents.challenge(
                                config,
                                false,
                                zone_entries,
                                centre_zone.clone(),
                                team_zone.clone(),
                                c.id,
                            )
                        })
                        .collect()
                } else {
                    fallback_challenges
                }
            }
        };
        self.challenges.shuffle(&mut thread_rng());
    }

    pub(self) fn generate_fallback_challenges(
        &self,
        config: &Config,
        raw_challenges: &[DBEntry<ChallengeEntry>],
        backup_challenges: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        centre_zone: DBEntry<ZoneEntry>,
        team_zone: DBEntry<ZoneEntry>,
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
            .choose_multiple(&mut thread_rng(), config.num_challenges as usize - 1);
        let mut challenges: Vec<InOpenChallenge> = challenges
            .iter()
            .map(|c| {
                c.contents.challenge(
                    config,
                    true,
                    zone_entries,
                    centre_zone.clone(),
                    team_zone.clone(),
                    c.id,
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
            backup_challenges,
            "choosing the unspecific challenge during fallback generation",
            config,
            false,
            zone_entries,
            centre_zone,
            team_zone,
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
        self.grace_period_end = None;
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
    ) -> chrono::NaiveTime {
        self.generate_challenges(config, challenge_db, zone_db);
        self.role = TeamRole::Runner;
        let end_time = chrono::Local::now().time() + config.grace_period_duration;
        self.grace_period_end = Some(end_time);
        self.points += bounty;
        self.bounty = 0;
        self.new_period(PeriodContext::Catcher {
            caught_team: caught_id,
            bounty,
        });
        end_time
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
                self.grace_period_end = None;
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

#[allow(clippy::too_many_arguments)]
fn smart_choose(
    challenges: &mut Vec<&DBEntry<ChallengeEntry>>,
    raw_challenges: &[DBEntry<ChallengeEntry>],
    backup_challenges: &[DBEntry<ChallengeEntry>],
    context: &str,
    config: &Config,
    zone_zoneables: bool,
    zone_db: &DBMirror<ZoneEntry>,
    centre_zone: DBEntry<ZoneEntry>,
    team_zone: DBEntry<ZoneEntry>,
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
                    backup_challenges
                        .choose(&mut thread_rng())
                        .expect(
                            "There were no raw challenges, \
                            that should never happen",
                        )
                        .clone()
                });
            selected.contents.challenge(
                config,
                zone_zoneables,
                zone_db,
                centre_zone,
                team_zone,
                selected.id,
            )
        }
        Some((i, selected)) => {
            let ret = selected.contents.challenge(
                config,
                zone_zoneables,
                zone_db,
                centre_zone,
                team_zone,
                selected.id,
            );
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
