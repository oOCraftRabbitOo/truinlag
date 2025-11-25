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
use std::{collections::HashMap, ops::Range};
use team::{PeriodContext, TeamEntry};
use truinlag::{
    commands::{EngineAction, EngineCommand},
    *,
};

/// this just starts the manager from the runtime :)
#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();
    Ok(())
}

/// This is to keep track of timers. A `TimerHook` represents a timer that should be currently
/// running. This is useful to cancel the timer if it is no longer needed or to restart it if it is
/// not actually running.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TimerHook {
    payload: InternEngineCommand,
    end_time: chrono::DateTime<chrono::Local>,
    id: u64,
}

// This implementation exists for db backwards compatibility.
impl Default for TimerHook {
    fn default() -> Self {
        Self {
            payload: InternEngineCommand::Command(Box::new(EngineCommand {
                action: EngineAction::Ping(None),
                session: None,
            })),
            end_time: chrono::Local::now(),
            id: u64::MAX,
        }
    }
}

impl TimerHook {
    /// Returns a `RuntimeRequest` that cancels the timer.
    fn cancel_request(&self) -> RuntimeRequest {
        RuntimeRequest::CancelTimer(self.id)
    }

    /// Returns a `RuntimeRequest` that starts the timer.
    ///
    /// If an id is provided, it will overwrite the `TimerHook`'s current id.
    fn create_request(&self) -> RuntimeRequest {
        RuntimeRequest::CreateAlarm {
            time: self.end_time,
            payload: self.payload.clone(),
            id: self.id,
        }
    }
}

/// The `TimerTracker` is used to create new timers.
///
/// Each timer needs to have a unique id for it to be able to be cancelled. This struct contains
/// a simple counter that is incremented whenever a timer is created using it.
#[derive(Debug)]
pub struct TimerTracker {
    current_id: u64,
}

impl TimerTracker {
    /// Creates a new `TimerTracker`.
    fn new() -> Self {
        TimerTracker { current_id: 0 }
    }

    /// Returns a `RuntimeRequest` that creates a new alarm and a `TimerHook` that represents it.
    ///
    /// An alarm in truinlag is a request to the runtime to send an `InternEngineCommand` to the
    /// engine at a specific time. This method is used to create such alarms.
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

    /// Returns a `RuntimeRequest` that creates a new timer and a `TimerHook` that represents it.
    ///
    /// A timer in truinlag is a request to the runtime to send an `InternEngineCommand` to the
    /// engine at a specific time. This method is used to create such timers.
    fn timer(
        &mut self,
        duration: chrono::TimeDelta,
        payload: InternEngineCommand,
    ) -> (RuntimeRequest, TimerHook) {
        let time = chrono::Local::now() + duration;
        self.alarm(time, payload)
    }
}

/// A grouping of references to data contained within the engine. Its primary purpose is to make
/// function signatures smaller.
#[derive(Debug)]
pub struct EngineContext<'a> {
    player_db: &'a DBMirror<PlayerEntry>,
    challenge_db: &'a DBMirror<ChallengeEntry>,
    challenge_set_db: &'a DBMirror<ChallengeSetEntry>,
    zone_db: &'a DBMirror<ZoneEntry>,
    past_game_db: &'a mut DBMirror<PastGame>,
    picture_db: &'a mut DBMirror<PictureEntry>,
    timer_tracker: &'a mut TimerTracker,
}

/// A grouping of data contained within the relevant session and the engine. Its primary purpose is
/// to make function signatures smaller.
#[derive(Debug)]
pub struct SessionContext<'a> {
    engine_context: EngineContext<'a>,
    top_team_points: Option<u64>,
    session_id: u64,
    config: Config,
}

/// An owned copy of an entry in the database.
#[derive(Clone, Debug)]
pub struct ClonedDBEntry<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    pub id: u64,
    pub contents: T,
}

