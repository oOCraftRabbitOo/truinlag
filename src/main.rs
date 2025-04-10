pub(crate) mod challenge;
pub(crate) mod engine;
mod error;
pub(crate) mod runtime;
pub(crate) mod session;
pub(crate) mod team;

use bonsaidb::{
    core::schema::{Collection, Schema, SerializedCollection},
    local::Database,
};
use challenge::{ChallengeEntry, ChallengeSetEntry, InOpenChallenge};
use error::Result;
use image::imageops::FilterType;
use partially::Partial;
use runtime::{manager, InternEngineCommand, RuntimeRequest};
use serde::{Deserialize, Serialize};
use session::Session;
use std::collections::HashMap;
use team::{PeriodContext, TeamEntry};
use truinlag::{
    commands::{EngineAction, EngineCommand},
    *,
};

#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TimerHook {
    payload: InternEngineCommand,
    end_time: chrono::DateTime<chrono::Local>,
    id: u64,
}

impl Default for TimerHook {
    fn default() -> Self {
        Self {
            payload: InternEngineCommand::Command(EngineCommand {
                action: EngineAction::Ping(None),
                session: None,
            }),
            end_time: chrono::Local::now(),
            id: u64::MAX,
        }
    }
}

impl TimerHook {
    fn cancel_request(&self) -> RuntimeRequest {
        RuntimeRequest::CancelTimer(self.id)
    }

    fn create_request(&self) -> RuntimeRequest {
        RuntimeRequest::CreateAlarm {
            time: self.end_time,
            payload: self.payload.clone(),
            id: self.id,
        }
    }
}

#[derive(Debug)]
pub struct TimerTracker {
    current_id: u64,
}

impl TimerTracker {
    fn new() -> Self {
        TimerTracker { current_id: 0 }
    }

    fn alarm(
        &mut self,
        time: chrono::DateTime<chrono::Local>,
        payload: InternEngineCommand,
    ) -> (RuntimeRequest, TimerHook) {
        self.current_id = self.current_id.wrapping_add(1);
        (
            RuntimeRequest::CreateAlarm {
                time,
                payload: payload.clone(),
                id: self.current_id,
            },
            TimerHook {
                payload,
                end_time: time,
                id: self.current_id,
            },
        )
    }

    fn timer(
        &mut self,
        duration: chrono::TimeDelta,
        payload: InternEngineCommand,
    ) -> (RuntimeRequest, TimerHook) {
        let time = chrono::Local::now() + duration;
        self.alarm(time, payload)
    }
}

#[derive(Debug)]
pub struct EngineContext<'a> {
    player_db: &'a DBMirror<PlayerEntry>,
    challenge_db: &'a DBMirror<ChallengeEntry>,
    zone_db: &'a DBMirror<ZoneEntry>,
    past_game_db: &'a mut DBMirror<PastGame>,
    timer_tracker: &'a mut TimerTracker,
}

#[derive(Debug)]
pub struct SessionContext<'a> {
    engine_context: EngineContext<'a>,
    top_team_points: Option<u64>,
    session_id: u64,
    config: Config,
}

#[derive(Clone, Debug)]
pub struct ClonedDBEntry<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    pub id: u64,
    pub contents: T,
}

#[derive(Debug, Clone, Copy)]
pub struct DBEntry<'a, T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    pub id: u64,
    pub contents: &'a T,
}

impl<T> DBEntry<'_, T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    pub fn clone_contents(&self) -> ClonedDBEntry<T> {
        ClonedDBEntry {
            id: self.id,
            contents: self.contents.clone(),
        }
    }
}

#[derive(Debug)]
pub struct MutDBEntry<'a, T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    pub id: u64,
    pub contents: &'a mut T,
}

impl<T> MutDBEntry<'_, T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    pub fn clone_contents(&self) -> ClonedDBEntry<T> {
        ClonedDBEntry {
            id: self.id,
            contents: self.contents.clone(),
        }
    }
}

#[derive(Clone, Debug)]
enum DBStatus {
    Unchanged,
    Edited,
    ToBeDeleted,
    BeingDeleted,
}

#[derive(Clone, Debug)]
pub struct DBMirror<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    entries: Vec<Option<(T, DBStatus)>>,
}

