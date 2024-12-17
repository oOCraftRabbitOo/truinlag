use super::runtime::{
    InternEngineCommand, InternEngineResponse, InternEngineResponsePackage, RuntimeRequest,
};
use bonsaidb::{
    core::{
        connection::{Connection, StorageConnection},
        document::{CollectionDocument, Emit, Header},
        key::KeyEncoding,
        schema::{
            Collection, CollectionMapReduce, DefaultSerialization, ReduceResult, Schema,
            SerializedCollection, View, ViewMapResult, ViewMappedValue, ViewSchema,
        },
        transaction::Transaction,
        transmog_pot,
    },
    local::{
        config::{self, Builder},
        Database, Storage,
    },
};
use chrono::{self, NaiveTime};
use image::imageops::FilterType;
use partially::Partial;
use rand::prelude::*;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};
use strsim::normalized_damerau_levenshtein as strcmp;
use tokio::time::{sleep, Duration};
use truinlag::{
    commands::{BroadcastAction, EngineAction, EngineResponse, ResponseAction},
    *,
};

#[derive(Partial, Debug, Clone, Serialize, Deserialize)]
#[partially(derive(Debug, Clone, Serialize, Deserialize, Default))]
struct Config {
    // Pointcalc
    relative_standard_deviation: f64,
    points_per_kaffness: u64,
    points_per_grade: u64,
    points_per_walking_minute: u64,
    points_per_stationary_minute: u64,
    points_per_travel_minute: u64,

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
    start_time: chrono::NaiveTime,
    end_time: chrono::NaiveTime,
    specific_minutes: u64,
    perimeter_minutes: u64,
    zkaff_minutes: u64,
    end_game_minutes: u64,

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
            points_per_travel_minute: 12,
            points_per_bad_connectivity_index: 25,
            points_per_connected_zone_less_than_6: 15,
            points_for_no_train: 30,
            points_for_mongus: 50,
            num_catchers: 3,
            num_challenges: 3,
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
#[schema(name="engine", collections=[Session, PlayerEntry, ChallengeEntry, ZoneEntry, PastGame, PictureEntry])]
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
#[collection(name = "player", views = [PlayersByPassphrase])]
struct PlayerEntry {
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

#[derive(Debug, Clone, Collection, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[collection(name = "set")]
struct ChallengeSetEntry {
    name: String,
}

impl ChallengeSetEntry {
    fn to_sendable(&self, id: u64) -> ChallengeSet {
        ChallengeSet {
            name: self.name.clone(),
            id,
        }
    }
}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "challenge", views = [UnspecificChallengeEntries, SpecificChallengeEntries, GoodChallengeEntries])]
struct ChallengeEntry {
    kind: ChallengeType,
    sets: std::collections::HashSet<u64>,
    status: ChallengeStatus,
    title: Option<String>,
    description: Option<String>,
    random_place: Option<RandomPlaceType>,
    place: Option<String>,
    comment: String,
    kaffskala: Option<u8>,
    grade: Option<u8>,
    zone: Vec<u64>,
    bias_sat: f32,
    bias_sun: f32,
    walking_time: u8,
    stationary_time: u8,
    additional_points: i16,
    repetitions: std::ops::Range<u16>,
    points_per_rep: i16,
    station_distance: u16,
    time_to_hb: u8,
    departures: u8,
    dead_end: bool,
    no_disembark: bool,
    fixed: bool,
    in_perimeter_override: Option<bool>,
    translated_titles: HashMap<String, String>,
    translated_descriptions: HashMap<String, String>,
    action: Option<ChallengeActionEntry>,
    last_edit: chrono::DateTime<chrono::Local>,
}

impl ChallengeEntry {
    fn to_sendable(
        &self,
        id: u64,
        challenge_sets: &[DBEntry<ChallengeSetEntry>],
        zone_entries: &[DBEntry<ZoneEntry>],
    ) -> Result<RawChallenge, commands::Error> {
        Ok(RawChallenge {
            kind: self.kind,
            sets: {
                let mut sets = std::collections::HashSet::new();
                for s in self.sets.clone() {
                    sets.insert({
                        let set = challenge_sets.iter().find(|c| c.id == s).ok_or_else(|| {
                            eprintln!("Couldn't find ChallengeSet with id {} in db while making challenge with id {} sendable, maybe it was improperly removed?", s, id);
                            commands::Error::InternalError})?; set.contents.to_sendable(set.id)});
                }
                sets
            },
            status: self.status,
            title: self.title.clone(),
            description: self.description.clone(),
            random_place: self.random_place,
            place: self.place.clone(),
            comment: self.comment.clone(),
            kaffskala: self.kaffskala,
            grade: self.grade,
            zone: {
                let mut zones = Vec::new();
                for s in self.zone.clone() {
                    zones.push({
                        let set = zone_entries.iter().find(|z| z.id == s).ok_or_else(|| {
                            eprintln!("Couldn't find ChallengeSet with id {} in db, maybe it was improperly removed?", s);
                            commands::Error::InternalError})?; set.contents.to_sendable(set.id)});
                }
                zones
            },
            bias_sat: self.bias_sat,
            bias_sun: self.bias_sun,
            walking_time: self.walking_time,
            stationary_time: self.stationary_time,
            additional_points: self.additional_points,
            repetitions: self.repetitions.clone(),
            points_per_rep: self.points_per_rep,
            station_distance: self.station_distance,
            time_to_hb: self.time_to_hb,
            departures: self.departures,
            dead_end: self.dead_end,
            no_disembark: self.no_disembark,
            fixed: self.fixed,
            in_perimeter_override: self.in_perimeter_override,
            action: self.action.clone(),
            translated_titles: self.translated_titles.clone(),
            translated_descriptions: self.translated_descriptions.clone(),
            last_edit: self.last_edit,
            id: Some(id),
        })
    }