impl<T> ClonedDBEntry<T>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64>,
    T: Clone,
{
    /// Creates a `DBEntry` representation of the `ClonedDBEntry`
    pub fn as_borrowed(&self) -> DBEntry<'_, T> {
        DBEntry {
            id: self.id,
            contents: &self.contents,
        }
    }
}

/// An entry in the database.
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
    /// Creates an owned copy of the database entry.
    pub fn clone_contents(&self) -> ClonedDBEntry<T> {
        ClonedDBEntry {
            id: self.id,
            contents: self.contents.clone(),
        }
    }
}

/// A mutable entry in the database.
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
    /// Creates an owned copy of the database entry.
    pub fn clone_contents(&self) -> ClonedDBEntry<T> {
        ClonedDBEntry {
            id: self.id,
            contents: self.contents.clone(),
        }
    }
}

/// Used to remember changes in the db mirrors for less expensive autosaves and to enable
/// deletions.
#[derive(Clone, Debug)]
enum DBStatus {
    Unchanged,
    Edited,
    ToBeDeleted,
    BeingDeleted,
}

/// A mirror of a database collection that contains all entries (in memory!).
///
/// The truinlag engine needs to run strictly synchronously and its primary purpose is to
/// manipulate game data. If the data was stored in the database, the access would take a very long
/// time and the engine would be very slow. Since simple information about the game (like player
/// names and teams and point counts etc.) is very small, it's all stored in memory for much
/// quicker access and periodically written to disk in a separate task. The `DBMirror` struct
/// exists to facilitate this.
///
/// The `DBMirror` contains all database items and their status. Database items can be retreived
/// from the mirror and if an item is retreived mutably, its status is set to `Edited`. The autosave
/// will then extract all changes from the mirror, which yields all changed entries and sets their
/// status back to `Unchanged`. Items can also be deleted from the db. Deleted items are retained,
/// but have their status changed to `ToBeDeleted`. When the autosave extracts all changes, their
/// status is further changed to `BeingDeleted`. This is to prevent a desync between the database
/// and the mirror, which could occur of there was an error while deleting the file from the db.
/// The deleted entries are only deleted from the mirror once confirmation is received.
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
    /// Loads a collection's contents from the db and creates a mirror.
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

    /// Returns the amount of entries in the db
    fn count(&self) -> usize {
        self.entries.len()
    }

    /// Gets the item with the requested id from the db. Returns `None` if the item doesn't exist.
    fn get(&self, id: u64) -> Option<DBEntry<'_, T>> {
        match self.entries.get(id as usize) {
            // could be optimised with binary search?
            Some(Some((contents, status))) => match status {
                DBStatus::Unchanged | DBStatus::Edited => Some(DBEntry { id, contents }),
                DBStatus::ToBeDeleted | DBStatus::BeingDeleted => None,
            },
            Some(None) => None,
            None => None,
        }
    }

    /// Mutably gets the item with the requested id from the db. Returns `None` if the item doesn't exist.
    fn get_mut(&mut self, id: u64) -> Option<MutDBEntry<'_, T>> {
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

    /// Tests if any entry in the collection matches a predicate.
    fn any(&self, mut predicate: impl FnMut(&T) -> bool) -> bool {
        self.entries.iter().any(|entry| match entry {
            None => false,
            Some((item, _)) => predicate(item),
        })
    }

    /// Searches for an entry in the collection that satisfies a predicate.
    fn find(&self, mut predicate: impl FnMut(&T) -> bool) -> Option<DBEntry<'_, T>> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, entry)| match entry {
                None => false,
                Some((item, _)) => predicate(item), // TODO: return false if deleted
            })
            .map(|(index, entry)| DBEntry {
                id: index as u64,
                contents: match entry {
                    None => panic!(),
                    Some((item, _)) => item,
                },
            })
    }

    /// Searches for an entry in the collection that satisfies a predicate. (mutably)
    fn find_mut(&mut self, mut predicate: impl FnMut(&T) -> bool) -> Option<MutDBEntry<'_, T>> {
        match self
            .entries
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| match entry {
                None => false,
                Some((item, _)) => predicate(item), // TODO: return false if deleted
            }) {
            Some((index, entry)) => {
                let contents = entry.as_mut().unwrap();
                contents.1 = DBStatus::Edited;
                Some(MutDBEntry {
                    id: index as u64,
                    contents: &mut contents.0,
                })
            }
            None => None,
        }
    }

    /// Gets all entries from the collection in a `Vec`.
    fn get_all(&self) -> Vec<DBEntry<'_, T>> {
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

    /// Adds a new entry to the collection.
    fn add(&mut self, thing: T) -> u64 {
        let new_entry = Some((thing, DBStatus::Edited));
        let iter = self.entries.iter_mut();
        match iter.enumerate().find(|(_i, e)| e.is_none()) {
            Some((index, entry)) => {
                *entry = new_entry;
                index as u64
            }
            None => {
                self.entries.push(new_entry);
                self.entries.len() as u64 - 1
            }
        }
    }

    /// Deletes an entry from the collection
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

    /// Deletes all entries from the collection
    fn delete_all(&mut self) {
        for entry in self.entries.iter_mut().flatten() {
            entry.1 = DBStatus::ToBeDeleted;
        }
    }

    /// Extracts all changes in the collection for saving them to disk.
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

    /// Confirms deletions.
    fn clear_pending_deletions(&mut self) {
        for entry in &mut self.entries {
            if let Some((_, status)) = entry {
                if matches!(status, DBStatus::BeingDeleted) {
                    *entry = None;
                }
            }
        }
    }

    /// Extracts all deletions in the collection for saving them to disk.
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

    /// Returns whether the collection is empty.
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Contains config values for the game.
///
/// A `Config` is always a complete set of configuration values that can be used to run the game.
/// Since many of the values are very specific, obscure and unlikely to need change, a
/// `PartialConfig` can store overrides for only some of the values. A `PartialConfig` is saved
/// per session and can be made into a full config using `Config`'s implementation of `Default`.
#[derive(Partial, Debug, Clone, Serialize, Deserialize)]
#[partially(derive(Debug, Clone, Serialize, Deserialize, Default))]
pub struct Config {
    // Pointcalc
    /// At the very end of the point calculation process, the amount of points calculated is
    /// randomised a bit, so that the points don't look to sterile. This is done with a normal
    /// distribution where the standard deviation is a fraction of the points calculated. This
    /// value corresponds to that fraction.
    /// *Recommended Value:* **0.05**
    pub relative_standard_deviation: f64,
    /// The Kaffskala is system to measure how out-of-the-way and hard to reach a place is. The
    /// more kaffig a place is, the more points it should give. Kaffness ranges from 1 to 6.
    /// *Recommended Value:* **90**
    pub points_per_kaffness: u64,
    /// The transit grade is a system to measure how well-accessible a place is by transit. It is
    /// [provided by the canton of Zürich](https://geo.zh.ch). Less accessible places should give
    /// more points. Transit grade ranges from 1 to 6.
    /// *Recommended Value:* **20**
    pub points_per_grade: u64,
    /// For certain challenges, teams need to walk. Such challenges should give more points,
    /// depending on how long the team has to walk to solve it.
    /// *Recommended Value:* **10**
    pub points_per_walking_minute: u64,
    /// For certain challenges, teams have to wait around. Such challenges should give more points,
    /// depending on the how long the team has to wait.
    /// *Recommended Value:* **10**
    pub points_per_stationary_minute: u64,
    /// Since challenges are selected randomly, some challenges may be further from the team than
    /// others. The longer the team has to travel, the more points the challenge should give.
    /// The points p for minutes travelled t are calculated based on the exponent e and the
    /// multiplier m using the formula $p = m * (t^e)$.
    /// *Recommended Value:* **1.4**
    pub travel_minutes_exponent: f32,
    /// Since challenges are selected randomly, some challenges may be further from the team than
    /// others. The longer the team has to travel, the more points the challenge should give.
    /// The points p for minutes travelled t are calculated based on the exponent e and the
    /// multiplier m using the formula $p = m * (t^e)$.
    /// *Recommended Value:* **3.0**
    pub travel_minutes_multiplier: f32,
    /// Zoneables get points like unspecific challenges. If they are made to be specific, they
    /// should receive a boost in points.
    /// *Recommended Value:* **100**
    pub points_for_zoneable: u64,
    /// Stations that, once reached, have only one way back, should get a boost in points.
    /// *Recommended Value:* **50**
    pub zkaff_points_for_dead_end: u64,
    /// Stations in Zürich should get more points depending on how far from the closest train
    /// station they are.
    /// Points p for station distance s (in metres) are calculated based on the divisor d using the
    /// formula $p = \frac{s}{d}$.
    /// *Recommended Value:* **20**
    pub zkaff_station_distance_divisor: u64,
    /// Stations in Zürich should get more points depending on how long it takes to get to HB.
    /// *Recommended Value:* **5**
    pub zkaff_points_per_minute_to_hb: u64,
    /// Stations in Zürich should get more points depending on how few departures they have.
    /// Points p for departures d (per hour) are calculated based on the exponent e, the base b and
    /// the multiplier m using the formula $p = m(b-d^e)$.
    /// *Recommended Value:* **1/3**
    pub zkaff_departures_exponent: f32,
    /// Stations in Zürich should get more points depending on how few departures they have.
    /// Points p for departures d (per hour) are calculated based on the exponent e, the base b and
    /// the multiplier m using the formula $p = m(b-d^e)$.
    /// *Recommended Value:* **7.0**
    pub zkaff_departures_base: f32,
    /// Stations in Zürich should get more points depending on how few departures they have.
    /// Points p for departures d (per hour) are calculated based on the exponent e, the base b and
    /// the multiplier m using the formula $p = m(b-d^e)$.
    /// *Recommended Value:* **32.0**
    pub zkaff_departures_multiplier: f32,
    /// If a challenge has fixed points, it does not follow normal point calculation. In order to
    /// avoid imbalance, if the regularly calcuted points minus the fixed points are more than
    /// fixed_cutoff_mult times as large than the fixed points, the regular points minus the fixed
    /// points are used.
    pub fixed_cutoff_mult: f32,

