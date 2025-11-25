use crate::{DBEntry, SessionContext, ZoneEntry};
use bonsaidb::core::{
    document::{CollectionDocument, Emit},
    schema::{
        Collection, CollectionMapReduce, ReduceResult, View, ViewMapResult, ViewMappedValue,
        ViewSchema,
    },
};
use chrono::Datelike;
use rand::prelude::*;
use rand::rng;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use truinlag::*;

/// The representation of a challenge set inside the db
#[derive(Debug, Clone, Collection, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[collection(name = "challenge set")]
pub struct ChallengeSetEntry {
    pub name: String,
}

impl ChallengeSetEntry {
    /// Converts the engine-internal `ChallengeSetEntry` type into a sendable truinlag
    /// `ChallengeSet` type
    pub fn to_sendable(&self, id: u64) -> ChallengeSet {
        ChallengeSet {
            name: self.name.clone(),
            id,
        }
    }
}

/// The representation of a raw challenge inside the db. Conforms roughly to the challenge data
/// layout presented in UC4
#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "challenge", views = [UnspecificChallengeEntries, SpecificChallengeEntries, GoodChallengeEntries])]
pub struct ChallengeEntry {
    pub kind: ChallengeType,
    pub sets: Vec<u64>,
    pub status: ChallengeStatus,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Information about whether the challenge location is randomised during challenge generation
    pub random_place: Option<RandomPlaceType>,
    /// Applicable to challenges of types `Kaff` and `ZKaff`. The name of the transit station the
    /// challenge corresponds to.
    pub place: Option<String>,
    pub comment: String,
    pub kaffskala: Option<u8>,
    pub grade: Option<u8>,
    /// The ids of the zones in which the challenge can be completed
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
    /// Calculates the distance to a challenge from a provided zone
    pub fn distance(&self, from: &DBEntry<ZoneEntry>) -> u64 {
        match self.closest_zone(from) {
            Some(zone) => *from.contents.minutes_to.get(&zone).unwrap_or(&0),
            None => 0,
        }
    }

    /// Selects the zone from the challenge's zones that is the closest from a provided zone and
    /// returns its id.
    pub fn closest_zone(&self, from: &DBEntry<ZoneEntry>) -> Option<u64> {
        self.zone
            .iter()
            .filter(|z| {
                if from.contents.minutes_to.contains_key(z) {
                    true
                } else {
                    eprintln!(
                        "Engine: error while calculating distance to zone: \
                        zone {} (id: {}) doesn't have a distance \
                        to the zone with id {}, ignoring zone",
                        from.contents.zone, from.id, z
                    );
                    false
                }
            })
            .min_by_key(|z| from.contents.minutes_to.get(z).unwrap())
            .cloned()
    }

    /// Converts the engine-internal `ChallengeEntry` type into a sendable truinlag `RawChallenge` type
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

