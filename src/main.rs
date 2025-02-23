pub(crate) mod challenge;
pub(crate) mod engine;
mod error;
pub(crate) mod runtime;
pub(crate) mod session;
pub(crate) mod team;

use bonsaidb::core::schema::{Collection, Schema, SerializedCollection};
use challenge::{ChallengeEntry, ChallengeSetEntry};
use error::Result;
use image::imageops::FilterType;
use partially::Partial;
use runtime::manager;
use serde::{Deserialize, Serialize};
use session::Session;
use std::collections::HashMap;
use team::TeamEntry;
use truinlag::*;

#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DBEntry<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
{
    id: u64,
    contents: T,
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
    pub points_per_travel_minute: u64,

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

    // Number of Catchers
    pub num_catchers: u64,

    // starting zone id
    pub start_zone: u64,

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
            points_per_travel_minute: 12,
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
            start_zone: 0,
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

pub fn add_into<T>(collection: &mut Vec<DBEntry<T>>, item: T)
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