    // underdog system
    /// Teams that are far behind first place should get a subtle boost in points. The starting
    /// difference determines how far behind a team must be for the boost to start to kick in.
    /// *Recommended Value:* **1000**
    pub underdog_starting_difference: u64,
    /// Teams that are far behind first place should get a subtle boost in points. For each 1000
    /// points they are behind, their challenges should give a fraction of their initial point
    /// count more points.
    /// *Recommended Value:* **0.25**
    pub underdog_multiplyer_per_1000: f32,

    // perimeter system
    /// While the perimeter tries to limit the playing area, teams shouldn't get challenges that
    /// are too out-of-the-way anymore. During the perimeter period, the kaffness of challenges
    /// should be limited to a certain maximum (inclusive).
    /// *Recommended Value:* **4**
    pub perim_max_kaff: u64,
    /// The perimeter period tries to slowly limit the playing area. In the beginning, only
    /// challenges that are very far (in terms of travel time in minutes) from HB should be
    /// discarded and at the end, only relatively close challenges should remain.
    /// *Recommended Value:* **20..61**
    pub perim_distance_range: std::ops::Range<u64>,

    // distances
    /// During normal period, one challenge generated should be close by. This limits challenges to
    /// ones that are a certain number of minutes in terms of travel time away.
    /// *Recommended Value:* **0..26**
    pub normal_period_near_distance_range: std::ops::Range<u64>,
    /// During normal period, one challenge generated should be far away. This limits challenges to
    /// ones that are a certain number of minutes in terms of travel time away.
    /// *Recommended Value:* **40..71**
    pub normal_period_far_distance_range: std::ops::Range<u64>,

