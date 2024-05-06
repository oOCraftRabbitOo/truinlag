use bonsaidb::core::connection::{Connection, StorageConnection};
use bonsaidb::core::document::{CollectionDocument, Emit};
use bonsaidb::core::schema::{
    Collection, CollectionMapReduce, ReduceResult, Schema, SerializedCollection, SerializedView,
    View, ViewMapResult, ViewMappedValue, ViewSchema,
};
use bonsaidb::local::config::Builder;
use bonsaidb::local::{config, Database, Storage};
use chrono;
use geo::Point;
use partially::Partial;
use rand::prelude::*;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use std::path::Path;
use strsim::normalized_damerau_levenshtein as strcmp;
use truinlag::commands::{
    BroadcastAction, EngineAction, EngineCommand, EngineResponse, Mode, ResponseAction,
};
use truinlag::*;

pub fn vroom(command: EngineCommand) -> EngineResponse {
    EngineResponse {
        response_action: ResponseAction::Success,
        broadcast_action: Some(BroadcastAction::Success),
    }
}

#[derive(Partial, Debug, Clone, Serialize, Deserialize)]
#[partially(derive(Debug, Clone, Serialize, Deserialize, Default))]
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

    // Number of active challenges per team
    num_challenges: u64,

    // Bounty system
    bounty_base_points: u64,
    bounty_start_points: u64,
    bounty_percentage: f64,

    // Times
    unspecific_time: chrono::NaiveTime,
    specific_minutes: u64,

    // Fallback Defaults
    default_challenge_title: String,
    default_challenge_description: String,

    // additional options
    team_colours: Vec<Colour>,
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
            num_challenges: 3,
            bounty_base_points: 100,
            bounty_start_points: 250,
            bounty_percentage: 0.25,
            unspecific_time: chrono::NaiveTime::from_hms_opt(16, 0, 0).unwrap(),
            specific_minutes: 15,
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
#[schema(name="engine", collections=[Session, Player, ChallengeEntry, ZoneEntry])]
struct EngineSchema {}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "session")]
struct Session {
    name: String,
    teams: Vec<Team>,
    mode: Mode,
    config: PartialConfig,
    discord_server_id: Option<u64>,
    discord_game_channel: Option<u64>,
    discord_admin_channel: Option<u64>,
    game: Option<Game>,
    past_games: Vec<PastGame>,
}

#[derive(Debug, Collection, Serialize, Deserialize)]
#[collection(name = "player")]
struct Player {
    name: String,
    passphrase: String,
    discord_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
enum ChallengeType {
    Kaff,
    Ortsspezifisch,
    Regionsspezifisch,
    Unspezifisch,
    Zoneable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
enum RandomPlaceType {
    Zone,
    SBahnZone,
}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "challenge", views = [UnspecificChallengeEntries, SpecificChallengeEntries, GoodChallengeEntries])]
struct ChallengeEntry {
    kind: ChallengeType,
    kaff: Option<String>,
    title: Option<String>,
    description: Option<String>,
    kaffskala: Option<u64>,
    grade: Option<u64>,
    random_place: Option<RandomPlaceType>,
    points: i64,
    repetitions: std::ops::Range<u64>,
    points_per_rep: i64,
    fixed: bool,
    zones: Vec<u64>,
    walking_time: i64,
    stationary_time: i64,
    action: Option<ChallengeActionEntry>,
    active: bool,
    accepted: bool,
    last_edit: chrono::DateTime<chrono::Local>,
    comment: String,
}

