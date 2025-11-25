use image::{DynamicImage, ImageFormat};
use partially::Partial;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub mod api;
pub mod commands;

type Timestamp = i64;

#[derive(Partial, Debug, Clone, Serialize, Deserialize)]
#[partially(derive(Debug, Clone, Serialize, Deserialize, Default))]
pub struct GameConfig {
    pub num_catchers: u64,
    pub start_zone: u64,
    pub start_time: chrono::NaiveTime,
    pub end_time: chrono::NaiveTime,
    pub challenge_sets: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailedLocation {
    pub latitude: f32,
    pub longitude: f32,
    /// accuracy in metres
    pub accuracy: u16,
    /// heading in degrees. 0º is north (hopefully)
    pub heading: f32,
    /// speed in m/s
    pub speed: f32,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinimalLocation {
    pub latitude: f32,
    pub longitude: f32,
    pub timestamp: Timestamp,
}

#[cfg(feature = "build-binary")]
impl MinimalLocation {
    pub fn as_point(&self) -> geo::Point<f64> {
        geo::Point::from((self.latitude as f64, self.longitude as f64))
    }
}

impl From<DetailedLocation> for MinimalLocation {
    fn from(value: DetailedLocation) -> Self {
        MinimalLocation {
            latitude: value.latitude,
            longitude: value.longitude,
            timestamp: value.timestamp,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A binary JPEG-encoded picture. It *should* always be a valid JPEG.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawPicture {
    data: Vec<u8>,
}

/// A binary JPEG-encoded picture with some metadata. It *should*
/// always be a valid JPEG.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Picture {
    pub data: RawPicture,
    pub is_thumbnail: bool,
    pub id: u64,
}

impl TryFrom<DynamicImage> for RawPicture {
    type Error = image::ImageError;
    fn try_from(img: DynamicImage) -> Result<Self, image::ImageError> {
        Self::from_img(img)
    }
}

impl From<RawPicture> for DynamicImage {
    fn from(jpeg: RawPicture) -> Self {
        jpeg.try_into_img().unwrap()
    }
}

impl RawPicture {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, image::ImageError> {
        Self::from_img(image::load_from_memory_with_format(
            &bytes,
            ImageFormat::Jpeg,
        )?)
    }

    pub fn from_img(img: DynamicImage) -> Result<Self, image::ImageError> {
        let mut buff = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buff, ImageFormat::Jpeg)?;
        Ok(Self {
            data: buff.into_inner(),
        })
    }

    pub fn try_into_img(self) -> Result<DynamicImage, image::ImageError> {
        image::load_from_memory_with_format(&self.data, ImageFormat::Jpeg)
    }

    pub fn get_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameSession {
    pub name: String,
    pub mode: Mode,
    pub id: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum ChallengeType {
    Kaff,
    Ortsspezifisch,
    Regionsspezifisch,
    Unspezifisch,
    Zoneable,
    ZKaff,
}

impl FromStr for ChallengeType {
    type Err = TextError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace("_", "").as_str() {
            "kaff" => Ok(Self::Kaff),
            "ortsspezifisch" => Ok(Self::Ortsspezifisch),
            "regionsspezifisch" => Ok(Self::Regionsspezifisch),
            "unspezifisch" => Ok(Self::Unspezifisch),
            "zoneable" => Ok(Self::Zoneable),
            "zkaff" => Ok(Self::ZKaff),
            _ => Err(TextError(format!(
                "failed parsing \"{}\" as ChallengeType",
                s
            ))),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub enum TeamRole {
    Runner,
    Catcher,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub enum RandomPlaceType {
    Zone,
    SBahnZone,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum ChallengeStatus {
    Approved,
    Edited,
    Rejected,
    Glorious,
    ToSort,
    Refactor,
}

#[derive(Debug)]
pub struct TextError(pub String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TextError {}

impl From<&str> for TextError {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl FromStr for ChallengeStatus {
    type Err = TextError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace("_", "").as_str() {
            "approved" => Ok(Self::Approved),
            "edited" => Ok(Self::Edited),
            "rejected" => Ok(Self::Rejected),
            "glorious" => Ok(Self::Glorious),
            "tosort" => Ok(Self::ToSort),
            "refactor" => Ok(Self::Refactor),
            _ => Err(TextError(format!(
                "failed parsing \"{}\" as ChallengeStatus",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChallengeActionEntry {
    UncompletableMinutes(Option<u64>), // None -> uses repetitions (%r)

    //das mit de trap isch mal es konzept, ich säg mal bitte nonig :)
    Trap {
        stuck_minutes: Option<u64>, // None -> uses repetitions (%r)
        catcher_message: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChallengeSet {
    pub name: String,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, partially::Partial)]
#[partially(derive(Clone, Debug))]
pub struct RawChallenge {
    pub kind: ChallengeType,
    pub sets: Vec<ChallengeSet>,
    pub status: ChallengeStatus,
    pub title: Option<String>,
    pub description: Option<String>,
    pub random_place: Option<RandomPlaceType>,
    pub place: Option<String>,
    pub comment: String,
    pub kaffskala: Option<u8>,
    pub grade: Option<u8>,
    pub zone: Vec<Zone>,
    pub bias_sat: f32,
    pub bias_sun: f32,
    pub walking_time: u8,
    pub stationary_time: u8,
    pub additional_points: i16,
    pub repetitions: std::ops::Range<u16>,
    pub points_per_rep: i16,
    pub station_distance: u16,
    pub time_to_hb: u8,
    pub departures: u8,
    pub dead_end: bool,
    pub no_disembark: bool,
    pub fixed: bool,
    pub in_perimeter_override: Option<bool>,
    pub translated_titles: std::collections::HashMap<String, String>,
    pub translated_descriptions: std::collections::HashMap<String, String>,
    pub action: Option<ChallengeActionEntry>,
    pub last_edit: chrono::DateTime<chrono::Local>,
    pub id: Option<u64>,
}

impl From<RawChallenge> for InputChallenge {
    fn from(challenge: RawChallenge) -> Self {
        InputChallenge {
            kind: challenge.kind,
            sets: challenge.sets.iter().map(|s| s.id).collect(),
            status: challenge.status,
            title: challenge.title,
            description: challenge.description,
            random_place: challenge.random_place,
            place: challenge.place,
            comment: challenge.comment,
            kaffskala: challenge.kaffskala,
            grade: challenge.grade,
            zone: challenge.zone.iter().map(|z| z.id).collect(),
            bias_sat: challenge.bias_sat,
            bias_sun: challenge.bias_sun,
            walking_time: challenge.walking_time,
            stationary_time: challenge.stationary_time,
            additional_points: challenge.additional_points,
            repetitions: challenge.repetitions,
            points_per_rep: challenge.points_per_rep,
            station_distance: challenge.station_distance,
            time_to_hb: challenge.time_to_hb,
            departures: challenge.departures,
            dead_end: challenge.dead_end,
            no_disembark: challenge.no_disembark,
            fixed: challenge.fixed,
            in_perimeter_override: challenge.in_perimeter_override,
            translated_titles: challenge.translated_titles,
            translated_descriptions: challenge.translated_descriptions,
            action: challenge.action,
            id: challenge.id,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, partially::Partial)]
#[partially(derive(Clone, Debug))]
pub struct InputChallenge {
    pub kind: ChallengeType,
    pub sets: Vec<u64>,
    pub status: ChallengeStatus,
    pub title: Option<String>,
    pub description: Option<String>,
    pub random_place: Option<RandomPlaceType>,
    pub place: Option<String>,
    pub comment: String,
    pub kaffskala: Option<u8>,
    pub grade: Option<u8>,
    pub zone: Vec<u64>,
    pub bias_sat: f32,
    pub bias_sun: f32,
    pub walking_time: u8,
    pub stationary_time: u8,
    pub additional_points: i16,
    pub repetitions: std::ops::Range<u16>,
    pub points_per_rep: i16,
    pub station_distance: u16,
    pub time_to_hb: u8,
    pub departures: u8,
    pub dead_end: bool,
    pub no_disembark: bool,
    pub fixed: bool,
    pub in_perimeter_override: Option<bool>,
    pub translated_titles: std::collections::HashMap<String, String>,
    pub translated_descriptions: std::collections::HashMap<String, String>,
    pub action: Option<ChallengeActionEntry>,
    pub id: Option<u64>,
}

impl InputChallenge {
    pub fn check_validity(&self) -> Result<(), TextError> {
        if matches!(self.status, ChallengeStatus::ToSort) {
            return Ok(());
        }
        if self.sets.is_empty() {
            return Err("has no challenge sets".into());
        }
        use ChallengeType::*;
        if matches!(self.kind, Kaff | ZKaff) && self.place.is_none() {
            return Err(TextError(String::from(
                "is Kaff or ZKaff, but doesn't have place",
            )));
        };
        if matches!(self.kind, Kaff | Ortsspezifisch) {
            if self.kaffskala.is_none() {
                return Err("is Kaff or Ortsspezifisch, but doesn't have kaffskala".into());
            }
            if self.grade.is_none() {
                return Err("is Kaff or Ortsspezifisch, but doesn't have grade".into());
            }
            if self.zone.is_empty() {
                return Err("is Kaff or Ortsspezifisch, but doesn't have zone".into());
            }
        }
        if matches!(self.kind, ZKaff) {
            if self.station_distance == 0 {
                return Err("is ZKaff, but station distance is 0".into());
            }
            if self.time_to_hb == 0 {
                return Err("is ZKaff, but time to HB is 0".into());
            }
        }
        if !matches!(self.kind, Kaff | ZKaff) {
            if self.description.is_none() {
                return Err("is neither Kaff nor ZKaff, but has no description".into());
            }
            if self.title.is_none() {
                return Err("is neither Kaff nor ZKaff, but has no title".into());
            }
        }
        if matches!(self.kind, ZKaff | Kaff | Ortsspezifisch) && self.random_place.is_some() {
            return Err("is ortsspezifisch, but has random place".into());
        }
        Ok(())
    }

    pub fn is_valid(&self) -> bool {
        self.check_validity().is_ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    pub zone: u64,
    pub num_conn_zones: u64,
    pub num_connections: u64,
    pub train_through: bool,
    pub mongus: bool,
    pub s_bahn_zone: bool,
    pub minutes_to: std::collections::HashMap<u64, u64>,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Team {
    pub role: TeamRole,
    pub name: String,
    pub picture_id: Option<u64>,
    pub id: usize,
    pub colour: Colour,
    pub bounty: u64,
    pub points: u64,
    pub players: Vec<Player>,
    pub challenges: Vec<Challenge>,
    pub completed_challenges: Vec<CompletedChallenge>,
    pub location: Option<DetailedLocation>,
    pub in_grace_period: bool,
    pub period_id: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Player {
    pub name: String,
    pub id: u64,
    pub session: Option<u64>,
    pub picture_id: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Challenge {
    pub title: String,
    pub description: String,
    pub points: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CompletedChallenge {
    pub title: String,
    pub description: String,
    pub points: u64,
    pub time: chrono::NaiveTime,
    pub picture_ids: Vec<u64>,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub enum Mode {
    Traditional,
    Gfrorefurz,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Game {
    pub name: String,
    pub date: chrono::NaiveDate,
    pub mode: Mode,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Event {
    Catch {
        catcher_id: usize,
        caught_id: usize,
        bounty: u64,
        time: chrono::NaiveTime,
        picture_ids: Vec<u64>,
        location: MinimalLocation,
    },
    Complete {
        challenge: Challenge,
        completer_id: usize,
        time: chrono::NaiveTime,
        picture_ids: Vec<u64>,
        location: MinimalLocation,
    },
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        let my_time = match self {
            Event::Catch {
                catcher_id: _,
                caught_id: _,
                bounty: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
            Event::Complete {
                challenge: _,
                completer_id: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
        };
        let other_time = match other {
            Event::Catch {
                catcher_id: _,
                caught_id: _,
                bounty: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
            Event::Complete {
                challenge: _,
                completer_id: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
        };
        my_time.eq(other_time)
    }
}

impl Eq for Event {}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let my_time = match self {
            Event::Catch {
                catcher_id: _,
                caught_id: _,
                bounty: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
            Event::Complete {
                challenge: _,
                completer_id: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
        };
        let other_time = match other {
            Event::Catch {
                catcher_id: _,
                caught_id: _,
                bounty: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
            Event::Complete {
                challenge: _,
                completer_id: _,
                picture_ids: _,
                time,
                location: _,
            } => time,
        };
        my_time.cmp(other_time)
    }
}