    // Zonenkaff
    /// Zones that are connected to fewer than 6 zones should give more points depending on how
    /// many fewer than 6 there are.
    /// *Recommended Value:* **15**
    pub points_per_connected_zone_less_than_6: u64,
    /// Zones should give more points depending on how badly they are connected. The bad
    /// connectivity index i is calculated with the number of connections the zone has c using the
    /// formula $i = 6 - sqrt{c}$.
    /// *Recommended Value:* **25**
    pub points_per_bad_connectivity_index: u64,
    /// Zones that have no train running through should give more points.
    /// *Recommended Value:* **30**
    pub points_for_no_train: u64,
    /// Zones that are very inaccessible for some miscellaneous reason should give more points.
    /// *Recommended Value:* **50**
    pub points_for_mongus: u64,

    // challenge generation
    /// The centre zone is the zone that the game converges to during its later hours. It should be
    /// the same as the one containing all the ZKaff challenges, so zone 110.
    /// *Recommended Value:* **110**
    pub centre_zone: u64,
    /// The ids of the challenge sets the game should be played with. Depends on the challenge sets
    /// present in the db, so check those before setting this value.
    /// *Recommended Value:* **¯\\\_\(ツ\)\_/¯**
    pub challenge_sets: Vec<u64>,
    /// How many challenges gatherers should be able to choose from. The game design, as well as
    /// the challenge selection algorithm are finely tuned for *3* challenges, but technically any
    /// value >= 2 should work.
    /// *Recommended Value:* **3**
    pub num_challenges: u64,
    /// Some normal period far challenges should be replaced with regionspecific ones at random.
    /// Determine the ratio/probability. The value should be between 0 and 1.
    /// *Recommended Value:* **0.4**
    pub regio_ratio: f64,
    /// During the ZKaff period, depending how deep into the zkaff period, a certain fraction of
    /// specific challenges generated should be ZKaff. At the beginning, only a small fraction and
    /// at the end a large fraction. A fraction above 1 means that 100% of challenges will be ZKaff
    /// before the end of the period already.
    /// *Recommended Value:* **0.2..1.2**
    pub zkaff_ratio_range: std::ops::Range<f64>,