impl<T> DBMirror<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    fn from_db(db: &Database) -> Self {
        let db_entries = T::all(db).query().unwrap();
        let max_id = db_entries.iter().fold(
            0,
            |acc, e| if e.header.id > acc { e.header.id } else { acc },
        ) as usize;
        let mut entries = vec![None; max_id + 1];
        for entry in db_entries {
            entries[entry.header.id as usize] = Some((entry.contents, DBStatus::Unchanged));
        }

        Self { entries }
    }

    fn get(&self, id: u64) -> Option<DBEntry<T>> {
        match self.entries.get(id as usize) {
            Some(Some((contents, status))) => match status {
                DBStatus::Unchanged | DBStatus::Edited => Some(DBEntry { id, contents }),
                DBStatus::ToBeDeleted | DBStatus::BeingDeleted => None,
            },
            Some(None) => None,
            None => None,
        }
    }

    fn get_mut(&mut self, id: u64) -> Option<MutDBEntry<T>> {
        match self.entries.get_mut(id as usize) {
            Some(Some((contents, status))) => match status {
                DBStatus::Unchanged | DBStatus::Edited => {
                    *status = DBStatus::Edited;
                    Some(MutDBEntry { id, contents })
                }
                DBStatus::ToBeDeleted | DBStatus::BeingDeleted => None,
            },
            Some(None) => None,
            None => None,
        }
    }

    fn any(&self, mut predicate: impl FnMut(&T) -> bool) -> bool {
        self.entries.iter().any(|entry| match entry {
            None => false,
            Some((item, _)) => predicate(item),
        })
    }

    fn find(&self, mut predicate: impl FnMut(&T) -> bool) -> Option<DBEntry<T>> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, entry)| match entry {
                None => false,
                Some((item, _)) => predicate(item),
            })
            .map(|(index, entry)| DBEntry {
                id: index as u64,
                contents: match entry {
                    None => panic!(),
                    Some((item, _)) => item,
                },
            })
    }

    fn find_mut(&mut self, mut predicate: impl FnMut(&T) -> bool) -> Option<MutDBEntry<T>> {
        self.entries
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| match entry {
                None => false,
                Some((item, _)) => predicate(item),
            })
            .map(|(index, entry)| MutDBEntry {
                id: index as u64,
                contents: match entry {
                    None => panic!(),
                    Some((item, _)) => item,
                },
            })
    }

    fn get_all(&self) -> Vec<DBEntry<T>> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                None => None,
                Some((item, status)) => match status {
                    DBStatus::ToBeDeleted | DBStatus::BeingDeleted => None,
                    DBStatus::Unchanged | DBStatus::Edited => Some(DBEntry {
                        id: index as u64,
                        contents: item,
                    }),
                },
            })
            .collect()
    }

    fn add(&mut self, thing: T) {
        let new_entry = Some((thing, DBStatus::Edited));
        let mut iter = self.entries.iter_mut();
        //iter.next(); // May be necessary if BonsaiDB doesn't support zero-indexing
        match iter.find(|e| e.is_none()) {
            Some(entry) => *entry = new_entry,
            None => self.entries.push(new_entry),
        }
    }

    fn delete(&mut self, id: u64) -> Result<(), ()> {
        match self.entries.get_mut(id as usize) {
            None => Err(()),
            Some(None) => Err(()),
            Some(Some((_, status))) => match status {
                DBStatus::ToBeDeleted | DBStatus::BeingDeleted => Err(()),
                DBStatus::Edited | DBStatus::Unchanged => {
                    *status = DBStatus::ToBeDeleted;
                    Ok(())
                }
            },
        }
    }

    fn delete_all(&mut self) {
        for entry in self.entries.iter_mut().flatten() {
            entry.1 = DBStatus::ToBeDeleted;
        }
    }

    fn extract_changes(&mut self) -> Vec<ClonedDBEntry<T>> {
        let mut changes = Vec::new();
        for (index, entry) in &mut self.entries.iter_mut().enumerate() {
            if let Some((item, status)) = entry {
                if matches!(status, DBStatus::Edited) {
                    changes.push(ClonedDBEntry {
                        id: index as u64,
                        contents: item.clone(),
                    });
                    *status = DBStatus::Unchanged;
                }
            }
        }
        changes
    }

    fn clear_pending_deletions(&mut self) {
        for entry in &mut self.entries {
            if let Some((_, status)) = entry {
                if matches!(status, DBStatus::BeingDeleted) {
                    *entry = None;
                }
            }
        }
    }

    fn extract_deletions(&mut self) -> Vec<u64> {
        let mut deletions = Vec::new();
        for (index, entry) in &mut self.entries.iter_mut().enumerate() {
            if let Some((_, status)) = entry {
                if matches!(status, DBStatus::ToBeDeleted) {
                    deletions.push(index as u64);
                    *status = DBStatus::BeingDeleted;
                }
            }
        }
        deletions
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Partial, Debug, Clone, Serialize, Deserialize)]
#[partially(derive(Debug, Clone, Serialize, Deserialize, Default))]
pub struct Config {
    // Pointcalc
    pub relative_standard_deviation: f64,
    pub points_per_kaffness: u64,
    pub points_per_grade: u64,
    pub points_per_walking_minute: u64,
    pub points_per_stationary_minute: u64,
    // the points p for minutes travelled t are calculated based on the exponent e and the
    // multiplier m using the formula $p = m * (t^e)$.
    pub travel_minutes_exponent: f32,
    pub travel_minutes_multiplier: f32,
    pub points_for_zoneable: u64,
    pub zkaff_points_for_dead_end: u64,
    // points p for station distance s (in metres) are calculated based on the divisor d using the
    // formula $p = \frac{s}{d}$.
    pub zkaff_station_distance_divisor: u64,
    pub zkaff_points_per_minute_to_hb: u64,
    // points p for departures d (per hour) are calculated based on the exponent e, the base b and
    // the multiplier m using the formula $p = m(b-d^e)$.
    pub zkaff_departures_exponent: f32,
    pub zkaff_departures_base: f32,
    pub zkaff_departures_multiplier: f32,