impl ChallengeEntry {
    async fn challenge(
        &self,
        config: &Config,
        zone_zoneables: bool,
        db: &Database,
    ) -> Option<OpenChallenge> {
        // TODO: if zoneable and zone specified do something to let me know kthxbye
        let mut points = 0;
        points += self.points;
        if let Some(kaffskala) = self.kaffskala {
            points += kaffskala as i64 * config.points_per_kaffness as i64;
        }
        if let Some(grade) = self.grade {
            points += grade as i64 * config.points_per_grade as i64;
        }
        points += self.walking_time as i64 * config.points_per_walking_minute as i64;
        points += self.stationary_time as i64 * config.points_per_stationary_minute as i64;
        let reps = self
            .repetitions
            .clone()
            .choose(&mut thread_rng())
            .unwrap_or(0);
        points += reps as i64 * self.points_per_rep as i64;
        let mut zone_entries = vec![];
        let initial_zones = self.zones.clone();
        for zone in initial_zones {
            match db
                .view::<ZonesByZone>()
                .with_key(&zone)
                .query_with_collection_docs()
            {
                Ok(zones) => {
                    if zones.len() == 0 {
                        eprintln!(
                            "Engine: Couldn't find zone {} in database, skipping Zonenkaff and granting 0 points",
                            zone
                        );
                    } else if zones.len() > 1 {
                        let entry = zones.get(0).unwrap();
                        eprintln!(
                            "Engine: Found {} entries for zone {} in database, using the entry with id {} for Zonenkaff",
                            zones.len(),
                            zone,
                            entry.document.header.id
                            );
                        zone_entries.push(entry.document.clone())
                    } else {
                        zone_entries.push(zones.get(0).unwrap().document.clone())
                    }
                }
                Err(err) => {
                    eprintln!(
                        "Engine: Couldn't query database for zone {}, ignoring zone: {}",
                        zone, err
                    )
                }
            }
        }
        if zone_zoneables && matches!(self.kind, ChallengeType::Zoneable) {
            match ZoneEntry::all(db).query() {
                Ok(entries) => {
                    zone_entries = vec![entries.iter().choose(&mut thread_rng()).unwrap().clone()]
                }
                Err(err) => {
                    eprintln!("Engine: Couldn't retreive zones from database while selecting random zone for zoneable, skipping step: {}", err)
                }
            }
        }
        if let Some(place_type) = &self.random_place {
            match place_type{RandomPlaceType::Zone=>{match ZoneEntry::all(db).query(){Ok(entries)=>zone_entries=vec![entries.iter().choose(&mut thread_rng()).unwrap().clone()],Err(err)=>eprintln!("Engine: Couldn't retrieve zones from database while choosing random zone, skipping step: {}",err),}}RandomPlaceType::SBahnZone=>{match db.view::<ZonesBySBahn>().with_key(&true).query_with_collection_docs(){Ok(entries)=>zone_entries=vec![entries.documents.values().choose(&mut thread_rng()).expect("no s-bahn zones found in database").clone()],Err(err)=>eprintln!("Engine: Couldn't retrieve s-bahn zones from database while choosing random s-bahn zone, skipping step: {}",err),}}}
        }
        let (zone, z_points) = zone_entries.iter().fold((None, 0), |acc, z| {
            if acc.1 == 0 || acc.1 > z.contents.zonic_kaffness(config) {
                (Some(z), z.contents.zonic_kaffness(config))
            } else {
                acc
            }
        });
        points += z_points as i64;
        if !self.fixed {
            points += Normal::new(0_f64, points as f64 * config.relative_standard_deviation)
                .unwrap()
                .sample(&mut thread_rng())
                .round() as i64
        }

        let mut title = None;
        if let Some(kaff) = &self.kaff {
            title = Some(format!("Usflug Uf {}", kaff))
        }
        if let Some(title_override) = &self.title {
            title = Some(title_override.clone())
        }
        if let Some(_) = self.random_place {
            if let Some(t) = &mut title {
                *t = t.replace("%p", &zone.unwrap().contents.zone.to_string())
            }
        }
        if let Some(t) = &mut title {
            *t = t.replace("%r", &reps.to_string());
        }

        let mut description = None;
        if let Some(kaff) = &self.kaff {
            description = Some(format!("Gönd nach {}.", kaff))
        }
        if let Some(description_override) = &self.description {
            description = Some(description_override.clone())
        }
        if let Some(_) = self.random_place {
            if let Some(d) = &mut description {
                *d = d.replace("%p", &zone.unwrap().contents.zone.to_string())
            }
        }
        if let Some(d) = &mut description {
            *d = d.replace("%r", &reps.to_string());
        }

        let zone = match zone {
            Some(z) => Some(z.header.id),
            None => None,
        };

        let mut action = None;
        if let Some(action_entry) = &self.action {
            action = Some(match action_entry {
                ChallengeActionEntry::Trap {
                    stuck_minutes: min,
                    catcher_message,
                } => {
                    let stuck_minutes = match min {
                        Some(minutes) => *minutes,
                        None => reps,
                    };
                    ChallengeAction::Trap {
                        completable_after: chrono::Local::now()
                            + chrono::Duration::minutes(min.unwrap_or(reps) as i64),
                        catcher_message: catcher_message.clone(),
                    }
                }
                ChallengeActionEntry::UncompletableMinutes(minutes) => {
                    ChallengeAction::UncompletableMinutes(
                        chrono::Local::now()
                            + chrono::Duration::minutes(minutes.unwrap_or(reps) as i64),
                    )
                }
            });
        }

        Some(OpenChallenge {
            title: title.unwrap_or(config.default_challenge_title.clone()),
            description: description.unwrap_or(config.default_challenge_description.clone()),
            points: points as u64,
            action,
            zone,
        })
    }
}