    // miscellaneous game options
    /// How many players should be hunters. This depends on how many players participate. Around
    /// 40% is recommended and 35% if less experienced teams are present.
    /// *Recommended Value:* **~40% of the player count (rounded down)**
    pub num_catchers: u64,
    /// The ZVV zone in which the game starts.
    /// *Recommended Value:* **Zone in which the game starts (probably 110)**
    pub start_zone: u64,
    /// Whenever a team catches another, they should be invincible and invisible for some amount of
    /// time, so that they don't immediately get caught again and have time to regroup and move
    /// away.
    /// *Recommended Value:* **15 minutes**
    pub grace_period_duration: chrono::TimeDelta,

    // Bounty system
    /// When a team becomes gatherer, they should start off with a certain bounty. Or not, we have
    /// found that it makes more sense if they don't have any starting bounty.
    /// *Recommended Value:* **0**
    pub bounty_base_points: u64,
    /// When a team starts as hunter, they are immediately disadvantaged and should therefore
    /// receive some amount of points in compensation.
    /// *Recommended Value:* **500**
    pub bounty_start_points: u64,
    /// Whenever a team completes a challenge, their bounty should increase, so that they become a
    /// more attractive target. That amount should be some fraction of the points they earn from
    /// completing the challenge.
    /// *Recommended Value:* **0.3**
    pub bounty_percentage: f64,