    // underdog system
    pub underdog_starting_difference: u64,
    pub underdog_multiplyer_per_1000: f32,

    // perimeter system
    pub perim_max_kaff: u64,
    pub perim_distance_range: std::ops::Range<u64>,

    // distances
    pub normal_period_near_distance_range: std::ops::Range<u64>,
    pub normal_period_far_distance_range: std::ops::Range<u64>,

    // Zonenkaff
    pub points_per_connected_zone_less_than_6: u64,
    pub points_per_bad_connectivity_index: u64,
    pub points_for_no_train: u64,
    pub points_for_mongus: u64,

    // challenge generation
    pub centre_zone: u64,
    pub challenge_sets: Vec<u64>,
    pub num_challenges: u64,
    pub regio_ratio: f64,
    pub zkaff_ratio_range: std::ops::Range<f64>,

    // miscellaneous game options
    pub num_catchers: u64,
    pub start_zone: u64,
    pub grace_period_duration: chrono::TimeDelta,

    // Bounty system
    pub bounty_base_points: u64,
    pub bounty_start_points: u64,
    pub bounty_percentage: f64,

    // Times
    pub start_time: chrono::NaiveTime,
    pub end_time: chrono::NaiveTime,
    pub specific_minutes: u64,
    pub perimeter_minutes: u64,
    pub zkaff_minutes: u64,
    pub end_game_minutes: u64,
    pub time_wiggle_minutes: u64,

    // Fallback Defaults
    pub default_challenge_title: String,
    pub default_challenge_description: String,