    #[allow(dead_code)]
    async fn challenge(
        &self,
        config: &Config,
        zone_zoneables: bool,
        db: &Database,
    ) -> Option<InOpenChallenge> {
        // TODO: if zoneable and zone specified do something to let me know kthxbye
        let mut points = 0_i64;
        points += self.additional_points as i64;
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
        let initial_zones = self.zone.clone();
        for zone in initial_zones {
            match db
                .view::<ZonesByZone>()
                .with_key(&zone)
                .query_with_collection_docs()
            {
                Ok(zones) => {
                    if zones.is_empty() {
                        eprintln!(
                            "Engine: Couldn't find zone {} in database, skipping Zonenkaff and granting 0 points",
                            zone
                        );
                    } else if zones.len() > 1 {
                        let entry = zones
                            .get(0)
                            .expect("This should never fail, since zones.len() > 1");
                        eprintln!(
                            "Engine: Found {} entries for zone {} in database, using the entry with id {} for Zonenkaff",
                            zones.len(),
                            zone,
                            entry.document.header.id
                            );
                        zone_entries.push(entry.document.clone())
                    } else {
                        zone_entries.push(
                            zones
                                .get(0)
                                .expect("This should never even be called")
                                .document
                                .clone(),
                        )
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
                    zone_entries = vec![entries
                        .iter()
                        .choose(&mut thread_rng())
                        .expect("There are probably no ZoneEntries")
                        .clone()]
                }
                Err(err) => {
                    eprintln!("Engine: Couldn't retreive zones from database while selecting random zone for zoneable, skipping step: {}", err)
                }
            }
        }
        if let Some(place_type) = &self.random_place {
            match place_type{RandomPlaceType::Zone=>{match ZoneEntry::all(db).query(){Ok(entries)=>zone_entries=vec![entries.iter().choose(&mut thread_rng()).expect("There are probably no ZoneEntries").clone()],Err(err)=>eprintln!("Engine: Couldn't retrieve zones from database while choosing random zone, skipping step: {}",err),}}RandomPlaceType::SBahnZone=>{match db.view::<ZonesBySBahn>().with_key(&true).query_with_collection_docs(){Ok(entries)=>zone_entries=vec![entries.documents.values().choose(&mut thread_rng()).expect("no s-bahn zones found in database").clone()],Err(err)=>eprintln!("Engine: Couldn't retrieve s-bahn zones from database while choosing random s-bahn zone, skipping step: {}",err),}}}
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
                .expect("This should't fail if the challenge points and the relative_standard_deviation have reasonable values")
                .sample(&mut thread_rng())
                .round() as i64
        }

        let mut title = None;
        if let Some(kaff) = &self.place {
            title = Some(format!("Usflug Uf {}", kaff))
        }
        if let Some(title_override) = &self.title {
            title = Some(title_override.clone())
        }
        if self.random_place.is_some() {
            if let Some(t) = &mut title {
                *t = t.replace("%p", &zone.expect("This should never fail, because it should only run if there is exactly 1 zone_entry").contents.zone.to_string())
            }
        }
        if let Some(t) = &mut title {
            *t = t.replace("%r", &reps.to_string());
        }

        let mut description = None;
        if let Some(kaff) = &self.place {
            description = Some(format!("Gönd nach {}.", kaff))
        }
        if let Some(description_override) = &self.description {
            description = Some(description_override.clone())
        }
        if self.random_place.is_some() {
            if let Some(d) = &mut description {
                *d = d.replace("%p", &zone.expect("This should never fail, because it should only run if there is exactly 1 zone_entry").contents.zone.to_string())
            }
        }
        if let Some(d) = &mut description {
            *d = d.replace("%r", &reps.to_string());
        }

        let zone = zone.map(|z| z.header.id);

        let mut action = None;
        if let Some(action_entry) = &self.action {
            action = Some(match action_entry {
                ChallengeActionEntry::Trap {
                    stuck_minutes: min,
                    catcher_message,
                } => ChallengeAction::Trap {
                    completable_after: chrono::Local::now()
                        + chrono::Duration::minutes(min.unwrap_or(reps as u64) as i64),
                    catcher_message: catcher_message.clone(),
                },
                ChallengeActionEntry::UncompletableMinutes(minutes) => {
                    ChallengeAction::UncompletableMinutes(
                        chrono::Local::now()
                            + chrono::Duration::minutes(minutes.unwrap_or(reps as u64) as i64),
                    )
                }
            });
        }

        Some(InOpenChallenge {
            title: title.unwrap_or(config.default_challenge_title.clone()),
            description: description.unwrap_or(config.default_challenge_description.clone()),
            points: points as u64,
            action,
            zone,
        })
    }
}

impl From<RawChallenge> for ChallengeEntry {
    fn from(v: RawChallenge) -> Self {
        ChallengeEntry {
            kind: v.kind,
            sets: v.sets.iter().map(|s| s.id).collect(),
            status: v.status,
            title: v.title,
            description: v.description,
            random_place: v.random_place,
            place: v.place,
            comment: v.comment,
            kaffskala: v.kaffskala,
            grade: v.grade,
            zone: v.zone.iter().map(|z| z.id).collect(),
            bias_sat: v.bias_sat,
            bias_sun: v.bias_sun,
            walking_time: v.walking_time,
            stationary_time: v.stationary_time,
            additional_points: v.additional_points,
            repetitions: v.repetitions,
            points_per_rep: v.points_per_rep,
            station_distance: v.station_distance,
            time_to_hb: v.time_to_hb,
            departures: v.departures,
            dead_end: v.dead_end,
            no_disembark: v.no_disembark,
            fixed: v.fixed,
            in_perimeter_override: v.in_perimeter_override,
            translated_titles: v.translated_titles,
            translated_descriptions: v.translated_descriptions,
            action: v.action,
            last_edit: v.last_edit,
        }
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
            .emit_key_and_value(document.contents.status == ChallengeStatus::Approved, 1)
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
            } && document.contents.status == ChallengeStatus::Approved,
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
            } && document.contents.status == ChallengeStatus::Approved,
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
    minutes_to: HashMap<u64, u64>,
}

