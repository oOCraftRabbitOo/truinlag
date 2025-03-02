use crate::{
    challenge::{ChallengeEntry, ChallengeSetEntry},
    runtime::{
        InternEngineCommand, InternEngineResponse, InternEngineResponsePackage, RuntimeRequest,
    },
    session::Session,
    ClonedDBEntry, DBMirror, EngineSchema, PastGame, PictureEntry, PlayerEntry, ZoneEntry,
};
use bonsaidb::{
    core::{
        connection::StorageConnection, document::Header, schema::SerializedCollection,
        transaction::Transaction,
    },
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

#[allow(dead_code)]
pub struct Engine {
    db: Database,
    changes_since_save: bool,

    // sessions: Vec<DBEntry<Session>>,
    // challenges: Vec<DBEntry<ChallengeEntry>>,
    // challenge_sets: Vec<DBEntry<ChallengeSetEntry>>,
    // zones: Vec<DBEntry<ZoneEntry>>,
    // players: Vec<DBEntry<PlayerEntry>>,
    sessions: DBMirror<Session>,
    challenges: DBMirror<ChallengeEntry>,
    challenge_sets: DBMirror<ChallengeSetEntry>,
    zones: DBMirror<ZoneEntry>,
    players: DBMirror<PlayerEntry>,

    pictures: Vec<Header>,
    past_games: Vec<Header>,

    autosave_in_progress: Arc<AtomicBool>,
    autosave_done: Arc<Notify>,
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

        let challenges = DBMirror::from_db(&db);
        let challenge_sets = DBMirror::from_db(&db);
        let zones = DBMirror::from_db(&db);
        let sessions = DBMirror::from_db(&db);
        let players = DBMirror::from_db(&db);

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
            autosave_in_progress: Arc::new(AtomicBool::new(false)),
            autosave_done: Arc::new(Notify::new()),
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

    pub fn vroom(&mut self, command: InternEngineCommand) -> InternEngineResponsePackage {
        match command {
            InternEngineCommand::Command(command) => {
                self.changes_since_save = true;
                match command.session {
                    Some(id) => match self.sessions.get_mut(id) {
                        Some(session) => session.contents.vroom(
                            command.action,
                            id,
                            &self.players,
                            &self.challenges,
                            &self.zones,
                        ),
                        None => Error(NotFound(format!("session with id {}", id))).into(),
                    },
                    None => self.handle_action(command.action),
                }
            }

            InternEngineCommand::AutoSave => {
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
                if self.changes_since_save {
                    self.autosave_in_progress.store(true, Ordering::Relaxed);
                    let db = self.db.to_async();

                    let player_changes = self.players.extract_changes();
                    self.players.clear_pending_deletions();
                    let _ = self.players.extract_deletions(); // We never delete players
                                                              //
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

                    self.changes_since_save = false;

                    let autosave_in_progress = self.autosave_in_progress.clone();
                    let autosave_done = self.autosave_done.clone();

                    InternEngineResponsePackage {
                        response: Success.into(),
                        runtime_requests: Some(vec![RuntimeRequest::RawLoopback(tokio::spawn(
                            async move {
                                println!("Engine Autosave: starting autosave");
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

                                match transaction.apply_async(&db).await {
                                    Ok(_) => println!(
                                        "Engine Autosave: autosave succeeded in {} ms",
                                        now.elapsed().as_millis(),
                                    ),
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
                                sleep(Duration::from_secs(3)).await;
                                InternEngineCommand::AutoSave
                            },
                        ))]),
                    }
                } else {
                    RuntimeRequest::CreateTimer {
                        duration: Duration::from_secs(3),
                        payload: InternEngineCommand::AutoSave,
                    }
                    .into()
                }
            }
        }
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
        //         let db = self.db.clone();
        // let challenges = std::mem::take(&mut self.challenges);
        // let autosave_in_progress = self.autosave_in_progress.clone();
        // let autosave_done = self.autosave_done.clone();
        // InternEngineResponse::DelayedLoopback(tokio::spawn(async move {
        //     if autosave_in_progress.load(Ordering::Relaxed) {
        //         autosave_done.notified().await;
        //     }
        //     let query = ChallengeEntry::all_async(&db.to_async()).await;
        //     match query {
        //         Ok(all) => {
        //             let mut transaction = Transaction::new();
        //             for entry in all {
        //                 entry.delete_in_transaction(&mut transaction).unwrap();
        //             }
        //             match transaction.apply_async(&db.to_async()).await {
        //                 Ok(_) => {
        //                     InternEngineCommand::ChallengesCleared { leftovers: Vec::with_capacity(0) }
        //                 }
        //                 Err(err) => {
        //                     eprintln!("Engine: coudln't clear challenges from db: {}", err);
        //                     InternEngineCommand::ChallengesCleared { leftovers: challenges }
        //                 }
        //             }
        //         }
        //         Err(err) => {
        //             eprintln!(
        //                 "Engine: couldn't retrieve challenges from db while clearing challenges: {}",
        //                 err
        //             );
        //             InternEngineCommand::ChallengesCleared { leftovers: challenges }
        //         }
        //     }
        // })).into()
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

    fn handle_action(&mut self, action: EngineAction) -> InternEngineResponsePackage {
        match action {
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
            GenerateTeamChallenges(_) => Error(NoSessionSupplied).into(),
            AddChallengeToTeam {
                team: _,
                challenge: _,
            } => Error(NoSessionSupplied).into(),
            RenameTeam {
                team: _,
                new_name: _,
            } => Error(NoSessionSupplied).into(),
        }
    }
}