    // additional options
    pub team_colours: Vec<Colour>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            relative_standard_deviation: 0.05,
            points_per_kaffness: 90,
            points_per_grade: 20,
            points_per_walking_minute: 10,
            points_per_stationary_minute: 10,
            travel_minutes_exponent: 1.4,
            travel_minutes_multiplier: 3.0,
            points_for_zoneable: 100,
            zkaff_points_for_dead_end: 50,
            zkaff_station_distance_divisor: 20,
            zkaff_points_per_minute_to_hb: 5,
            zkaff_departures_exponent: 1.0 / 3.0,
            zkaff_departures_multiplier: 32.0,
            zkaff_departures_base: 7.0,
            underdog_starting_difference: 1000,
            underdog_multiplyer_per_1000: 0.25,
            perim_max_kaff: 4,
            perim_distance_range: 20..61,
            normal_period_near_distance_range: 0..26,
            normal_period_far_distance_range: 40..71,
            points_per_bad_connectivity_index: 25,
            points_per_connected_zone_less_than_6: 15,
            points_for_no_train: 30,
            points_for_mongus: 50,
            centre_zone: 110,
            challenge_sets: Vec::new(),
            regio_ratio: 0.4,
            zkaff_ratio_range: 0.2..1.2,
            num_catchers: 3,
            num_challenges: 3,
            start_zone: 110,
            grace_period_duration: chrono::TimeDelta::minutes(15),
            bounty_base_points: 100,
            bounty_start_points: 250,
            bounty_percentage: 0.25,
            start_time: chrono::NaiveTime::from_hms_opt(9, 0, 0)
                .expect("This is hardcoded and should never fail"),
            end_time: chrono::NaiveTime::from_hms_opt(17, 0, 0)
                .expect("This is hardcoded and should never fail"),
            specific_minutes: 15,
            perimeter_minutes: 90,
            zkaff_minutes: 90,
            end_game_minutes: 30,
            time_wiggle_minutes: 7,
            default_challenge_title: "[Kreative Titel]".into(),
            default_challenge_description:
                "Ihr hend Päch, die Challenge isch unlösbar. Ihr müend e anderi uswähle.".into(),
            team_colours: vec![
                Colour {
                    r: 93,
                    g: 156,
                    b: 236,
                },
                Colour {
                    r: 79,
                    g: 193,
                    b: 233,
                },
                Colour {
                    r: 72,
                    g: 207,
                    b: 173,
                },
                Colour {
                    r: 160,
                    g: 212,
                    b: 104,
                },
                Colour {
                    r: 255,
                    g: 206,
                    b: 84,
                },
                Colour {
                    r: 252,
                    g: 110,
                    b: 81,
                },
                Colour {
                    r: 237,
                    g: 85,
                    b: 101,
                },
                Colour {
                    r: 171,
                    g: 146,
                    b: 236,
                },
                Colour {
                    r: 236,
                    g: 135,
                    b: 192,
                },
            ],
        }
    }
}

#[derive(Schema)]
#[schema(name="engine", collections=[Session, PlayerEntry, ChallengeEntry, ZoneEntry, PastGame, PictureEntry, ChallengeSetEntry])]
struct EngineSchema {}

#[derive(Debug, Collection, Serialize, Deserialize, Clone)]
#[collection(name = "picture")]
enum PictureEntry {
    Profile { small: Picture, large: Picture },
    ChallengePicture(Picture),
}

#[allow(dead_code)]
impl PictureEntry {
    fn new_profile(image: image::DynamicImage) -> Result<Self, image::ImageError> {
        let (x, y, width, height) = if image.width() > image.height() {
            (
                (image.width() - image.height()) / 2,
                0,
                image.height(),
                image.height(),
            )
        } else {
            (
                0,
                (image.height() - image.width()) / 2,
                image.width(),
                image.width(),
            )
        };
        let image = image.crop_imm(x, y, width, height);

        let small = image.resize(128, 128, FilterType::CatmullRom);
        let large = image.resize(512, 512, FilterType::CatmullRom);

        Ok(Self::Profile {
            small: small.try_into()?,
            large: large.try_into()?,
        })
    }

    fn new_challenge_picture(image: image::DynamicImage) -> Result<Self, image::ImageError> {
        Ok(Self::ChallengePicture(image.try_into()?))
    }
}

#[derive(Debug, Collection, Serialize, Deserialize, Clone)]
#[collection(name = "player")]
pub struct PlayerEntry {
    name: String,
    passphrase: String,
    discord_id: Option<u64>,
    session: Option<u64>,
}

