use super::{
    add_into,
    challenge::{ChallengeEntry, ChallengeSetEntry},
    runtime::{
        InternEngineCommand, InternEngineResponse, InternEngineResponsePackage, RuntimeRequest,
    },
    session::Session,
    DBEntry, EngineSchema, PastGame, PictureEntry, PlayerEntry, ZoneEntry,
};
use bonsaidb::{
    core::{
        connection::StorageConnection, document::Header, schema::SerializedCollection,
        transaction::Transaction,
    },
    local::{
        config::{self, Builder},
        Database, Storage,
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

    sessions: Vec<DBEntry<Session>>,
    challenges: Vec<DBEntry<ChallengeEntry>>,
    challenge_sets: Vec<DBEntry<ChallengeSetEntry>>,
    zones: Vec<DBEntry<ZoneEntry>>,
    players: Vec<DBEntry<PlayerEntry>>,

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
                    Some(id) => match self.sessions.iter_mut().find(|s| s.id == id) {
                        Some(session) => session.contents.vroom(
                            command.action,
                            id,
                            &self.players,
                            &self.challenges,
                            &self.zones,
                        ),
                        None => Error(NotFound).into(),
                    },
                    None => self.handle_action(command.action),
                }
            }

            InternEngineCommand::ChallengesCleared { leftovers } => {
                if leftovers.is_empty() {
                    Success.into()
                } else {
                    self.challenges = leftovers;
                    Error(InternalError).into()
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
                    self.autosave_in_progress.store(true, Ordering::Relaxed);
                    let players = self.players.clone();
                    let db = self.db.clone();
                    let sessions = self.sessions.clone();
                    let challenges = self.challenges.clone();
                    let challenge_sets = self.challenge_sets.clone();
                    let zones = self.zones.clone();
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
                                vec_overwrite_in_transaction(players, &mut transaction).unwrap();
                                vec_overwrite_in_transaction(sessions, &mut transaction).unwrap();
                                vec_overwrite_in_transaction(challenges, &mut transaction).unwrap();
                                vec_overwrite_in_transaction(challenge_sets, &mut transaction)
                                    .unwrap();
                                vec_overwrite_in_transaction(zones, &mut transaction).unwrap();

                                match transaction.apply_async(&db.to_async()).await {
                                    Ok(_) => println!(
                                        "Engine Autosave: autosave succeeded in {} ms",
                                        now.elapsed().as_millis(),
                                    ),
                                    Err(err) => {
                                        eprintln!("Engine Autosave: AUTOSAVE FAILED HIGH ALERT YOU ARE ALL FUCKED NOW (in {} ms): {}", now.elapsed().as_millis(), err);
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
        add_into(
            &mut self.zones,
            ZoneEntry {
                zone,
                num_conn_zones,
                num_connections,
                train_through,
                mongus,
                s_bahn_zone,
                minutes_to: HashMap::new(),
            },
        );
        Success.into()
    }

    fn add_minutes_to(
        &mut self,
        from_zone: u64,
        to_zone: u64,
        minutes: u64,
    ) -> InternEngineResponsePackage {
        if !self.zones.iter().any(|z| z.id == to_zone) {
            Error(NotFound).into()
        } else {
            match self.zones.iter_mut().find(|z| z.id == from_zone) {
                None => Error(NotFound).into(),
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
                .iter()
                .filter_map(|c| {
                    c.contents
                        .to_sendable(c.id, &self.challenge_sets, &self.zones)
                        .ok()
                })
                .collect(),
        )
        .into()
    }

    fn set_raw_challenge(&mut self, challenge: InputChallenge) -> InternEngineResponsePackage {
        match challenge.id {
            Some(id) => match self.challenges.iter_mut().find(|c| c.id == id) {
                None => Error(NotFound).into(),
                Some(c) => {
                    c.contents = challenge.clone().into();
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
        let db = self.db.clone();
        let challenges = std::mem::take(&mut self.challenges);
        let autosave_in_progress = self.autosave_in_progress.clone();
        let autosave_done = self.autosave_done.clone();
        InternEngineResponse::DelayedLoopback(tokio::spawn(async move {
            if autosave_in_progress.load(Ordering::Relaxed) {
                autosave_done.notified().await;
            }
            let query = ChallengeEntry::all_async(&db.to_async()).await;
            match query {
                Ok(all) => {
                    let mut transaction = Transaction::new();
                    for entry in all {
                        entry.delete_in_transaction(&mut transaction).unwrap();
                    }
                    match transaction.apply_async(&db.to_async()).await {
                        Ok(_) => {
                            InternEngineCommand::ChallengesCleared { leftovers: Vec::with_capacity(0) }
                        }
                        Err(err) => {
                            eprintln!("Engine: coudln't clear challenges from db: {}", err);
                            InternEngineCommand::ChallengesCleared { leftovers: challenges }
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "Engine: couldn't retrieve challenges from db while clearing challenges: {}",
                        err
                    );
                    InternEngineCommand::ChallengesCleared { leftovers: challenges }
                }
            }
        })).into()
    }

    fn get_player_by_passphrase(&self, passphrase: String) -> InternEngineResponsePackage {
        let doc = self
            .players
            .iter()
            .filter(|p| p.contents.passphrase == passphrase);
        match doc.count() {
            0 => Error(NotFound).into(),
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

    fn add_raw_challenge(&mut self, challenge: InputChallenge) -> InternEngineResponsePackage {
        let entry: ChallengeEntry = challenge.clone().into();
        add_into(&mut self.challenges, entry);
        Success.into()
    }

    fn add_session(&mut self, name: String, mode: Mode) -> InternEngineResponsePackage {
        if self.sessions.iter().any(|s| s.contents.name == name) {
            ResponseAction::Error(commands::Error::AlreadyExists).into()
        } else {
            add_into(&mut self.sessions, Session::new(name, mode));
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
        if self
            .players
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
                },
            );
            Success.into()
        }
    }

    fn set_player_session(
        &mut self,
        player: u64,
        session: Option<u64>,
    ) -> InternEngineResponsePackage {
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
                            if self.sessions.iter().any(|s| s.id == session) {
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
                                Error(NotFound).into()
                            }
                        }
                    }
                }
            }
        }
    }

    fn set_player_name(&mut self, player: u64, name: String) -> InternEngineResponsePackage {
        match self.players.iter_mut().find(|p| p.id == player) {
            None => Error(NotFound).into(),
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
        match self.players.iter_mut().find(|p| p.id == player) {
            None => Error(NotFound).into(),
            Some(player) => {
                player.contents.passphrase = passphrase;
                Success.into()
            }
        }
    }

    fn remove_player(&mut self, player: u64) -> InternEngineResponsePackage {
        // Note that this doesn't actually remove the player from the db, it just
        // removes all references to them from all sessions and removes their
        // passphrase.
        match self.players.iter_mut().find(|p| p.id == player) {
            None => Error(NotFound).into(),
            Some(p) => {
                p.contents.passphrase = "".into();
                self.sessions.iter_mut().for_each(|s| {
                    s.contents
                        .teams
                        .iter_mut()
                        .for_each(|t| t.players.retain(|p| p != &player))
                });
                Success.into()
            }
        }
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
            .iter()
            .map(|s| s.contents.to_sendable(s.id))
            .collect();
        let players = self
            .players
            .iter()
            .map(|p| p.contents.to_sendable(p.id))
            .collect();
        SendGlobalState { sessions, players }.into()
    }

    fn add_challenge_set(&mut self, name: String) -> InternEngineResponsePackage {
        if self.challenge_sets.iter().any(|s| s.contents.name == name) {
            Error(AlreadyExists).into()
        } else {
            add_into(&mut self.challenge_sets, ChallengeSetEntry { name });
            Success.into()
        }
    }

    fn get_challenge_sets(&self) -> InternEngineResponsePackage {
        SendChallengeSets(
            self.challenge_sets
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
