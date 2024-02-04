use crate::commands;
use crate::commands::{
    BroadcastAction, EngineAction, EngineCommand, EngineResponse, Mode, ResponseAction,
};
use bonsaidb::core::connection::AsyncStorageConnection;
use bonsaidb::core::document::{CollectionDocument, Emit};
use bonsaidb::core::schema::{
    Collection, CollectionMapReduce, ReduceResult, Schema, SerializedCollection, SerializedView,
    View, ViewMapResult, ViewMappedValue, ViewSchema,
};
use bonsaidb::local::config::Builder;
use bonsaidb::local::{config, AsyncDatabase, AsyncStorage};
use chrono;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use serde_with;
use std::path::Path;

pub fn vroom(command: EngineCommand) -> EngineResponse {
    EngineResponse {
        response_action: ResponseAction::Success,
        broadcast_action: Some(BroadcastAction::Success),
    }
}

#[serde_with::serde_as]
#[derive(Debug, Serialize, Deserialize)]
struct Config {
    // Pointcalc
    relative_standard_deviation: f64,
    points_per_kaffness: u64,
    points_per_grade: u64,
    points_per_walking_minute: u64,
    points_per_stationary_minute: u64,

    // Zonenkaff
    points_per_connected_zone_less_than_6: u64,
    points_per_bad_connectivity_index: u64,
    points_for_no_train: u64,
    points_for_mongus: u64,

    // Number of Catchers
    num_catchers: u64,

    // Bounty system
    bounty_base_points: u64,
    bounty_start_points: u64,
    bounty_percentage: f64,

    // Times
    unspecific_time: chrono::NaiveTime,
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    specific_period: chrono::Duration,
}

struct PartialConfig {
    relative_standard_deviation: Option<f64>,
    points_per_kaffness: Option<u64>,
    points_per_grade: Option<u64>,
    points_per_walking_minute: Option<u64>,
    points_per_stationary_minute: Option<u64>,

    // Zonenkaff
    points_per_connected_zone_less_than_6: Option<u64>,
    points_per_bad_connectivity_index: Option<u64>,
    points_for_no_train: Option<u64>,
    points_for_mongus: Option<u64>,

    // Number of Catchers
    num_catchers: Option<u64>,

    // Bounty system
    bounty_base_points: Option<u64>,
    bounty_start_points: Option<u64>,
    bounty_percentage: Option<f64>,

    // Times
    unspecific_time: Option<chrono::NaiveTime>,
    specific_period: Option<chrono::Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            relative_standard_deviation: 0.05,
            points_per_kaffness: 90,
            points_per_grade: 20,
            points_per_walking_minute: 10,
            points_per_stationary_minute: 10,
            points_per_bad_connectivity_index: 25,
            points_per_connected_zone_less_than_6: 15,
            points_for_no_train: 30,
            points_for_mongus: 50,
            num_catchers: 3,
            bounty_base_points: 100,
            bounty_start_points: 250,
            bounty_percentage: 0.25,
            unspecific_time: chrono::NaiveTime::from_hms_opt(16, 0, 0).unwrap(),
            specific_period: chrono::Duration::minutes(15),
        }
    }
}

impl PartialConfig {
    fn supplement(self, other: Self) -> Self {
        Self {
            relative_standard_deviation: self
                .relative_standard_deviation
                .or(other.relative_standard_deviation),
            points_per_kaffness: self.points_per_kaffness.or(other.points_per_kaffness),
            points_per_grade: self.points_per_grade.or(other.points_per_grade),
            points_per_connected_zone_less_than_6: self
                .points_per_connected_zone_less_than_6
                .or(other.points_per_connected_zone_less_than_6),
            points_per_bad_connectivity_index: self
                .points_per_bad_connectivity_index
                .or(other.points_per_bad_connectivity_index),
            points_for_no_train: self.points_for_no_train.or(other.points_for_no_train),
            points_for_mongus: self.points_for_mongus.or(other.points_for_mongus),
            num_catchers: self.num_catchers.or(other.num_catchers),
            bounty_base_points: self.bounty_base_points.or(other.bounty_base_points),
            bounty_percentage: self.bounty_percentage.or(other.bounty_percentage),
            unspecific_time: self.unspecific_time.or(other.unspecific_time),
            specific_period: self.specific_period.or(other.specific_period),
            bounty_start_points: self.bounty_start_points.or(other.bounty_start_points),
            points_per_stationary_minute: self
                .points_per_stationary_minute
                .or(other.points_per_stationary_minute),
            points_per_walking_minute: self
                .points_per_walking_minute
                .or(other.points_per_walking_minute),
        }
    }

    fn complete(self) -> Config {
        let default = Config::default();
        Config {
            relative_standard_deviation: self
                .relative_standard_deviation
                .unwrap_or(default.relative_standard_deviation),
            points_for_mongus: self.points_for_mongus.unwrap_or(default.points_for_mongus),
            points_for_no_train: self
                .points_for_no_train
                .unwrap_or(default.points_for_no_train),
            points_per_bad_connectivity_index: self
                .points_per_bad_connectivity_index
                .unwrap_or(default.points_per_bad_connectivity_index),
            points_per_connected_zone_less_than_6: self
                .points_per_connected_zone_less_than_6
                .unwrap_or(default.points_per_connected_zone_less_than_6),
            points_per_grade: self.points_per_grade.unwrap_or(default.points_per_grade),
            points_per_kaffness: self
                .points_per_kaffness
                .unwrap_or(default.points_per_kaffness),
            bounty_start_points: self
                .bounty_start_points
                .unwrap_or(default.bounty_start_points),
            bounty_base_points: self
                .bounty_base_points
                .unwrap_or(default.bounty_base_points),
            bounty_percentage: self.bounty_percentage.unwrap_or(default.bounty_percentage),
            specific_period: self.specific_period.unwrap_or(default.specific_period),
            unspecific_time: self.unspecific_time.unwrap_or(default.unspecific_time),
            num_catchers: self.num_catchers.unwrap_or(default.num_catchers),
            points_per_walking_minute: self
                .points_per_walking_minute
                .unwrap_or(default.points_per_walking_minute),
            points_per_stationary_minute: self
                .points_per_stationary_minute
                .unwrap_or(default.points_per_stationary_minute),
        }
    }
}