    /// Generates points for and returns an open challenge
    #[allow(clippy::too_many_arguments)]
    pub fn challenge(
        &self,
        zone_zoneables: bool,
        centre_zone: DBEntry<ZoneEntry>,
        current_zone: DBEntry<ZoneEntry>,
        allowed_zones: Option<Vec<u64>>,
        id: u64,
        points_to_top: Option<u64>,
        context: &SessionContext,
    ) -> InOpenChallenge {
        use ChallengeType::*;
        let zone_db = context.engine_context.zone_db;
        let config = &context.config;

        // calculate and select zone:
        // the zone is specified in Kaff and Ortsspezifisch challenges, and in ZKaff the zone is
        // always the centre (110). Zoneable challenges can be assigned a random zone and there are
        // challenges with a random place, where a random zone is selected as well. A specialty of
        // Ortsspezifisch challenges is that they can have multiple zones. The closest is selected.
        if zone_db.is_empty() {
            eprintln!(
                "Engine: there are no zones in the database, \
                pointcalc will not work as expected"
            );
        }
        let needs_zone = self.random_place.is_some()
            || matches!(self.kind, Ortsspezifisch | Kaff | ZKaff)
            || (matches!(self.kind, Zoneable) && zone_zoneables);
        let zone = match self.random_place {
            Some(place_type) => match place_type {
                RandomPlaceType::Zone => zone_db
                    .get_all()
                    .iter()
                    .filter(|z| match &allowed_zones {
                        None => true,
                        Some(allowed) => allowed.contains(&z.id),
                    })
                    .choose(&mut rng())
                    .cloned(),
                RandomPlaceType::SBahnZone => zone_db
                    .get_all()
                    .iter()
                    .filter(|z| z.contents.s_bahn_zone)
                    .filter(|z| match &allowed_zones {
                        None => true,
                        Some(allowed) => allowed.contains(&z.id),
                    })
                    .choose(&mut rng())
                    .cloned(),
            },
            None => match self.kind {
                Kaff => match self.zone.first() {
                    Some(id) => zone_db.get(*id),
                    None => {
                        eprintln!("challenge with id {id} has invalid zone");
                        None
                    }
                },
                Ortsspezifisch => match self.closest_zone(&current_zone) {
                    None => {
                        eprintln!("challenge with id {id} has invalid zones");
                        None
                    }
                    Some(zid) => zone_db.get(zid),
                },
                ZKaff => Some(centre_zone.clone()),
                Zoneable => {
                    if zone_zoneables {
                        zone_db
                            .get_all()
                            .iter()
                            .filter(|z| match &allowed_zones {
                                None => true,
                                Some(allowed) => allowed.contains(&z.id),
                            })
                            .choose(&mut rng())
                            .cloned()
                    } else {
                        None
                    }
                }
                _ => None,
            },
        };
        if needs_zone && zone.is_none() {
            eprintln!(
                "something went wrong selecting zone for challenge with id {id}, \
                omitting zone for pointcalc"
            )
        }

        // point calculation
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
        let reps = self.repetitions.clone().choose(&mut rng()).unwrap_or(0);
        points += reps as i64 * self.points_per_rep as i64;
        // zone points
        if let Some(z) = &zone {
            points += z.contents.zonic_kaffness(config) as i64;
            points += (config.travel_minutes_multiplier
                * (*current_zone
                    .contents
                    .minutes_to
                    .get(&z.id)
                    .unwrap_or_else(|| {
                        eprintln!(
                            "Engine: Zone {} has no distance to zone {}, \
                            skipping distance calculation",
                            current_zone.id, z.id
                        );
                        &0
                    }) as f32)
                    .powf(config.travel_minutes_exponent)) as i64;
        }
        match self.kind {
            // zkaff points
            ZKaff => {
                if self.dead_end {
                    points += config.zkaff_points_for_dead_end as i64;
                }
                points +=
                    self.station_distance as i64 / config.zkaff_station_distance_divisor as i64;
                points += self.time_to_hb as i64 * config.zkaff_points_per_minute_to_hb as i64;
                points += ((config.zkaff_departures_base
                    - (self.departures as f32).powf(config.zkaff_departures_exponent))
                    * config.zkaff_departures_multiplier) as i64;
            }
            Zoneable => {
                if zone_zoneables {
                    points += config.points_for_zoneable as i64;
                }
            }
            _ => (),
        }
        match chrono::Local::now().date_naive().weekday() {
            chrono::Weekday::Sat => points = (points as f32 * self.bias_sat) as i64,
            chrono::Weekday::Sun => points = (points as f32 * self.bias_sun) as i64,
            _ => (),
        }
        if let Some(p) = points_to_top {
            let p = p as i64 - config.underdog_starting_difference as i64;
            if p.is_positive() {
                points = (points as f32
                    * (1.0 + (p as f32 * config.underdog_multiplyer_per_1000 * 0.001)))
                    as i64;
            }
        }
        points += Normal::new(0_f64, points as f64 * config.relative_standard_deviation)
            .expect("this cannot fail, since both μ and σ must have real values")
            .sample(&mut rng())
            .round() as i64;
        if self.fixed {
            let fixed_points =
                self.additional_points as i64 + self.points_per_rep as i64 * reps as i64;
            // if the regularly calculated points are way larger than the fixed points, use the
            // regular points instead
            if !fixed_points as f32 * config.fixed_cutoff_mult <= (points - fixed_points) as f32 {
                points = fixed_points
            } else {
                // the regular points shouldn't include the fixed points, since the fixed points
                // are intended to represent the value of the challenge as a whole.
                points -= fixed_points;
                eprintln!(
                    "Engine: challenge {} (id: {}) \
                    granted {} normal points but only {} fixed points, \
                    using normal points instead (total: {}).",
                    self.title
                        .clone()
                        .or(self.place.clone())
                        .unwrap_or("[no title]".into()),
                    id,
                    points,
                    fixed_points,
                    points - fixed_points,
                )
            }
        }

        // generating title and description. These may be automatically generated based on the
        // place name.
        let mut title = config.default_challenge_title.clone();
        let mut description = config.default_challenge_description.clone();
        if let Some(place_name) = self.place.clone() {
            match self.kind {
                ZKaff => {
                    title = format!("Züridrift nach {place_name}");
                    description = format!("Gönd zu de Station {place_name} in Züri.");
                }
                _ => {
                    title = format!("Usflug uf {place_name}");
                    description = format!("Gönd nach {place_name}.");
                }
            }
        }
        if let Some(title_override) = self.title.clone() {
            title = title_override;
        }
        if let Some(desc_override) = self.description.clone() {
            description = desc_override;
        }
        if self.no_disembark {
            title = format!("🛤️ {}", title);
        }
        if let Some(z) = &zone {
            if matches!(self.kind, Zoneable) && zone_zoneables {
                description.push_str(
                    format!(
                        " ⚠️ Damit ihr Pünkt überchömed, mached das i de Zone {}.",
                        z.contents.zone
                    )
                    .as_str(),
                );
            }
            title = title.replace("%z", format!("{}", z.contents.zone).as_str());
            description = description.replace("%z", format!("{}", z.contents.zone).as_str());
            title = title.replace("%s", format!("{}", z.contents.zone).as_str());
            description = description.replace("%s", format!("{}", z.contents.zone).as_str());
        }
        title = title.replace("%r", format!("{}", reps).as_str());
        description = description.replace("%r", format!("{}", reps).as_str());

        // generating challenge action
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
            title,
            description,
            points: points as u64,
            action,
            zone: zone.map(|z| z.id),
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

/// A view that is never actually used anywhere but it's still here... TODO: remove this?
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

/// A view that is never actually used anywhere but it's still here... TODO: remove this?
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

/// A view that is never actually used anywhere but it's still here... TODO: remove this?
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

/// The representation of an open challenge inside the db
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
    /// Determines whether the challenge can be completed
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

    /// Converts the engine-internal `InOpenChallenge` type into a sendable truinlag `Challenge` type
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

/// A special quirk a challenge may have
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChallengeAction {
    UncompletableMinutes(chrono::DateTime<chrono::Local>),
    Trap {
        completable_after: chrono::DateTime<chrono::Local>,
        catcher_message: Option<String>,
    },
}