    // Times
    /// The planned game start time. Probably in the morning, depends on the planning.
    /// *Recommended Value:* **Whenever you start the game, we often do 09:00**
    pub start_time: chrono::NaiveTime,
    /// The planned game end time. Probably in the evening, depends on the planning.
    /// *Recommended Value:* **Whenever you want to end the game, we often do 17:00**
    pub end_time: chrono::NaiveTime,
    /// Right at the start of the game, all challenges should be specific, so that the teams spread
    /// out. Thus, for a certain amount of minutes after the start of the game, the specific period
    /// takes place. It shouldn't be too short in case a team gets a challenge that is at the
    /// starting location.
    /// *Recommended Value:* **15**
    pub specific_minutes: u64,
    /// For a certain amount of minutes before the ZKaff period starts, the perimeter period should
    /// get teams out of the far flung corners of the map and towards the centre.
    /// *Recommended Value:* **90**
    pub perimeter_minutes: u64,
    /// For a certain amount of minutes before the end game period, the ZKaff period should slowly
    /// transition the specific challenges to ZKaff challenges.
    /// *Recommended Value:* **120**
    pub zkaff_minutes: u64,
    /// For a certain amount of minutes right before the end of the game, more challenges should be
    /// unspecific, so that most teams still have a challenge that is possible for them.
    /// *Recommended Value:* **45**
    pub end_game_minutes: u64,
    /// In order to prevent players (and especially organisers) from planning around the challenge
    /// generation periods, their borders should be reandomised by a certain amount of minutes.
    /// That means that any period may start that amount of minutes before or after the configured
    /// time. That does not affect the specific period.
    /// *Recommended Value:* **7**
    pub time_wiggle_minutes: u64,

    // Fallback Defaults
    /// If all else fails during challenge generation, there should be some sort of a fallback
    /// title for the challenge.
    /// *Recommended Value:* **[Kreative Titel]**
    pub default_challenge_title: String,
    /// If all else fails during challenge generation, there should be some sort of a fallback
    /// description for the challenge.
    /// *Recommended Value:* **Ihr händ Päch, die Challenge isch leider unlösbar. Ihr müend e
    /// anderi uswähle.**
    pub default_challenge_description: String,

    // additional options
    /// The teams should all have a unique colour. If no colour is specified for a team, they
    /// should have assigned a default colour to them.
    /// *Recommended Value:* **At least 8 different colours**
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
            fixed_cutoff_mult: 2.0,
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
            bounty_base_points: 0,
            bounty_start_points: 500,
            bounty_percentage: 0.3,
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

impl From<Config> for GameConfig {
    fn from(value: Config) -> Self {
        Self {
            num_catchers: value.num_catchers,
            start_zone: value.start_zone,
            start_time: value.start_time,
            end_time: value.end_time,
            challenge_sets: value.challenge_sets,
        }
    }
}

impl From<PartialGameConfig> for PartialConfig {
    fn from(value: PartialGameConfig) -> Self {
        Self {
            num_catchers: value.num_catchers,
            start_zone: value.start_zone,
            start_time: value.start_time,
            end_time: value.end_time,
            challenge_sets: value.challenge_sets,
            ..Default::default()
        }
    }
}

/// Some bonsaidb thing to make the db work
#[derive(Schema)]
#[schema(name="engine", collections=[Session, PlayerEntry, ChallengeEntry, ZoneEntry, PastGame, PictureEntry, ChallengeSetEntry])]
struct EngineSchema {}

#[derive(Debug, Collection, Serialize, Deserialize, Clone)]
#[collection(name = "picture")]
enum PictureEntry {
    Profile { thumb: RawPicture, full: RawPicture },
    ChallengePicture(RawPicture),
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

        let small = image
            .crop_imm(x, y, width, height)
            .resize(256, 256, FilterType::CatmullRom);

        Ok(Self::Profile {
            thumb: small.try_into()?,
            full: image.try_into()?,
        })
    }

    fn new_challenge_picture(picture: RawPicture) -> Self {
        Self::ChallengePicture(picture)
    }
}