#[derive(Schema)]
#[schema(name="engine", collections=[SessionEntry])]
struct EngineSchema {}

#[derive(Debug, Collection, Serialize, Deserialize)]
#[collection(name = "session")]
struct SessionEntry {
    name: String,
    mode: Mode,
    discord_game_channel: u64,
    discord_admin_channel: u64,
}

#[derive(Debug, Collection, Serialize, Deserialize)]
#[collection(name = "player")]
struct PlayerEntry {
    name: String,
    discord_id: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
enum ChallengeType {
    Kaff,
    Ortsspezifisch,
    Regionsspezifisch,
    Unspezifisch,
    Zoneable,
}

#[derive(Debug, Collection, Serialize, Deserialize)]
#[collection(name = "challenge")]
struct ChallengeEntry {
    kind: ChallengeType,
    kaff: Option<String>,
    title: Option<String>,
    description: Option<String>,
    kaffskala: u64,
    grade: u64,
    points: i64,
    repetitions: std::ops::Range<u64>,
    points_per_rep: i64,
    fixed: bool,
    zones: Vec<u64>,
    walking_time: i64,
    stationary_time: i64,
    last_edit: chrono::DateTime<chrono::Local>,
    comment: String,
}

#[derive(Debug, Collection, Serialize, Deserialize)]
#[collection(name = "zone", views = [ZonesByZone])]
struct ZoneEntry {
    zone: u64,
    num_conn_zones: u64,
    num_connections: u64,
    train_through: bool,
    mongus: bool,
    s_bahn_zone: bool,
}

#[derive(Debug, Clone, View, ViewSchema)]
#[view(collection = ZoneEntry, key = u64, value = u64, name = "by-zone")]
struct ZonesByZone;

impl CollectionMapReduce for ZonesByZone {
    fn map<'doc>(
        &self,
        document: CollectionDocument<ZoneEntry>,
    ) -> ViewMapResult<'doc, Self::View> {
        document
            .header
            .emit_key_and_value(document.contents.zone, 1)
    }

    fn reduce(
        &self,
        mappings: &[ViewMappedValue<Self>],
        _rereduce: bool,
    ) -> ReduceResult<Self::View> {
        Ok(mappings.iter().map(|m| m.value).sum())
    }
}

#[derive(Schema)]
#[schema(name="session", collections=[])]
struct SessionSchema {}

#[derive(Debug, Collection, Serialize, Deserialize)]
#[collection(name = "team")]
struct TeamEntry {
    name: String,
    players: Vec<u64>,
    discord_channel: Option<u64>,
}

impl ZoneEntry {
    fn zonic_kaffness(&self, config: &Config) -> u64 {
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
            0
        }
    }
}

impl ChallengeEntry {
    fn challenge(&self, config: &Config, zone_zoneables: bool) -> Challenge {
        let mut points = 0;
        points += self.points;
        points += self.kaffskala as i64 * config.points_per_kaffness as i64;
        points += self.grade as i64 * config.points_per_grade as i64;
        points += self.walking_time as i64 * config.points_per_walking_minute as i64;
        points += self.stationary_time as i64 * config.points_per_stationary_minute as i64;
        let reps = self.repetitions.choose(&mut thread_rng()).unwrap_or(0);
        points += reps as i64 * self.points_per_rep as i64;
    }
}

struct Challenge {
    title: String,
    description: String,
    points: u64,
    zone: Option<u64>,
}

struct Session {
    name: String,
    db: AsyncDatabase,
    mode: Mode,
}

pub struct Engine {
    db: AsyncDatabase,
    store: AsyncStorage,
    sessions: Vec<Session>,
}

impl Session {
    async fn init(entry: SessionEntry, store: &AsyncStorage) -> Self {
        let db = store
            .create_database::<SessionSchema>(&entry.name, true)
            .await
            .unwrap();
        Session {
            name: entry.name,
            db,
            mode: entry.mode,
        }
    }

    async fn vroom(&self, command: EngineAction, meta_db: &AsyncDatabase) -> EngineResponse {
        todo!();
    }
}

impl Engine {
    pub async fn init(storage_path: &Path) -> Self {
        let store = AsyncStorage::open(
            config::StorageConfiguration::new(storage_path)
                .with_schema::<EngineSchema>()
                .unwrap()
                .with_schema::<SessionSchema>()
                .unwrap(),
        )
        .await
        .unwrap();
        let db = store
            .create_database::<EngineSchema>("engine", true)
            .await
            .unwrap();

        let sessions = futures::future::join_all(
            SessionEntry::all_async(&db)
                .await
                .unwrap()
                .into_iter()
                .map(|doc| Session::init(doc.contents, &store)),
        )
        .await;

        Engine {
            store,
            db,
            sessions,
        }
    }

    pub async fn vroom(&self, command: EngineCommand) -> EngineResponse {
        match command.session {
            Some(name) => match self.sessions.iter().find(|item| item.name == name) {
                Some(session) => session.vroom(command.action, &self.db).await,
                None => EngineResponse {
                    response_action: ResponseAction::Error(commands::Error::SessionNotFound(name)),
                    broadcast_action: None,
                },
            },
            None => todo!(),
        }
    }
}