impl ZoneEntry {
    fn to_sendable(&self, id: u64) -> Zone {
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

#[derive(Debug, Clone, View, ViewSchema)]
#[view(collection = PlayerEntry, key = String, value = u64, name = "by-passphrase")]
struct PlayersByPassphrase;

impl CollectionMapReduce for PlayersByPassphrase {
    fn map<'doc>(
        &self,
        document: CollectionDocument<PlayerEntry>,
    ) -> ViewMapResult<'doc, Self::View> {
        document
            .header
            .emit_key_and_value(document.contents.passphrase, 1)
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
pub struct TeamEntry {
    pub name: String,
    pub players: Vec<u64>,
    pub discord_channel: Option<u64>,
    pub role: TeamRole,
    pub colour: Colour,
    pub points: u64,
    pub bounty: u64,
    pub locations: Vec<(f64, f64, NaiveTime)>,
    pub challenges: Vec<InOpenChallenge>,
    pub completed_challenges: Vec<ChompletedChallengePeriod>,
    pub catcher_periods: Vec<CatcherPeriod>,
    pub caught_periods: Vec<CaughtPeriod>,
    pub trophy_periods: Vec<TrophyPeriod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrophyPeriod {
    trophies: u64,
    points_spent: u64,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaughtPeriod {
    catcher_team: u64,
    bounty: u64,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatcherPeriod {
    caught_team: u64,
    bounty: u64,
    position_start_index: u64,
    position_end_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChompletedChallengePeriod {
    title: String,
    description: String,
    zone: Option<u64>,
    points: u64,
    photo: u64,
    time: chrono::NaiveTime,
    position_start_index: u64,
    position_end_index: u64,
}

impl ChompletedChallengePeriod {
    pub fn to_sendable(&self) -> truinlag::CompletedChallenge {
        truinlag::CompletedChallenge {
            title: self.title.clone(),
            description: self.description.clone(),
            points: self.points,
            time: self.time,
        }
    }
}

impl TeamEntry {
    fn new(name: String, players: Vec<u64>, discord_channel: Option<u64>, colour: Colour) -> Self {
        TeamEntry {
            name,
            players,
            discord_channel,
            colour,
            completed_challenges: Vec::new(),
            challenges: Vec::new(),
            role: TeamRole::Runner,
            points: 0,
            bounty: 0,
            locations: Vec::new(),
            catcher_periods: Vec::new(),
            caught_periods: Vec::new(),
            trophy_periods: Vec::new(),
        }
    }

    fn to_sendable(&self, player_entries: &[DBEntry<PlayerEntry>], index: usize) -> truinlag::Team {
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
                        .iter()
                        .find(|pp| &pp.id == p)
                        .expect("PlayerEntry not found in db while making team sendable")
                        .contents
                        .to_sendable(*p)
                })
                .collect(),
            challenges: self.challenges.iter().map(|c| c.to_sendable()).collect(),
            completed_challenges: self
                .completed_challenges
                .iter()
                .map(|c| c.to_sendable())
                .collect(),
            location: if !self.locations.is_empty() {
                Some((self.locations[0].0, self.locations[0].1))
            } else {
                None
            },
        }
    }
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
pub struct InOpenChallenge {
    title: String,
    description: String,
    points: u64,
    action: Option<ChallengeAction>,
    zone: Option<u64>, // id for ZoneEntry collection in db
}

impl InOpenChallenge {
    #[allow(dead_code)]
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

    pub fn to_sendable(&self) -> truinlag::Challenge {
        truinlag::Challenge {
            title: self.title.clone(),
            points: self.points,
            description: self.description.clone(),
        }
    }
}

impl PartialEq for InOpenChallenge {
    fn eq(&self, other: &InOpenChallenge) -> bool {
        self.title == other.title
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InGame {
    name: String,
    date: chrono::NaiveDate,
    mode: Mode,
}

impl InGame {
    pub fn to_sendable(&self) -> truinlag::Game {
        truinlag::Game {
            name: self.name.clone(),
            date: self.date,
            mode: self.mode,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Collection)]
#[collection(name = "past game")]
struct PastGame {
    name: String,
    date: chrono::NaiveDate,
    mode: Mode,
    challenge_entries: Vec<ChallengeEntry>,
    teams: Vec<TeamEntry>,
}

fn add_into<T>(collection: &mut Vec<DBEntry<T>>, item: T)
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
{
    collection.push(DBEntry {
        id: match collection.iter().max_by(|x, y| x.id.cmp(&y.id)) {
            None => 1,
            Some(max_item) => max_item.id + 1,
        },
        contents: item,
    })
}

#[allow(dead_code)]
fn get_from_db<T, F, I, Cn>(connection: &Cn, id: I, on_success: F) -> InternEngineResponse
where
    T: SerializedCollection,
    F: Fn(CollectionDocument<T>) -> InternEngineResponse,
    I: KeyEncoding<T::PrimaryKey> + std::fmt::Display,
    Cn: Connection,
{
    match T::get(&id, connection) {
        Ok(db_response) => match db_response {
            Some(collection_doc) => on_success(collection_doc),
            None => ResponseAction::Error(commands::Error::NotFound).into(),
        },
        Err(err) => {
            eprintln!(
                "Engine: Couldn't get {} with id {} from db: {}",
                std::any::type_name::<T>(),
                id,
                err
            );
            ResponseAction::Error(commands::Error::InternalError).into()
        }
    }
}

#[allow(dead_code)]
fn get_all_from_db<T, F, Cn>(connection: &Cn, on_success: F) -> InternEngineResponse
where
    T: SerializedCollection,
    F: Fn(Vec<CollectionDocument<T>>) -> InternEngineResponse,
    Cn: Connection,
{
    match T::all(connection).query() {
        Ok(docs) => on_success(docs),
        Err(err) => {
            eprintln!(
                "Engine: Couldn't get all {} from db: {}",
                std::any::type_name::<T>(),
                err
            );
            ResponseAction::Error(commands::Error::InternalError).into()
        }
    }
}

#[allow(dead_code)]
fn update_in_db<T, Cn>(connection: &Cn, doc: CollectionDocument<T>) -> InternEngineResponse
where
    T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
    Cn: Connection,
{
    update_in_db_and(connection, doc, || ResponseAction::Success.into())
}

#[allow(dead_code)]
fn update_in_db_and<T, Cn, F>(
    connection: &Cn,
    mut doc: CollectionDocument<T>,
    on_success: F,
) -> InternEngineResponse
where
    T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
    Cn: Connection,
    F: Fn() -> InternEngineResponse,
{
    match doc.update(connection) {
        Ok(_) => on_success(),
        Err(err) => {
            eprintln!(
                "Couldn't update {} in db: {}",
                std::any::type_name::<T>(),
                err
            );
            ResponseAction::Error(commands::Error::InternalError).into()
        }
    }
}

#[allow(dead_code)]
fn delete_from_db<T, Cn>(connection: &Cn, doc: CollectionDocument<T>) -> InternEngineResponse
where
    T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
    Cn: Connection,
{
    delete_from_db_and(connection, doc, || ResponseAction::Success.into())
}

#[allow(dead_code)]
fn delete_from_db_and<T, Cn, F>(
    connection: &Cn,
    doc: CollectionDocument<T>,
    on_success: F,
) -> InternEngineResponse
where
    T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
    Cn: Connection,
    F: Fn() -> InternEngineResponse,
{
    match doc.delete(connection) {
        Ok(_) => on_success(),
        Err(err) => {
            eprintln!(
                "Couldn't delete {} from db: {}",
                std::any::type_name::<T>(),
                err
            );
            ResponseAction::Error(commands::Error::InternalError).into()
        }
    }
}

#[allow(dead_code)]
fn add_to_db_and<T, Cn, F>(connection: &Cn, value: T, on_success: F) -> InternEngineResponse
where
    T: SerializedCollection<Contents = T> + 'static,
    Cn: Connection,
    F: Fn(CollectionDocument<T>) -> InternEngineResponse,
{
    match T::push(value, connection) {
        Err(err) => {
            eprintln!(
                "Couldn't push {} into db: {}",
                std::any::type_name::<T>(),
                err
            );
            ResponseAction::Error(commands::Error::InternalError).into()
        }
        Ok(doc) => on_success(doc),
    }
}

#[allow(dead_code)]
fn add_to_db<T, Cn>(connection: &Cn, value: T) -> InternEngineResponse
where
    T: SerializedCollection<Contents = T> + 'static,
    Cn: Connection,
{
    add_to_db_and(connection, value, |_| ResponseAction::Success.into())
}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "session")]
struct Session {
    name: String,
    teams: Vec<TeamEntry>,
    mode: Mode,
    config: PartialConfig,
    discord_server_id: Option<u64>,
    discord_game_channel: Option<u64>,
    discord_admin_channel: Option<u64>,
    game: Option<InGame>,
}

impl Session {
    fn config(&self) -> Config {
        let mut cfg = Config::default();
        cfg.apply_some(self.config.clone());
        cfg
    }

    fn new(name: String, mode: Mode) -> Self {
        Session {
            name,
            mode,
            teams: Vec::new(),
            config: PartialConfig::default(),
            discord_server_id: None,
            discord_game_channel: None,
            discord_admin_channel: None,
            game: None,
        }
    }

    fn to_sendable(&self, id: u64) -> GameSession {
        GameSession {
            name: self.name.clone(),
            mode: self.mode,
            id,
        }
    }

    fn vroom(
        &mut self,
        command: EngineAction,
        session_id: u64,
        player_entries: &[DBEntry<PlayerEntry>],
    ) -> InternEngineResponsePackage {
        use commands::Error::*;
        use BroadcastAction::*;
        use EngineAction::*;
        use ResponseAction::*;
        match command {
            AddChallengeToTeam { team, challenge } => match self.teams.get_mut(team) {
                // THIS METHOD SHOULD BE TEMPORARY AND EXISTS ONLY FOR TESTING PURPOSES
                None => Error(NotFound).into(),
                Some(team) => {
                    team.challenges.push(InOpenChallenge {
                        title: challenge.title,
                        description: challenge.description,
                        points: challenge.points,
                        action: None,
                        zone: None,
                    });
                    Success.into()
                }
            },
            RenameTeam { team, new_name } => match self.teams.get_mut(team) {
                None => Error(NotFound).into(),
                Some(team) => {
                    team.name = new_name;
                    Success.into()
                }
            },
            MakeTeamCatcher(id) => match self.teams.get_mut(id) {
                None => Error(NotFound).into(),
                Some(team) => match team.role {
                    TeamRole::Catcher => Success.into(),
                    TeamRole::Runner => {
                        team.role = TeamRole::Catcher;
                        EngineResponse {
                            response_action: Success,
                            broadcast_action: Some(TeamMadeCatcher(
                                team.to_sendable(player_entries, id),
                            )),
                        }
                        .into()
                    }
                },
            },
            MakeTeamRunner(id) => match self.teams.get_mut(id) {
                None => Error(NotFound).into(),
                Some(team) => match team.role {
                    TeamRole::Runner => Success.into(),
                    TeamRole::Catcher => {
                        team.role = TeamRole::Runner;
                        EngineResponse {
                            response_action: Success,
                            broadcast_action: Some(TeamMadeRunner(
                                team.to_sendable(player_entries, id),
                            )),
                        }
                        .into()
                    }
                },
            },
            SendLocation { player, location } => {
                //println!("Engine: received SendLocation");
                match self
                    .teams
                    .iter()
                    .position(|t| t.players.iter().all(|&p| p == player))
                {
                    None => Error(NotFound).into(),
                    Some(team) => {
                        self.teams[team].locations.insert(
                            0,
                            (location.0, location.1, chrono::offset::Local::now().time()),
                        );
                        //println!("Engine: done with SendLocation");
                        EngineResponse {
                            response_action: Success,
                            broadcast_action: Some(Location { team, location }),
                        }
                        .into()
                    }
                }
            }
            AssignPlayerToTeam { player, team } => {
                let mut old_team = None;
                self.teams.iter_mut().enumerate().for_each(|(index, t)| {
                    if let Some(i) = t.players.iter().position(|p| p == &player) {
                        t.players.remove(i);
                        old_team = Some(index)
                    }
                });
                match team {
                    Some(t) => match self.teams.get_mut(t) {
                        Some(t) => {
                            t.players.push(player);
                            EngineResponse {
                                response_action: Success,
                                broadcast_action: Some(PlayerChangedTeam {
                                    session: session_id,
                                    player,
                                    from_team: old_team,
                                    to_team: team,
                                }),
                            }
                            .into()
                        }
                        None => Error(NotFound).into(),
                    },
                    None => EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(PlayerChangedTeam {
                            session: session_id,
                            player,
                            from_team: old_team,
                            to_team: team,
                        }),
                    }
                    .into(),
                }
            }
            Catch {
                catcher: _,
                caught: _,
            } => Error(NotImplemented).into(), // TODO:
            Complete {
                completer,
                completed,
            } => match self.teams.get_mut(completer) {
                Some(completer) => match completer.challenges.get_mut(completed) {
                    Some(_completed) => todo!(),
                    None => todo!(),
                },
                None => todo!(),
            },
            GetState => SendState {
                teams: self
                    .teams
                    .iter()
                    .enumerate()
                    .map(|(i, t)| t.to_sendable(player_entries, i))
                    .collect(),
                game: self.game.clone().map(|g| g.to_sendable()),
            }
            .into(),
            AddTeam {
                name,
                discord_channel,
                colour,
            } => {
                if let Some(nom) = self
                    .teams
                    .iter()
                    .map(|t| t.name.clone())
                    .find(|n| strcmp(&n.to_lowercase(), &name.to_lowercase()) >= 0.85)
                {
                    Error(TeamExists(nom)).into()
                } else {
                    let colour = match colour {
                        Some(c) => c,
                        None => {
                            match self
                                .config()
                                .team_colours
                                .iter()
                                .find(|&&c| !self.teams.iter().any(|t| t.colour == c))
                            {
                                Some(&colour) => colour,
                                None => Colour { r: 0, g: 0, b: 0 },
                            }
                        }
                    };
                    self.teams
                        .push(TeamEntry::new(name, Vec::new(), discord_channel, colour));
                    Success.into()
                }
            }
            Start => match self.game {
                Some(_) => Error(GameInProgress).into(),
                None => {
                    todo!(); // TODO:
                }
            },
            Stop => Error(NotImplemented).into(), // TODO:
            AddSession { name: _, mode: _ } => Error(SessionSupplied).into(),
            Ping(_) => Error(SessionSupplied).into(),
            GetPlayerByPassphrase(_) => Error(SessionSupplied).into(),
            RemovePlayer { player: _ } => Error(SessionSupplied).into(),
            SetPlayerSession {
                player: _,
                session: _,
            } => Error(SessionSupplied).into(),
            SetPlayerName { player: _, name: _ } => Error(SessionSupplied).into(),
            SetPlayerPassphrase {
                player: _,
                passphrase: _,
            } => Error(SessionSupplied).into(),
            AddPlayer {
                name: _,
                discord_id: _,
                passphrase: _,
                session: _,
            } => Error(SessionSupplied).into(),
            GetRawChallenges => Error(SessionSupplied).into(),
            SetRawChallenge(_) => Error(SessionSupplied).into(),
            AddRawChallenge(_) => Error(SessionSupplied).into(),
        }
    }
}

#[derive(Clone, Debug)]
struct DBEntry<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
{
    id: u64,
    contents: T,
}

impl<T> DBEntry<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
{
    fn make_entry_vector(db: &Database) -> Vec<Self> {
        T::all(db)
            .query()
            .unwrap()
            .into_iter()
            .map(|d| DBEntry {
                id: d.header.id,
                contents: d.contents,
            })
            .collect()
    }
}

#[allow(dead_code)]
pub struct Engine {
    db: Database,
    changes_since_save: bool,

    sessions: Vec<DBEntry<Session>>,
    challenges: Vec<DBEntry<ChallengeEntry>>,
    challenge_sets: Vec<DBEntry<ChallengeSetEntry>>,
    zones: Vec<DBEntry<ZoneEntry>>,
    players: Vec<DBEntry<PlayerEntry>>,

    pictures: Vec<Header>,
    past_games: Vec<Header>,
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

        fn make_entry_vector<T>(db: &Database) -> Vec<DBEntry<T>>
        where
            T: SerializedCollection<Contents = T, PrimaryKey = u64>,
        {
            T::all(db)
                .query()
                .unwrap()
                .into_iter()
                .map(|d| DBEntry {
                    id: d.header.id,
                    contents: d.contents,
                })
                .collect()
        }

        let challenges = make_entry_vector::<ChallengeEntry>(&db);
        let challenge_sets = make_entry_vector::<ChallengeSetEntry>(&db);
        let zones = make_entry_vector::<ZoneEntry>(&db);
        let sessions = make_entry_vector::<Session>(&db);
        let players = make_entry_vector::<PlayerEntry>(&db);

        let past_games = PastGame::all(&db).headers().unwrap();
        let pictures = PictureEntry::all(&db).headers().unwrap();

        Engine {
            db,
            changes_since_save: false,
            challenges,
            challenge_sets,
            zones,
            sessions,
            players,
            past_games,
            pictures,
        }
    }

    pub fn setup(&self) -> InternEngineResponsePackage {
        InternEngineResponsePackage {
            response: InternEngineResponse::DirectResponse(ResponseAction::Success.into()),
            runtime_requests: Some(vec![RuntimeRequest::CreateTimer {
                duration: tokio::time::Duration::from_secs(10),
                payload: InternEngineCommand::AutoSave,
            }]),
        }
    }

    #[allow(dead_code)]
    fn get_from_db<T, F, I>(&self, id: I, on_success: F) -> InternEngineResponse
    where
        T: SerializedCollection,
        F: Fn(CollectionDocument<T>) -> InternEngineResponse,
        I: KeyEncoding<T::PrimaryKey> + std::fmt::Display,
    {
        get_from_db::<T, _, _, _>(&self.db, id, on_success)
    }

    #[allow(dead_code)]
    fn update_in_db<T>(&self, doc: CollectionDocument<T>) -> InternEngineResponse
    where
        T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
    {
        update_in_db(&self.db, doc)
    }

    #[allow(dead_code)]
    fn update_in_db_and<T, F>(
        &self,
        doc: CollectionDocument<T>,
        on_success: F,
    ) -> InternEngineResponse
    where
        T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
        F: Fn() -> InternEngineResponse,
    {
        update_in_db_and(&self.db, doc, on_success)
    }

    #[allow(dead_code)]
    fn delete_from_db<T>(&self, doc: CollectionDocument<T>) -> InternEngineResponse
    where
        T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
    {
        delete_from_db(&self.db, doc)
    }

    #[allow(dead_code)]
    fn delete_from_db_and<T, F>(
        &self,
        doc: CollectionDocument<T>,
        on_success: F,
    ) -> InternEngineResponse
    where
        T: DefaultSerialization + for<'de> Deserialize<'de> + Serialize,
        F: Fn() -> InternEngineResponse,
    {
        delete_from_db_and(&self.db, doc, on_success)
    }

    pub fn vroom(&mut self, command: InternEngineCommand) -> InternEngineResponsePackage {
        use commands::Error::*;
        use BroadcastAction::*;
        use EngineAction::*;
        use ResponseAction::*;
        match command {
            InternEngineCommand::Command(command) => {
                self.changes_since_save = true;
                match command.session {
                    Some(id) => match self.sessions.iter_mut().find(|s| s.id == id) {
                        Some(session) => session.contents.vroom(command.action, id, &self.players),
                        None => Error(NotFound).into()
                    }
                    None => match command.action {
                        GetRawChallenges => SendRawChallenges(self.challenges.iter().filter_map(|c| c.contents.to_sendable(c.id, &self.challenge_sets, &self.zones).ok()).collect()).into(),
                        SetRawChallenge(challenge) => match challenge.id {
                            Some(id) => {
                                match self.challenges.iter_mut().find(|c| c.id == id) {
                                    None => Error(NotFound).into(),
                                    Some(c) => {
                                        c.contents = challenge.clone().into();
                                        Success.into()
                                    }
                                }
                            }
                            None => {
                                Error(BadData(
                                    "the supplied challenge doesn't have an id. this can happen because the RawChallenge::new() method doesn't assign an id. challenges sent from truinlag have an id."
                                    .into()))
                                .into()
                            }
                        },
                        AddRawChallenge(challenge) => {
                            let entry: ChallengeEntry = challenge.clone().into();
                            add_into(&mut self.challenges, entry);
                            Success.into()
                        }
                        GetPlayerByPassphrase(passphrase) => {
                            //println!("Engine: getting player by passphrase {}", passphrase);
                            let doc = self
                                .players
                                .iter()
                                .filter(|p| p.contents.passphrase == passphrase);
                            match doc.count() {
                                0 => {
                                    Error(NotFound).into()
                                }
                                1 => {
                                    let document = self
                                        .players
                                        .iter()
                                        .find(|p| p.contents.passphrase == passphrase)
                                        .expect("should always exist, I just checked for that");
                                    Player(document.contents.to_sendable(document.id)).into()
                                }
                                _ => {
                                    eprintln!(
                                        "Engine: Multiple players seem to have passphrase {}",
                                        passphrase
                                    );
                                    Error(AmbiguousData).into()
                                }
                            }
                        }
                        AddSession { name, mode } => {
                                if self.sessions
                                    .iter()
                                    .any(|s| s.contents.name == name)
                                {
                                    ResponseAction::Error(commands::Error::AlreadyExists).into()
                                } else {
                                    add_into(&mut self.sessions, Session::new(name, mode));
                                    Success.into()
                                }
                        },
                        AddPlayer {
                            name,
                            discord_id,
                            passphrase,
                            session,
                        } => {
                            if self.players
                                .iter()
                                .any(|p| p.contents.passphrase == passphrase)
                            {
                                Error(AlreadyExists).into()
                            } else {
                                add_into(
                                    &mut self.players,
                                    PlayerEntry {
                                        name,
                                        discord_id,
                                        passphrase,
                                        session,
                                    }
                                );
                                Success.into()
                            }
                        },
                        SetPlayerSession { player, session } => {
                            match self.players.iter_mut().find(|p| p.id == player) {
                                None => Error(NotFound).into(),
                                Some(i_player) => {
                                    let old_session = i_player.contents.session;
                                    if old_session == session {
                                        Success.into()
                                    } else {
                                        if let Some(tbr_session) = old_session {
                                            match self.sessions.iter_mut().find(|s| s.id == tbr_session) {
                                                None => {
                                                    println!("Engine: couldn't find session with id {} of player {}", tbr_session, i_player.id)
                                                }
                                                Some(tbr_session) => {
                                                    for team in &mut tbr_session.contents.teams {
                                                        team.players.retain(|p| p != &i_player.id);
                                                    }
                                                }
                                            }
                                        }
                                        match session {
                                            None => {
                                                i_player.contents.session = None;
                                                EngineResponse {
                                                    response_action: Success,
                                                    broadcast_action: Some(PlayerChangedSession {
                                                        player: i_player.contents.to_sendable(i_player.id),
                                                        from_session: old_session,
                                                        to_session: session,
                                                    })
                                                }.into()
                                            }
                                            Some(session) => {
                                                if self.sessions.iter().any(|s| s.id == session) {
                                                    i_player.contents.session = Some(session);
                                                    EngineResponse {
                                                        response_action: Success,
                                                        broadcast_action: Some(PlayerChangedSession {
                                                            player: i_player.contents.to_sendable(i_player.id),
                                                            from_session: old_session,
                                                            to_session: Some(session),
                                                        })
                                                    }.into()
                                                } else {
                                                    Error(NotFound).into()
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        SetPlayerName { player, name } => {
                            match self.players.iter_mut().find(|p| p.id == player) {
                                None => Error(NotFound).into(),
                                Some(player) => {
                                    player.contents.name = name;
                                    Success.into()
                                }
                            }
                        }
                        SetPlayerPassphrase { player, passphrase } => {
                            match self.players.iter_mut().find(|p| p.id == player) {
                                None => Error(NotFound).into(),
                                Some(player) => {
                                    player.contents.passphrase = passphrase;
                                    Success.into()
                                }
                            }
                        }
                        RemovePlayer { player } => {
                            // Note that this doesn't actually remove the player from the db, it just
                            // removes all references to them from all sessions and removes their
                            // passphrase.
                            match self.players.iter_mut().find(|p| p.id == player) {
                                None => Error(NotFound).into(),
                                Some(p) => {
                                    p.contents.passphrase = "".into();
                                    self.sessions.iter_mut().for_each(|s| s.contents.teams.iter_mut().for_each(|t| t.players.retain(|p| p != &player)));
                                    Success.into()
                                }
                            }
                        }
                        Ping(payload) => EngineResponse {
                            response_action: Success,
                            broadcast_action: Some(BroadcastAction::Pinged(payload)),
                        }.into(),
                        GetState => {
                            let sessions = self.sessions.iter().map(|s| s.contents.to_sendable(s.id)).collect();
                            let players = self.players.iter().map(|p| p.contents.to_sendable(p.id)).collect();
                            SendGlobalState { sessions, players }.into()
                        }
                        Start => Error(NoSessionSupplied).into(),
                        Stop => Error(NoSessionSupplied).into(),
                        Catch {
                            catcher: _,
                            caught: _,
                        } => Error(NoSessionSupplied).into(),
                        Complete {
                            completer: _,
                            completed: _,
                        } => Error(NoSessionSupplied).into(),
                        SendLocation {
                            player: _,
                            location: _,
                        } => Error(NoSessionSupplied).into(),
                        AddTeam {
                            name: _,
                            discord_channel: _,
                            colour: _,
                        } => Error(NoSessionSupplied).into(),
                        AssignPlayerToTeam { player: _, team: _ } => Error(NoSessionSupplied).into(),
                        MakeTeamRunner(_) => Error(NoSessionSupplied).into(),
                        MakeTeamCatcher(_) => Error(NoSessionSupplied).into(),
                        AddChallengeToTeam { team: _, challenge: _ } => Error(NoSessionSupplied).into(),
                        RenameTeam { team: _, new_name: _ } => Error(NoSessionSupplied).into(),
                    },
                }
            }
            InternEngineCommand::AutoSave => {
                fn vec_overwrite_in_transaction<T>(
                    entries: Vec<DBEntry<T>>,
                    transaction: &mut Transaction,
                ) -> Result<(), bonsaidb::core::Error>
                where
                    T: SerializedCollection<Contents = T, PrimaryKey = u64> + 'static,
                {
                    let mut ret = Ok(());
                    for entry in entries {
                        match entry
                            .contents
                            .overwrite_in_transaction(&entry.id, transaction)
                        {
                            Ok(()) => (),
                            Err(err) => {
                                println!(
                                    "Engine: something went wrong during overwrite_in_transaction: {}",
                                    err
                                );
                                ret = Err(err);
                            }
                        }
                    }
                    ret
                }
                if self.changes_since_save {
                    let players = self.players.clone();
                    let db = self.db.clone();
                    let sessions = self.sessions.clone();
                    let challenges = self.challenges.clone();
                    let challenge_sets = self.challenge_sets.clone();
                    let zones = self.zones.clone();
                    self.changes_since_save = false;

                    InternEngineResponsePackage {
                        response: Success.into(),
                        runtime_requests: Some(vec![RuntimeRequest::RawLoopback(tokio::spawn(
                            async move {
                                println!("Engine Autosave: starting autosave");
                                let now = tokio::time::Instant::now();
                                let mut transaction = Transaction::new();
                                let _ = vec_overwrite_in_transaction(players, &mut transaction);
                                let _ = vec_overwrite_in_transaction(sessions, &mut transaction);
                                let _ = vec_overwrite_in_transaction(challenges, &mut transaction);
                                let _ =
                                    vec_overwrite_in_transaction(challenge_sets, &mut transaction);
                                let _ = vec_overwrite_in_transaction(zones, &mut transaction);

                                match transaction.apply(&db) {
                                    Ok(yay) => println!(
                                        "Engine Autosave: autosave succeeded in {} ms: {:?}",
                                        now.elapsed().as_millis(),
                                        yay
                                    ),
                                    Err(err) => {
                                        eprintln!("Engine Autosave: AUTOSAVE FAILED HIGH ALERT YOU ARE ALL FUCKED NOW (in {} ms): {}", now.elapsed().as_millis(), err);
                                        panic!("autosave failed")
                                    }
                                }

                                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                InternEngineCommand::AutoSave
                            },
                        ))]),
                    }
                } else {
                    println!("Engine: Autosave requested, but no changes since last save");
                    RuntimeRequest::CreateTimer {
                        duration: Duration::from_secs(10),
                        payload: InternEngineCommand::AutoSave,
                    }
                    .into()
                }
            }
        }
    }
}