#[derive(Debug, Clone, View, ViewSchema)]
#[view(collection = ChallengeEntry, key = bool, value = u64, name = "good")]
struct GoodChallengeEntries;

impl CollectionMapReduce for GoodChallengeEntries {
    fn map<'doc>(
        &self,
        document: CollectionDocument<ChallengeEntry>,
    ) -> ViewMapResult<'doc, Self::View> {
        document
            .header
            .emit_key_and_value(document.contents.active && document.contents.accepted, 1)
    }

    fn reduce(
        &self,
        mappings: &[ViewMappedValue<Self>],
        _rereduce: bool,
    ) -> ReduceResult<Self::View> {
        Ok(mappings.iter().map(|m| m.value).sum())
    }
}

#[derive(Debug, Clone, View, ViewSchema)]
#[view(collection = ChallengeEntry, key = bool, value = u64, name = "unspecific")]
struct UnspecificChallengeEntries;

impl CollectionMapReduce for UnspecificChallengeEntries {
    fn map<'doc>(
        &self,
        document: CollectionDocument<ChallengeEntry>,
    ) -> ViewMapResult<'doc, Self::View> {
        document.header.emit_key_and_value(
            match document.contents.kind {
                ChallengeType::Kaff => false,
                ChallengeType::Zoneable => true,
                ChallengeType::Unspezifisch => true,
                ChallengeType::Ortsspezifisch => false,
                ChallengeType::Regionsspezifisch => true,
            } && document.contents.accepted
                && document.contents.active,
            1,
        )
    }

    fn reduce(
        &self,
        mappings: &[ViewMappedValue<Self>],
        _rereduce: bool,
    ) -> ReduceResult<Self::View> {
        Ok(mappings.iter().map(|m| m.value).sum())
    }
}

#[derive(Debug, Clone, View, ViewSchema)]
#[view(collection = ChallengeEntry, key = bool, value = u64, name = "specific")]
struct SpecificChallengeEntries;

impl CollectionMapReduce for SpecificChallengeEntries {
    fn map<'doc>(
        &self,
        document: CollectionDocument<ChallengeEntry>,
    ) -> ViewMapResult<'doc, Self::View> {
        document.header.emit_key_and_value(
            match document.contents.kind {
                ChallengeType::Kaff => true,
                ChallengeType::Zoneable => true,
                ChallengeType::Unspezifisch => false,
                ChallengeType::Ortsspezifisch => true,
                ChallengeType::Regionsspezifisch => false,
            } && document.contents.active
                && document.contents.accepted,
            1,
        )
    }

    fn reduce(
        &self,
        mappings: &[ViewMappedValue<Self>],
        _rereduce: bool,
    ) -> ReduceResult<Self::View> {
        Ok(mappings.iter().map(|m| m.value).sum())
    }
}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "zone", views = [ZonesByZone, ZonesBySBahn])]
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

#[derive(Debug, Clone, View, ViewSchema)]
#[view(collection = ZoneEntry, key = bool, value = u64, name = "by-sbahn")]
struct ZonesBySBahn;