/// The representation of a player in the db
#[derive(Debug, Collection, Serialize, Deserialize, Clone)]
#[collection(name = "player")]
pub struct PlayerEntry {
    #[serde(default)]
    picture: Option<u64>,
    name: String,
    passphrase: String,
    discord_id: Option<u64>,
    session: Option<u64>,
}

impl PlayerEntry {
    /// Converts the engine-internal `PlayerEntry` type into a sendable truinlag `Player` type
    pub fn to_sendable(&self, id: u64) -> truinlag::Player {
        truinlag::Player {
            name: self.name.clone(),
            session: self.session,
            id,
            picture_id: self.picture,
        }
    }
}

/// The representation of a zone in the db
#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "zone")]
pub struct ZoneEntry {
    /// The zone number, i.e. `110`
    pub zone: u64,
    pub num_conn_zones: u64,
    pub num_connections: u64,
    /// Whether a train passes through the zone, i.e. whether you can get from one ZVV zone to
    /// another ZVV zone through this one by train.
    pub train_through: bool,
    /// This is true for a few zones that are especially annoying to reach. Generally is set if the
    /// zone has no trains.
    pub mongus: bool,
    /// Is set for zones that contain the terminal stop of a line which has its other terminal stop
    /// within the ZVV and runs completely within the ZVV. Used for a challenge.
    pub s_bahn_zone: bool,
    /// Travel duration to every other zone. Key is the target zone id and value is the time in
    /// minutes required to travel there.
    pub minutes_to: HashMap<u64, u64>,
}

impl ZoneEntry {
    /// Converts the engine-internal `ZoneEntry` type into a sendable truinlag `Zone` type
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

    /// Calculates the zone's zonic kaffness based on a config.
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

    pub fn zones_with_distance(&self, range: Range<u64>) -> Vec<u64> {
        self.minutes_to
            .iter()
            .filter_map(|(to, t)| if range.contains(t) { Some(*to) } else { None })
            .collect()
    }
}

/// The representation of a running game in the db
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InGame {
    name: String,
    start_time: chrono::DateTime<chrono::Local>,
    mode: Mode,
    #[serde(default)]
    timer: TimerHook,
}

impl InGame {
    /// Converts the engine-internal `InGame` type into a sendable truinlag `Game` type
    pub fn to_sendable(&self) -> truinlag::Game {
        truinlag::Game {
            name: self.name.clone(),
            date: self.start_time.date_naive(),
            mode: self.mode,
        }
    }
}

/// The representation of a past game in the db
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

    /// Create a past game from a running game with the assumption that it is ending as the
    /// function is called.
    fn new_now(game: InGame, teams: Vec<TeamEntry>) -> Self {
        Self::new(game, teams, chrono::Local::now())
    }
}

/// The representation of a team that particitated in a past game in the db
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
    start_location: Option<(f32, f32)>,
    end_location: Option<(f32, f32)>,
    end_challenges: Vec<InOpenChallenge>,
    periods: Vec<PastPeriod>,
}

/// The representation of a thing that a team that particitated in a past game once did in the db
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PastPeriod {
    pub context: PeriodContext,
    pub end_time: chrono::DateTime<chrono::Local>,
    pub end_location: (f32, f32),
}

impl From<TeamEntry> for PastTeam {
    fn from(value: TeamEntry) -> Self {
        let start_location = value.locations.first();
        let start_location = start_location.map(|v| (v.latitude, v.longitude));
        let end_location = value.locations.last();
        let end_location = end_location.map(|v| (v.latitude, v.longitude));
        Self {
            name: value.name,
            players: value.players,
            discord_channel: value.discord_channel,
            end_role: value.role,
            colour: value.colour,
            points: value.points,
            bounty: value.bounty,
            end_zone_id: value.current_zone_id,
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
                        .get(p.location_end_index)
                        .map(|l| (l.latitude, l.longitude))
                        .unwrap_or((0.0, 0.0)),
                })
                .collect(),
        }
    }
}
