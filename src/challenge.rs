use crate::{Config, DBEntry, DBMirror, ZoneEntry};
use bonsaidb::core::{
    document::{CollectionDocument, Emit},
    schema::{
        Collection, CollectionMapReduce, ReduceResult, View, ViewMapResult, ViewMappedValue,
        ViewSchema,
    },
};
use rand::prelude::*;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use truinlag::*;

#[derive(Debug, Clone, Collection, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[collection(name = "challenge set")]
pub struct ChallengeSetEntry {
    pub name: String,
}

impl ChallengeSetEntry {
    pub fn to_sendable(&self, id: u64) -> ChallengeSet {
        ChallengeSet {
            name: self.name.clone(),
            id,
        }
    }
}

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "challenge", views = [UnspecificChallengeEntries, SpecificChallengeEntries, GoodChallengeEntries])]
pub struct ChallengeEntry {
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
    pub translated_titles: HashMap<String, String>,
    pub translated_descriptions: HashMap<String, String>,
    pub action: Option<ChallengeActionEntry>,
    pub last_edit: chrono::DateTime<chrono::Local>,
}

impl ChallengeEntry {
    pub fn distance(&self, from: &DBEntry<ZoneEntry>) -> u64 {
        *self.zone
            .iter()
            .filter_map(|z| from
                .contents
                .minutes_to
                .get(z)
                .or_else(|| {
                    eprintln!(
                        "Engine: error while calculating distance to zone: zone {} (id: {}) doesn't have a distance to the zone with id {}, ignoring zone",
                        from.contents.zone,
                        from.id,
                        z
                    );
                    None
                })
            )
        .min()
        .unwrap_or_else(|| {
            eprintln!("Engine: error while calculating distance to zone, has no zones or none with valid distances");
            &0
        })
    }

    pub fn to_sendable(
        &self,
        id: u64,
        challenge_sets: &[DBEntry<ChallengeSetEntry>],
        zone_entries: &[DBEntry<ZoneEntry>],
    ) -> Result<RawChallenge, commands::Error> {
        Ok(RawChallenge {
            kind: self.kind,
            sets: {
                let mut sets = Vec::new();
                for s in self.sets.clone() {
                    sets.push({
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
                        let zone = zone_entries.iter().find(|z| z.id == s).ok_or_else(|| {
                            eprintln!("Couldn't find zone with id {} in db, maybe it was improperly removed?", s);
                            commands::Error::InternalError})?; zone.contents.to_sendable(zone.id)});
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
    pub fn challenge(
        &self,
        config: &Config,
        zone_zoneables: bool,
        zone_db: &DBMirror<ZoneEntry>,
        id: u64,
    ) -> InOpenChallenge {
        // TODO: if zoneable and zone specified do something to let me know kthxbye
        if zone_db.is_empty() {
            eprintln!(
                "Engine: there are no zones in the database, \
                challenge generation will not work as expected"
            );
        }

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
        let mut zone_entries = self.zone.iter().filter_map(|z| zone_db.get(*z)).collect();
        if zone_zoneables && matches!(self.kind, ChallengeType::Zoneable) && !zone_db.is_empty() {
            zone_entries = vec![zone_db
                .get_all()
                .into_iter()
                .choose(&mut thread_rng())
                .expect("There are probably no ZoneEntries in the database")]
        }
        if let Some(place_type) = &self.random_place {
            match place_type {
                RandomPlaceType::Zone => {
                    zone_entries = vec![zone_db
                        .get_all()
                        .into_iter()
                        .choose(&mut thread_rng())
                        .expect("There are probably no ZoneEntries")];
                }
                RandomPlaceType::SBahnZone => {
                    zone_entries = vec![zone_db
                        .get_all()
                        .into_iter()
                        .filter(|z| z.contents.s_bahn_zone)
                        .choose(&mut thread_rng())
                        .expect("no s-bahn zones found in database")];
                }
            }
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

        let zone = zone.map(|z| z.id);

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

        InOpenChallenge {
            title: title.unwrap_or(config.default_challenge_title.clone()),
            description: description.unwrap_or(config.default_challenge_description.clone()),
            points: points as u64,
            action,
            zone,
            id,
        }
    }
}

impl From<InputChallenge> for ChallengeEntry {
    fn from(v: InputChallenge) -> Self {
        ChallengeEntry {
            kind: v.kind,
            sets: v.sets,
            status: v.status,
            title: v.title,
            description: v.description,
            random_place: v.random_place,
            place: v.place,
            comment: v.comment,
            kaffskala: v.kaffskala,
            grade: v.grade,
            zone: v.zone,
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
            last_edit: chrono::Local::now(),
        }
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
                ChallengeType::ZKaff => false,
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
                ChallengeType::ZKaff => false,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InOpenChallenge {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub points: u64,
    pub action: Option<ChallengeAction>,
    pub zone: Option<u64>, // id for ZoneEntry collection in db
}

impl InOpenChallenge {
    #[allow(dead_code)]
    pub fn completable(&self) -> bool {
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
pub enum ChallengeAction {
    UncompletableMinutes(chrono::DateTime<chrono::Local>),
    Trap {
        completable_after: chrono::DateTime<chrono::Local>,
        catcher_message: Option<String>,
    },
}
