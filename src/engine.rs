use crate::{
    challenge::{ChallengeEntry, ChallengeSetEntry},
    runtime::{
        InternEngineCommand, InternEngineResponse, InternEngineResponsePackage, RuntimeRequest,
    },
    session::Session,
    ClonedDBEntry, DBMirror, EngineContext, EngineSchema, PastGame, PictureEntry, PlayerEntry,
    TimerTracker, ZoneEntry,
};
use bonsaidb::{
    core::{connection::StorageConnection, schema::SerializedCollection, transaction::Transaction},
    local::{
        config::{self, Builder},
        AsyncDatabase, Database, Storage,
    },
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{
    sync::Notify,
    time::{sleep, Duration},
};
use truinlag::{
    commands::{
        BroadcastAction::*, EngineAction::*, EngineResponse, Error::*, ResponseAction::*, *,
    },
    *,
};

/// A helper function for engine autosaves that provides a shorthand for overwriting multiple
/// entries in a transaction.
fn vec_overwrite_in_transaction<T>(
    entries: Vec<ClonedDBEntry<T>>,
    transaction: &mut Transaction,
) -> Result<(), bonsaidb::core::Error>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64> + 'static,
    T: Clone,
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

/// A helper function for engine autosaves that provides a shorthand for deleting multiple entries
/// in a transaction.
async fn vec_delete_in_transaction<T>(
    entries: Vec<u64>,
    transaction: &mut Transaction,
    db: &AsyncDatabase,
) -> Result<(), bonsaidb::core::Error>
where
    T: SerializedCollection<Contents = T, PrimaryKey = u64> + 'static,
    T: Clone,
{
    for entry in T::get_multiple_async(&entries, db).await? {
        entry.delete_in_transaction(transaction)?;
    }
    Ok(())
}

/// The engine is the core of truinlag that handles all requests and keeps tabs on the database.
///
/// # In the abstract
///
/// TrainLag is a game where, at the core, all commands have to be executed sequentially. For
/// example, if one team catches another, it must be ensured that that operation is fully completed
/// before anything else happens, like a different team trying to catch that same team. Otherwise,
/// undefined behaviour may occur. At the same time, TrainLag is asynchronous, as many different
/// clients are connected to truinlag at the same. The engine is meant to handle one request after
/// another according to my specially cooked request-response-broadcast model, while the entirety
/// of the runtime exists solely to accomodate the engine, such that the engine never has to wait.
///
/// # In practice
///
/// The engine has to manage the database, which stores the current game state and player
/// information and such, but accessing it is way too slow for practical use. Thus, the engine
/// holds mirrors of frequently accessed database collections in memory and periodically autosaves
/// them. Otherwise, it just holds the database connection and some control data, like
/// `changes_since_save`.
#[allow(dead_code)]
pub struct Engine {
    db: Database,
    changes_since_save: bool, // technically redundant, I think

    sessions: DBMirror<Session>,
    challenges: DBMirror<ChallengeEntry>,
    challenge_sets: DBMirror<ChallengeSetEntry>,
    zones: DBMirror<ZoneEntry>,
    players: DBMirror<PlayerEntry>,
    past_games: DBMirror<PastGame>,
    pictures: DBMirror<PictureEntry>,

    // pictures: Vec<u64>,
    autosave_in_progress: Arc<AtomicBool>,
    autosave_done: Arc<Notify>,
    timer_tracker: TimerTracker,
}

impl Engine {
    /// Initialises the engine and loads the database from `storage_path`.
    ///
    /// # Panics
    ///
    /// This function panics on database errors, like when a corrupted database is found.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut engine = engine::Engine::init(std::Path::new("truintabase"));
    /// let engine_response = engine.vroom(
    ///     InternEngineCommand::Command(
    ///         truinlag::commands::EngineCommand::GetState
    ///     )
    /// );
    /// ```
    pub fn init(storage_path: &Path) -> Self {
        println!("Engine: initialising...");
        println!("Engine: opening database...");
        let db = Storage::open(
            config::StorageConfiguration::new(storage_path)
                .with_schema::<EngineSchema>()
                .unwrap(),
        )
        .unwrap()
        .create_database::<EngineSchema>("engine", true)
        .unwrap();

        // let pictures = PictureEntry::all(&db)
        //     .query() // this is dogshit but I don't know what else to do
        //     .unwrap()
        //     .iter()
        //     .map(|doc| doc.header.id)
        //     .collect();

        println!("Engine: loading challenges...");
        let challenges = DBMirror::from_db(&db);
        println!("Engine: loading challenge sets...");
        let challenge_sets = DBMirror::from_db(&db);
        println!("Engine: loading zones...");
        let zones = DBMirror::from_db(&db);
        println!("Engine: loading sessions...");
        let sessions: DBMirror<Session> = DBMirror::from_db(&db);
        println!("Engine: loading players...");
        let players = DBMirror::from_db(&db);
        println!("Engine: loading past games...");
        let past_games = DBMirror::from_db(&db);
        println!("Engine: loading pictures...");
        let pictures = DBMirror::from_db(&db);
        println!("Engine: done!");

        // let sessionista = sessions.get_all();
        // for session in sessionista {
        //     println!("Size of session {}:", session.contents.name);
        //     let mut teams_size = 0;
        //     for team in &session.contents.teams {
        //         let mut team_size = 0;
        //         println!("Size of team {}:", team.name);
        //         let locations_size = team.locations.capacity() * size_of::<(f64, f64, NaiveTime)>();
        //         println!("Locations: {}", locations_size);
        //         let periods_size = team.periods.capacity() * size_of::<Period>();
        //         println!("Periods: {}", periods_size);
        //         team_size += locations_size + periods_size;
        //         team_size += size_of::<TeamEntry>();
        //         println!("Total: {}", team_size);
        //         teams_size += team_size;
        //         let mut size = size_of_val(team); // Stack part
        //         size += team.name.capacity(); // String heap
        //         size += team.players.capacity() * std::mem::size_of::<u64>();
        //         size += team.locations.capacity() * std::mem::size_of::<(f64, f64, NaiveTime)>();
        //         size += team.challenges.capacity() * std::mem::size_of::<InOpenChallenge>();
        //         size += team.periods.capacity() * std::mem::size_of::<Period>();
        //         println!("Other total: {}\n", size);
        //     }
        //     println!("\nTotal: {}", teams_size + size_of::<Session>());
        // }

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
            autosave_in_progress: Arc::new(AtomicBool::new(false)),
            autosave_done: Arc::new(Notify::new()),
            timer_tracker: TimerTracker::new(),
        }
    }

    /// This is used to initialise the autosaves, timers and alarms.
    ///
    /// The autosaves are handled in a way where the engine autosaves when it receives an autosave
    /// signal. After completing the save, it sends itself another autosave signal on a timer. This
    /// function provides an initial timer for an autosave signal and should therefore only be
    /// called once. This method will also restart all timers and alarms that may have been running
    /// before the last shutdown.
    pub fn setup(&mut self) -> InternEngineResponsePackage {
        let mut requests = Vec::new();
        // get requests from team grace periods and games
        for session in self.sessions.get_all() {
            requests.append(&mut session.contents.setup())
        }
        // set the timer_tracker id
        self.timer_tracker.current_id = requests
            .iter()
            .max_by(|&r1, &r2| r1.id().cmp(&r2.id()))
            .map(|r| r.id())
            .unwrap_or(0);
        // create the autosave timer
        requests.push(
            self.timer_tracker
                .timer(chrono::TimeDelta::seconds(2), InternEngineCommand::AutoSave)
                .0,
        );
        InternEngineResponsePackage {
            response: InternEngineResponse::DirectResponse(ResponseAction::Success.into()),
            runtime_requests: Some(requests),
        }
    }

    /// This function fulfills engine requests and thereby modifies the game state and returns
    /// responses and broadcasts.
    pub fn vroom(&mut self, command: InternEngineCommand) -> InternEngineResponsePackage {
        // The engine can receive many kinds of commands, some from the runtime (and itself, with a
        // delay for example) and some from actual clients. All cases are categorised within the
        // `InternEngineCommand` enum.
        let start_time = std::time::Instant::now();
        let response = match command {
            // These are cases from actual external clients. For readability, they are passed to
            // helper functions.
            InternEngineCommand::Command(command) => {
                self.changes_since_save = true;
                // There are global and session-specific commands. If a command has a session, then
                // the action is handled by the corresponding session (which is saved within the
                // engine).
                match command.session {
                    Some(id) => match self.sessions.get_mut(id) {
                        Some(session) => {
                            let context = EngineContext {
                                player_db: &self.players,
                                challenge_db: &self.challenges,
                                challenge_set_db: &self.challenge_sets,
                                zone_db: &self.zones,
                                past_game_db: &mut self.past_games,
                                picture_db: &mut self.pictures,
                                timer_tracker: &mut self.timer_tracker,
                            };
                            session.contents.vroom(command.action, id, context)
                        }
                        None => Error(NotFound(format!("session with id {}", id))).into(),
                    },
                    None => self.handle_action(command.action), // global action helper function
                }
            }

            InternEngineCommand::TeamLeftGracePeriod {
                session_id,
                team_id,
            } => {
                let context = EngineContext {
                    player_db: &self.players,
                    challenge_db: &self.challenges,
                    challenge_set_db: &self.challenge_sets,
                    zone_db: &self.zones,
                    past_game_db: &mut self.past_games,
                    picture_db: &mut self.pictures,
                    timer_tracker: &mut self.timer_tracker,
                };
                match self.sessions.get_mut(session_id) {
                    None => Success.into(), // = do nothing
                    Some(session) => session
                        .contents
                        .team_left_grace_period(team_id, session_id, context),
                }
            }

            InternEngineCommand::UploadedImages(pictures_added) => {
                UploadedPictures(pictures_added).into()
            }

            InternEngineCommand::MadeTeamProfile {
                session_id,
                team_id,
                pfp,
            } => match pfp {
                Ok(pfp) => match self.sessions.get_mut(session_id) {
                    Some(session) => match session.contents.teams.get_mut(team_id) {
                        Some(team) => {
                            team.picture = Some(self.pictures.add(pfp));
                            Success
                        }
                        None => Error(NotFound(format!("team with id {}", team_id))),
                    },
                    None => Error(NotFound(format!("session with id {}", session_id))),
                },
                Err(err) => Error(err),
            }
            .into(),

            InternEngineCommand::MadePlayerProfile { player_id, pfp } => match pfp {
                Ok(pfp) => match self.players.get_mut(player_id) {
                    Some(player) => {
                        player.contents.picture = Some(self.pictures.add(pfp));
                        Success
                    }
                    None => Error(NotFound(format!("player with id {player_id}"))),
                },
                Err(err) => Error(err),
            }
            .into(),

            InternEngineCommand::AutoSave => {
                if self.changes_since_save {
                    self.autosave_in_progress.store(true, Ordering::Relaxed); // might not be used
                    let db = self.db.to_async(); // the db is stored as blocking bc idk

                    // what follows here is some ugly code repetition, which i should probably get
                    // rid of at some point...
                    let player_changes = self.players.extract_changes();
                    self.players.clear_pending_deletions();
                    let _ = self.players.extract_deletions(); // We never delete players

                    let session_changes = self.sessions.extract_changes();
                    self.sessions.clear_pending_deletions();
                    let session_deletions = self.sessions.extract_deletions();

                    let challenge_changes = self.challenges.extract_changes();
                    self.challenges.clear_pending_deletions();
                    let challenge_deletions = self.challenges.extract_deletions();

                    let challenge_set_changes = self.challenge_sets.extract_changes();
                    self.challenge_sets.clear_pending_deletions();
                    let challenge_set_deletions = self.challenge_sets.extract_deletions();

                    let zone_changes = self.zones.extract_changes();
                    self.zones.clear_pending_deletions();
                    let zone_deletions = self.zones.extract_deletions();

                    let picture_changes = self.pictures.extract_changes();
                    self.pictures.clear_pending_deletions();
                    let picture_deletions = self.pictures.extract_deletions();

                    self.changes_since_save = false;

                    let autosave_in_progress = self.autosave_in_progress.clone();
                    let autosave_done = self.autosave_done.clone();

                    InternEngineResponsePackage {
                        response: Success.into(),
                        runtime_requests: Some(vec![RuntimeRequest::RawLoopback(tokio::spawn(
                            async move {
                                let now = tokio::time::Instant::now();
                                let mut transaction = Transaction::new();
                                vec_overwrite_in_transaction(player_changes, &mut transaction)
                                    .unwrap();
                                vec_overwrite_in_transaction(session_changes, &mut transaction)
                                    .unwrap();
                                vec_delete_in_transaction::<Session>(
                                    session_deletions,
                                    &mut transaction,
                                    &db,
                                )
                                .await
                                .unwrap();
                                vec_overwrite_in_transaction(challenge_changes, &mut transaction)
                                    .unwrap();
                                vec_delete_in_transaction::<ChallengeEntry>(
                                    challenge_deletions,
                                    &mut transaction,
                                    &db,
                                )
                                .await
                                .unwrap();
                                vec_overwrite_in_transaction(
                                    challenge_set_changes,
                                    &mut transaction,
                                )
                                .unwrap();
                                vec_delete_in_transaction::<ChallengeSetEntry>(
                                    challenge_set_deletions,
                                    &mut transaction,
                                    &db,
                                )
                                .await
                                .unwrap();
                                vec_overwrite_in_transaction(zone_changes, &mut transaction)
                                    .unwrap();
                                vec_delete_in_transaction::<ZoneEntry>(
                                    zone_deletions,
                                    &mut transaction,
                                    &db,
                                )
                                .await
                                .unwrap();
                                vec_overwrite_in_transaction(picture_changes, &mut transaction)
                                    .unwrap();
                                vec_delete_in_transaction::<PictureEntry>(
                                    picture_deletions,
                                    &mut transaction,
                                    &db,
                                )
                                .await
                                .unwrap();

                                match transaction.apply_async(&db).await {
                                    Ok(_) => {}
                                    Err(err) => {
                                        eprintln!(
                                            "Engine Autosave: \
                                            AUTOSAVE FAILED HIGH ALERT YOU ARE ALL FUCKED NOW \
                                            (in {} ms): {}",
                                            now.elapsed().as_millis(),
                                            err
                                        );
                                        panic!("autosave failed")
                                    }
                                }

                                autosave_in_progress.store(false, Ordering::Relaxed);
                                autosave_done.notify_waiters();
                                sleep(Duration::from_secs(1)).await;
                                InternEngineCommand::AutoSave
                            },
                        ))]),
                    }
                } else {
                    // manual timer creation, since automatically created timers get killed during
                    // shutdown
                    RuntimeRequest::RawLoopback(tokio::spawn(async {
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        InternEngineCommand::AutoSave
                    }))
                    .into()
                }
            }
        };
        let duration = start_time.elapsed();
        if duration.as_millis() > 10 {
            println!("A command took {}ms to complete.", duration.as_millis());
        }
        response
    }

    fn get_all_zones(&self) -> InternEngineResponsePackage {
        SendZones(
            self.zones
                .get_all()
                .iter()
                .map(|z| z.contents.to_sendable(z.id))
                .collect(),
        )
        .into()
    }

    fn add_zone(
        &mut self,
        zone: u64,
        num_conn_zones: u64,
        num_connections: u64,
        train_through: bool,
        mongus: bool,
        s_bahn_zone: bool,
    ) -> InternEngineResponsePackage {
        self.zones.add(ZoneEntry {
            zone,
            num_conn_zones,
            num_connections,
            train_through,
            mongus,
            s_bahn_zone,
            minutes_to: HashMap::new(),
        });
        Success.into()
    }

    fn add_minutes_to(
        &mut self,
        from_zone: u64,
        to_zone: u64,
        minutes: u64,
    ) -> InternEngineResponsePackage {
        if self.zones.get(to_zone).is_none() {
            Error(NotFound(format!("to zone with id {}", to_zone))).into()
        } else {
            match self.zones.get_mut(from_zone) {
                None => Error(NotFound(format!("from zone with id {}", from_zone))).into(),
                Some(entry) => {
                    entry.contents.minutes_to.insert(to_zone, minutes);
                    Success.into()
                }
            }
        }
    }

    fn get_raw_challenges(&self) -> InternEngineResponsePackage {
        SendRawChallenges(
            self.challenges
                .get_all()
                .iter()
                .filter_map(|c| {
                    c.contents
                        .to_sendable(c.id, &self.challenge_sets.get_all(), &self.zones.get_all())
                        .ok()
                })
                .collect(),
        )
        .into()
    }

    fn set_raw_challenge(&mut self, challenge: InputChallenge) -> InternEngineResponsePackage {
        match challenge.id {
            Some(id) => match self.challenges.get_mut(id) {
                None => Error(NotFound(format!("challenge with id {}", id))).into(),
                Some(c) => {
                    *c.contents = challenge.clone().into();
                    Success.into()
                }
            },
            None => Error(BadData(
                "the supplied challenge doesn't have an id. \
                    this can happen because the RawChallenge::new() method doesn't assign an id. \
                    challenges sent from truinlag have an id."
                    .into(),
            ))
            .into(),
        }
    }

    fn delete_all_challenges(&mut self) -> InternEngineResponsePackage {
        self.challenges.delete_all();
        Success.into()
    }

    fn get_player_by_passphrase(&self, passphrase: String) -> InternEngineResponsePackage {
        match self.players.find(|p| p.passphrase == passphrase) {
            None => Error(NotFound(format!("player with passphrase {}", passphrase))).into(),
            Some(player) => Player(player.contents.to_sendable(player.id)).into(),
        }
    }

    fn add_raw_challenge(&mut self, challenge: InputChallenge) -> InternEngineResponsePackage {
        let entry: ChallengeEntry = challenge.clone().into();
        self.challenges.add(entry);
        Success.into()
    }

    fn add_session(&mut self, name: String, mode: Mode) -> InternEngineResponsePackage {
        if self.sessions.any(|s| s.name == name) {
            ResponseAction::Error(commands::Error::AlreadyExists).into()
        } else {
            self.sessions.add(Session::new(name, mode));
            Success.into()
        }
    }

    fn add_player(
        &mut self,
        name: String,
        discord_id: Option<u64>,
        passphrase: String,
        session: Option<u64>,
    ) -> InternEngineResponsePackage {
        if self.players.any(|p| p.passphrase == passphrase) {
            Error(AlreadyExists).into()
        } else {
            self.players.add(PlayerEntry {
                picture: None,
                name,
                discord_id,
                passphrase,
                session,
            });
            Success.into()
        }
    }

    fn set_player_session(
        &mut self,
        player: u64,
        session: Option<u64>,
    ) -> InternEngineResponsePackage {
        match self.players.get_mut(player) {
            None => Error(NotFound(format!("player with id {}", player))).into(),
            Some(i_player) => {
                let old_session = i_player.contents.session;
                if old_session == session {
                    Success.into()
                } else {
                    if let Some(tbr_session) = old_session {
                        match self.sessions.get_mut(tbr_session) {
                            None => {
                                println!(
                                    "Engine: couldn't find session with id {} of player {}",
                                    tbr_session, i_player.id
                                )
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
                                }),
                            }
                            .into()
                        }
                        Some(session) => {
                            if self.sessions.get(session).is_some() {
                                i_player.contents.session = Some(session);
                                EngineResponse {
                                    response_action: Success,
                                    broadcast_action: Some(PlayerChangedSession {
                                        player: i_player.contents.to_sendable(i_player.id),
                                        from_session: old_session,
                                        to_session: Some(session),
                                    }),
                                }
                                .into()
                            } else {
                                Error(NotFound(format!("session with id {}", session))).into()
                            }
                        }
                    }
                }
            }
        }
    }

    fn set_player_name(&mut self, player: u64, name: String) -> InternEngineResponsePackage {
        match self.players.get_mut(player) {
            None => Error(NotFound(format!("player with id {}", player))).into(),
            Some(player) => {
                player.contents.name = name;
                Success.into()
            }
        }
    }

    fn set_player_passphrase(
        &mut self,
        player: u64,
        passphrase: String,
    ) -> InternEngineResponsePackage {
        match self.players.get_mut(player) {
            None => Error(NotFound(format!("player with id {}", player))).into(),
            Some(player) => {
                player.contents.passphrase = passphrase;
                Success.into()
            }
        }
    }

    fn remove_player(&mut self, _player: u64) -> InternEngineResponsePackage {
        // Note that this doesn't actually remove the player from the db, it just
        // removes all references to them from all sessions and removes their
        // passphrase.
        // NEVERMIND, this currently does nothing
        Error(NotImplemented).into()
    }

    fn ping(&self, payload: Option<String>) -> InternEngineResponsePackage {
        EngineResponse {
            response_action: Success,
            broadcast_action: Some(BroadcastAction::Pinged(payload)),
        }
        .into()
    }

    fn get_state(&self) -> InternEngineResponsePackage {
        let sessions = self
            .sessions
            .get_all()
            .iter()
            .map(|s| s.contents.to_sendable(s.id))
            .collect();
        let players = self
            .players
            .get_all()
            .iter()
            .map(|p| p.contents.to_sendable(p.id))
            .collect();
        SendGlobalState { sessions, players }.into()
    }

    fn add_challenge_set(&mut self, name: String) -> InternEngineResponsePackage {
        if self.challenge_sets.any(|s| s.name == name) {
            Error(AlreadyExists).into()
        } else {
            self.challenge_sets.add(ChallengeSetEntry { name });
            Success.into()
        }
    }

    fn get_challenge_sets(&self) -> InternEngineResponsePackage {
        SendChallengeSets(
            self.challenge_sets
                .get_all()
                .iter()
                .map(|s| s.contents.to_sendable(s.id))
                .collect(),
        )
        .into()
    }

    // this is code for if pictures are not copied into memory. It should be used in some capacity
    // again in the future, but for now it's out of commission.
    //     fn upload_pictures(&mut self, pictures: Vec<Picture>) -> InternEngineResponsePackage {
    //         let db = self.db.to_async();
    //         let picture_task = tokio::spawn(async move {
    //             let mut pictures_added = Vec::new();
    //             for pic in pictures {
    //                 match PictureEntry::ChallengePicture(pic)
    //                     .push_into_async(&db)
    //                     .await
    //                 {
    //                     Ok(doc) => pictures_added.push(doc.header.id),
    //                     Err(err) => {
    //                         eprintln!("Engine: error adding picture to db: {err}");
    //                     }
    //                 }
    //             }
    //             InternEngineCommand::UploadedImages(pictures_added)
    //         });
    //         InternEngineResponse::DelayedLoopback(picture_task).into()
    //     }

    fn upload_player_picture(
        &mut self,
        player_id: u64,
        picture: RawPicture,
    ) -> InternEngineResponsePackage {
        match self.players.get_mut(player_id) {
            Some(_player) => InternEngineResponse::DelayedLoopback(tokio::spawn(async move {
                let pfp = tokio::task::block_in_place(|| PictureEntry::new_profile(picture.into()))
                    .map_err(|err| {
                        eprintln!("Engine: couldn't convert picture: {}", err);
                        PictureProblem
                    });
                InternEngineCommand::MadePlayerProfile { player_id, pfp }
            }))
            .into(),
            None => Error(NotFound(format!("player with id {player_id}"))).into(),
        }
    }

    fn get_thumbnails(&self, ids: Vec<u64>) -> InternEngineResponsePackage {
        Pictures(
            ids.iter()
                .filter_map(|id| {
                    self.pictures.get(*id).and_then(|p| match p.contents {
                        PictureEntry::Profile { thumb, full: _ } => Some(Picture {
                            data: thumb.clone(),
                            is_thumbnail: true,
                            id: *id,
                        }),
                        PictureEntry::ChallengePicture(_) => None,
                    })
                })
                .collect(),
        )
        .into()
    }

    fn get_pictures(&self, ids: Vec<u64>) -> InternEngineResponsePackage {
        Pictures(
            ids.iter()
                .filter_map(|id| {
                    self.pictures.get(*id).map(|p| match p.contents {
                        PictureEntry::ChallengePicture(pic) => Picture {
                            data: pic.clone(),
                            is_thumbnail: false,
                            id: *id,
                        },
                        PictureEntry::Profile { thumb: _, full } => Picture {
                            data: full.clone(),
                            is_thumbnail: false,
                            id: *id,
                        },
                    })
                })
                .collect(),
        )
        .into()
    }

    fn handle_action(&mut self, action: EngineAction) -> InternEngineResponsePackage {
        match action {
            GetPictures(ids) => self.get_pictures(ids),
            GetThumbnails(ids) => self.get_thumbnails(ids),
            UploadPlayerPicture { player_id, picture } => {
                self.upload_player_picture(player_id, picture)
            }
            GetAllZones => self.get_all_zones(),
            AddZone {
                zone,
                num_conn_zones,
                num_connections,
                train_through,
                mongus,
                s_bahn_zone,
            } => self.add_zone(
                zone,
                num_conn_zones,
                num_connections,
                train_through,
                mongus,
                s_bahn_zone,
            ),
            AddMinutesTo {
                from_zone,
                to_zone,
                minutes,
            } => self.add_minutes_to(from_zone, to_zone, minutes),
            GetRawChallenges => self.get_raw_challenges(),
            SetRawChallenge(challenge) => self.set_raw_challenge(challenge),
            AddRawChallenge(challenge) => self.add_raw_challenge(challenge),
            DeleteAllChallenges => self.delete_all_challenges(),
            GetPlayerByPassphrase(passphrase) => self.get_player_by_passphrase(passphrase),
            AddSession { name, mode } => self.add_session(name, mode),
            AddPlayer {
                name,
                discord_id,
                passphrase,
                session,
            } => self.add_player(name, discord_id, passphrase, session),
            SetPlayerSession { player, session } => self.set_player_session(player, session),
            SetPlayerName { player, name } => self.set_player_name(player, name),
            SetPlayerPassphrase { player, passphrase } => {
                self.set_player_passphrase(player, passphrase)
            }
            RemovePlayer { player } => self.remove_player(player),
            Ping(payload) => self.ping(payload),
            GetState => self.get_state(),
            AddChallengeSet(name) => self.add_challenge_set(name),
            GetChallengeSets => self.get_challenge_sets(),
            Start => Error(NoSessionSupplied).into(),
            Stop => Error(NoSessionSupplied).into(),
            Catch {
                catcher: _,
                caught: _,
                period_id: _,
            } => Error(NoSessionSupplied).into(),
            Complete {
                completer: _,
                completed: _,
                period_id: _,
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
            GenerateTeamChallenges(_) => Error(NoSessionSupplied).into(),
            AddChallengeToTeam {
                team: _,
                challenge: _,
            } => Error(NoSessionSupplied).into(),
            RenameTeam {
                team: _,
                new_name: _,
            } => Error(NoSessionSupplied).into(),
            GetEvents => Error(NoSessionSupplied).into(),
            UploadTeamPicture {
                team_id: _,
                picture: _,
            } => Error(NoSessionSupplied).into(),
            UploadPeriodPictures {
                pictures: _,
                team: _,
                period: _,
            } => Error(NoSessionSupplied).into(),
            GetLocations => Error(NoSessionSupplied).into(),
            GetPastLocations {
                team_id: _,
                of_past_seconds: _,
            } => Error(NoSessionSupplied).into(),
            GetGameConfig => Error(NoSessionSupplied).into(),
            SetGameConfig(_) => Error(NoSessionSupplied).into(),
        }
    }
}