impl PlayerEntry {
    pub fn to_sendable(&self, id: u64) -> truinlag::Player {
        truinlag::Player {
            name: self.name.clone(),
            session: self.session,
            id,
        }
    }
}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "zone")]
pub struct ZoneEntry {
    pub zone: u64,
    pub num_conn_zones: u64,
    pub num_connections: u64,
    pub train_through: bool,
    pub mongus: bool,
    pub s_bahn_zone: bool,
    pub minutes_to: HashMap<u64, u64>,
}

impl ZoneEntry {
    pub fn to_sendable(&self, id: u64) -> Zone {
        Zone {
            zone: self.zone,
            num_conn_zones: self.num_conn_zones,
            num_connections: self.num_connections,
            train_through: self.train_through,
            mongus: self.mongus,
            s_bahn_zone: self.s_bahn_zone,
            minutes_to: self.minutes_to.clone(),
            id,
        }
    }

    pub fn zonic_kaffness(&self, config: &Config) -> u64 {
        let zonic_kaffness = ((6_f64 - self.num_conn_zones as f64)
            * config.points_per_connected_zone_less_than_6 as f64
            + (6_f64 - (self.num_connections as f64).sqrt())
                * config.points_per_bad_connectivity_index as f64
            + if self.train_through {
                0_f64
            } else {
                config.points_for_no_train as f64
            }
            + if self.mongus {
                config.points_for_mongus as f64
            } else {
                0_f64
            })
        .floor();
        if zonic_kaffness > 0_f64 {
            zonic_kaffness as u64
        } else {
            eprintln!("Engine: zonic kaffness for zone {} <= 0", self.zone);
            0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InGame {
    name: String,
    start_time: chrono::DateTime<chrono::Local>,
    mode: Mode,
    #[serde(default)]
    timer: TimerHook,
}

impl InGame {
    pub fn to_sendable(&self) -> truinlag::Game {
        truinlag::Game {
            name: self.name.clone(),
            date: self.start_time.date_naive(),
            mode: self.mode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Collection)]
#[collection(name = "past game")]
struct PastGame {
    name: String,
    start_time: chrono::DateTime<chrono::Local>,
    end_time: chrono::DateTime<chrono::Local>,
    mode: Mode,
    teams: Vec<PastTeam>,
}

impl PastGame {
    fn new(game: InGame, teams: Vec<TeamEntry>, end_time: chrono::DateTime<chrono::Local>) -> Self {
        Self {
            name: game.name,
            start_time: game.start_time,
            end_time,
            mode: game.mode,
            teams: teams.iter().cloned().map(|t| t.into()).collect(),
        }
    }

    fn new_now(game: InGame, teams: Vec<TeamEntry>) -> Self {
        Self::new(game, teams, chrono::Local::now())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PastTeam {
    name: String,
    players: Vec<u64>,
    discord_channel: Option<u64>,
    end_role: TeamRole,
    colour: Colour,
    points: u64,
    bounty: u64,
    end_zone_id: u64,
    start_location: Option<(f64, f64)>,
    end_location: Option<(f64, f64)>,
    end_challenges: Vec<InOpenChallenge>,
    periods: Vec<PastPeriod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PastPeriod {
    pub context: PeriodContext,
    pub end_time: chrono::DateTime<chrono::Local>,
    pub end_location: (f64, f64),
}

impl From<TeamEntry> for PastTeam {
    fn from(value: TeamEntry) -> Self {
        let start_location = value.locations.first();
        let start_location = start_location.map(|v| (v.0, v.1));
        let end_location = value.locations.last();
        let end_location = end_location.map(|v| (v.0, v.1));
        Self {
            name: value.name,
            players: value.players,
            discord_channel: value.discord_channel,
            end_role: value.role,
            colour: value.colour,
            points: value.points,
            bounty: value.bounty,
            end_zone_id: value.zone_id,
            start_location,
            end_location,
            end_challenges: value.challenges,
            periods: value
                .periods
                .iter()
                .map(|p| PastPeriod {
                    context: p.context.clone(),
                    end_time: chrono::Local::now().with_time(p.end_time).unwrap(),
                    end_location: value
                        .locations
                        .get(p.position_end_index as usize)
                        .map(|l| (l.0, l.1))
                        .unwrap_or((0.0, 0.0)),
                })
                .collect(),
        }
    }
}