impl CollectionMapReduce for ZonesBySBahn {
    fn map<'doc>(
        &self,
        document: CollectionDocument<ZoneEntry>,
    ) -> ViewMapResult<'doc, Self::View> {
        document
            .header
            .emit_key_and_value(document.contents.s_bahn_zone, 1)
    }

    fn reduce(
        &self,
        mappings: &[ViewMappedValue<Self>],
        _rereduce: bool,
    ) -> ReduceResult<Self::View> {
        Ok(mappings.iter().map(|m| m.value).sum())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Team {
    name: String,
    players: Vec<u64>,
    discord_channel: Option<u64>,
    role: TeamRole,
    colour: Colour,
    challenges: Vec<OpenChallenge>,
    completed_challenges: Vec<CompletedChallenge>,
    catcher_periods: Vec<CatcherPeriod>,
    caught_periods: Vec<CaughtPeriod>,
    trophy_periods: Vec<TrophyPeriod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrophyPeriod {
    trophies: u64,
    points_spent: u64,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaughtPeriod {
    catcher_team: u64,
    bounty: u64,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatcherPeriod {
    caught_team: u64,
    bounty: u64,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletedChallenge {
    name: String,
    description: String,
    zone: Option<u64>,
    points: u64,
    photo: truinlag::Jpeg,
    time: chrono::NaiveTime,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum TeamRole {
    Runner,
    Catcher,
}

impl Team {
    fn new(name: String, players: Vec<u64>, discord_channel: Option<u64>, colour: Colour) -> Self {
        Team {
            name,
            players,
            discord_channel,
            colour,
            completed_challenges: Vec::new(),
            challenges: Vec::new(),
            role: TeamRole::Runner,
            catcher_periods: Vec::new(),
            caught_periods: Vec::new(),
            trophy_periods: Vec::new(),
        }
    }
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
            eprintln!("Engine: zonic kaffness for zone {} <= 0", self.zone);
            0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ChallengeActionEntry {
    UncompletableMinutes(Option<u64>), // None -> uses repetitions (%r)
    Trap {
        stuck_minutes: Option<u64>, // None -> uses repetitions (%r)
        catcher_message: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ChallengeAction {
    UncompletableMinutes(chrono::DateTime<chrono::Local>),
    Trap {
        completable_after: chrono::DateTime<chrono::Local>,
        catcher_message: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenChallenge {
    title: String,
    description: String,
    points: u64,
    action: Option<ChallengeAction>,
    zone: Option<u64>, // id for ZoneEntry collection in db
}

impl OpenChallenge {
    fn completable(&self) -> bool {
        match &self.action {
            None => true,
            Some(action) => match action {
                ChallengeAction::UncompletableMinutes(t) => &chrono::Local::now() > t,
                ChallengeAction::Trap {
                    completable_after: t,
                    catcher_message: _,
                } => &chrono::Local::now() > t,
            },
        }
    }
}

impl PartialEq for OpenChallenge {
    fn eq(&self, other: &OpenChallenge) -> bool {
        self.title == other.title
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Game {
    name: String,
    date: chrono::NaiveDate,
    mode: Mode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PastGame {
    name: String,
    date: chrono::NaiveDate,
    mode: Mode,
    challenge_entries: Vec<ChallengeEntry>,
    teams: Vec<Team>,
}

impl Session {
    fn config(&self) -> Config {
        let mut cfg = Config::default();
        cfg.apply_some(self.config.clone());
        cfg
    }
    fn assign(&mut self, team: usize, player: u64) {
        for t in self.teams.iter_mut() {
            t.players = t
                .players
                .iter()
                .filter(|&&p| p != player)
                .map(|&e| e)
                .collect();
        }
        if (team as usize) < self.teams.len() {
            self.teams[team as usize].players.push(player);
        }
    }
    fn vroom(&mut self, command: EngineAction, db: &Database) -> EngineResponse {
        use EngineAction::*;
        match command {
            AddTeam {
                name,
                players,
                discord_channel,
                colour,
            } => {
                if let Some(nom) = self
                    .teams
                    .iter()
                    .map(|t| t.name.clone())
                    .find(|n| strcmp(&n.to_lowercase(), &name.to_lowercase()) >= 0.85)
                {
                    EngineResponse {
                        response_action: ResponseAction::Error(commands::Error::TeamExists(nom)),
                        broadcast_action: None,
                    }
                } else {
                    let colour = match colour {
                        Some(c) => c,
                        None => {
                            match self
                                .config()
                                .team_colours
                                .iter()
                                .filter(|&&c| self.teams.iter().any(|t| t.colour == c))
                                .next()
                            {
                                Some(&colour) => colour,
                                None => Colour { r: 0, g: 0, b: 0 },
                            }
                        }
                    };
                    self.teams
                        .push(Team::new(name, Vec::new(), discord_channel, colour));
                    for p in players {
                        self.assign(self.teams.len() - 1, p);
                    }
                    EngineResponse {
                        response_action: ResponseAction::Success,
                        broadcast_action: None,
                    }
                }
            }

            Start => match self.game {
                Some(_) => EngineResponse {
                    response_action: ResponseAction::Error(commands::Error::GameInProgress),
                    broadcast_action: None,
                },
                None => {
                    todo!();
                }
            },
            _ => EngineResponse {
                response_action: ResponseAction::Error(commands::Error::SessionSupplied),
                broadcast_action: None,
            },
        }
    }
}

pub struct Engine {
    db: Database,
    // store: Storage, --- Don't need that no more, everything stored in the db
    // sessions: Vec<Session>, --- see above
}

impl Engine {
    pub fn init(storage_path: &Path) -> Self {
        let db = Storage::open(
            config::StorageConfiguration::new(storage_path)
                .with_schema::<EngineSchema>()
                .unwrap(),
        )
        .unwrap()
        .create_database::<EngineSchema>("engine", true)
        .unwrap();

        /*
        let sessions = Session::all(&db)
            .query()
            .unwrap()
            .into_iter()
            .map(|doc| doc.contents.clone().init(&store, db.clone()))
            .collect();
        */

        Engine { db }
    }

    pub fn vroom(&self, command: EngineCommand) -> EngineResponse {
        use EngineAction::*;
        match command.session {
            Some(id) => match Session::get(&id, &self.db) {
                Err(err) => {
                    eprintln!("Couldn't retrieve session from db: {}", err);
                    EngineResponse {
                        response_action: ResponseAction::Error(commands::Error::InternalError),
                        broadcast_action: None,
                    }
                }
                Ok(maybedoc) => match maybedoc {
                    None => EngineResponse {
                        response_action: ResponseAction::Error(commands::Error::NotFound),
                        broadcast_action: None,
                    },
                    Some(mut doc) => {
                        let response = doc.contents.vroom(command.action, &self.db);
                        doc.update(&self.db).unwrap();
                        response
                    }
                },
            },
            None => match command.action {
                AddPlayer {
                    name,
                    discord_id,
                    passphrase,
                } => match Player::all(&self.db).query() {
                    Err(err) => {
                        println!("Couldn't retreive all players from db: {}", err);
                        EngineResponse {
                            response_action: ResponseAction::Error(commands::Error::InternalError),
                            broadcast_action: None,
                        }
                    }
                    Ok(players) => {
                        if let Some(_) = players
                            .iter()
                            .map(|p| &p.contents.passphrase)
                            .find(|&pp| pp == &passphrase)
                        {
                            EngineResponse {
                                response_action: ResponseAction::Error(
                                    commands::Error::AlreadyExists,
                                ),
                                broadcast_action: None,
                            }
                        } else {
                            match (Player {
                                name,
                                discord_id,
                                passphrase,
                            })
                            .push_into(&self.db)
                            {
                                Err(err) => {
                                    println!("Couldn't add player to db: {}", err);
                                    EngineResponse {
                                        response_action: ResponseAction::Error(
                                            commands::Error::InternalError,
                                        ),
                                        broadcast_action: None,
                                    }
                                }
                                Ok(doc) => EngineResponse {
                                    response_action: ResponseAction::AddedPlayer(doc.header.id),
                                    broadcast_action: None,
                                },
                            }
                        }
                    }
                },
                _ => EngineResponse {
                    response_action: ResponseAction::Error(commands::Error::NoSessionSupplied),
                    broadcast_action: None,
                },
            },
        }
    }
}
