use crate::{
    challenge::{ChallengeEntry, InOpenChallenge},
    runtime::{InternEngineCommand, InternEngineResponsePackage, RuntimeRequest},
    team::{PeriodContext, TeamEntry},
    Config, DBEntry, DBMirror, InGame, PartialConfig, PlayerEntry, ZoneEntry,
};
use bonsaidb::core::schema::Collection;
use geo::GeodesicDistance;
use partially::Partial;
use rand::{seq::IteratorRandom, thread_rng};
use serde::{Deserialize, Serialize};
use strsim::normalized_damerau_levenshtein as strcmp;
use truinlag::{
    commands::{BroadcastAction::*, EngineAction::*, Error::*, ResponseAction::*, *},
    *,
};

#[derive(Debug, Clone, Collection, Serialize, Deserialize)]
#[collection(name = "session")]
pub struct Session {
    pub name: String,
    pub teams: Vec<TeamEntry>,
    pub mode: Mode,
    pub config: PartialConfig,
    pub discord_server_id: Option<u64>,
    pub discord_game_channel: Option<u64>,
    pub discord_admin_channel: Option<u64>,
    pub game: Option<InGame>,
}

impl Session {
    pub fn config(&self) -> Config {
        let mut cfg = Config::default();
        cfg.apply_some(self.config.clone());
        cfg
    }

    pub fn new(name: String, mode: Mode) -> Self {
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

    pub fn to_sendable(&self, id: u64) -> GameSession {
        GameSession {
            name: self.name.clone(),
            mode: self.mode,
            id,
        }
    }

    fn generate_team_challenges(
        &mut self,
        id: usize,
        challenge_entries: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
    ) -> InternEngineResponsePackage {
        let config = self.config();
        match self.teams.get_mut(id) {
            None => Error(NotFound(format!("team with id {}", id))).into(),
            Some(team) => {
                team.generate_challenges(&config, challenge_entries, zone_entries);
                Success.into()
            }
        }
    }

    fn add_challenge_to_team(
        &mut self,
        team: usize,
        challenge: Challenge,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(team) {
            // THIS METHOD SHOULD BE TEMPORARY AND EXISTS ONLY FOR TESTING PURPOSES
            None => Error(NotFound(format!("team with id {}", team))).into(),
            Some(team) => {
                if self.game.is_some() {
                    Error(GameInProgress).into()
                } else {
                    team.challenges.push(InOpenChallenge {
                        title: challenge.title,
                        description: challenge.description,
                        points: challenge.points,
                        action: None,
                        zone: None,
                        id: 0,
                    });
                    Success.into()
                }
            }
        }
    }

    fn rename_team(&mut self, team: usize, new_name: String) -> InternEngineResponsePackage {
        match self.teams.get_mut(team) {
            None => Error(NotFound(format!("team with id {}", team))).into(),
            Some(team) => {
                team.name = new_name;
                Success.into()
            }
        }
    }

    fn make_team_catcher(
        &mut self,
        id: usize,
        player_entries: &DBMirror<PlayerEntry>,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound(format!("team with id {}", id))).into(),
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
        }
    }

    fn make_team_runner(
        &mut self,
        id: usize,
        player_entries: &DBMirror<PlayerEntry>,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound(format!("team with id {}", id))).into(),
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
        }
    }

    fn send_location(&mut self, player: u64, location: (f64, f64)) -> InternEngineResponsePackage {
        match self
            .teams
            .iter()
            .position(|t| t.players.iter().all(|&p| p == player))
        {
            None => Error(NotFound(format!("team with the player with id {}", player))).into(),
            Some(team) => {
                self.teams[team].locations.push((
                    location.0,
                    location.1,
                    chrono::offset::Local::now().time(),
                ));
                EngineResponse {
                    response_action: Success,
                    broadcast_action: Some(Location { team, location }),
                }
                .into()
            }
        }
    }

    fn assign_player_to_team(
        &mut self,
        player: u64,
        team: Option<usize>,
        session_id: u64,
    ) -> InternEngineResponsePackage {
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
                None => Error(NotFound(format!("team with id {}", t))).into(),
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

    fn catch(
        &mut self,
        catcher: usize,
        caught: usize,
        challenge_entries: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        player_entries: &DBMirror<PlayerEntry>,
    ) -> InternEngineResponsePackage {
        match self.game {
            Some(_) => {
                if caught == catcher {
                    return Error(BadData("a team cannot catch itself".into())).into();
                }
                let bounty;
                let config = self.config();
                let broadcast;
                match self.teams.get(catcher) {
                    Some(catcher_team) => match catcher_team.role {
                        TeamRole::Catcher => match self.teams.get(caught) {
                            Some(caught_team) => match caught_team.role {
                                TeamRole::Runner => {
                                    if caught_team.location().is_some()
                                        && catcher_team.location().is_some()
                                    {
                                        let caught_location =
                                            geo::Point::from(caught_team.location().unwrap());
                                        let catcher_location =
                                            geo::Point::from(catcher_team.location().unwrap());
                                        if caught_location.geodesic_distance(&catcher_location)
                                            > 300.0
                                        {
                                            return Error(TeamsTooFar).into();
                                        }
                                    }
                                    bounty = caught_team.bounty;
                                    broadcast = Caught {
                                        catcher: catcher_team.to_sendable(player_entries, catcher),
                                        caught: caught_team.to_sendable(player_entries, caught),
                                    };
                                }
                                TeamRole::Catcher => return Error(TeamIsCatcher(caught)).into(),
                            },
                            None => {
                                return Error(NotFound(format!("caught team with id {}", caught)))
                                    .into()
                            }
                        },
                        TeamRole::Runner => return Error(TeamIsRunner(catcher)).into(),
                    },
                    None => {
                        return Error(NotFound(format!("catcher team with id {}", catcher))).into()
                    }
                };
                match self.teams.get_mut(catcher) {
                    Some(catcher_team) => {
                        catcher_team.have_caught(
                            bounty,
                            caught,
                            &config,
                            challenge_entries,
                            zone_entries,
                        );
                    }
                    None => {
                        return Error(NotFound(format!("catcher team with id {}", catcher))).into()
                    }
                }
                match self.teams.get_mut(caught) {
                    Some(caught_team) => {
                        caught_team.be_caught(catcher);
                    }
                    None => {
                        return Error(NotFound(format!("caught team with id {}", caught))).into()
                    }
                }
                EngineResponse {
                    response_action: Success,
                    broadcast_action: Some(broadcast),
                }
                .into()
            }
            None => Error(GameNotRunning).into(),
        }
    }

    fn complete(
        &mut self,
        completer: usize,
        completed: usize,
        challenge_entries: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        player_entries: &DBMirror<PlayerEntry>,
    ) -> InternEngineResponsePackage {
        let config = self.config();
        match self.teams.get_mut(completer) {
            Some(completer_team) => match completer_team.complete_challenge(
                completed,
                &config,
                challenge_entries,
                zone_entries,
            ) {
                Ok(completed) => EngineResponse {
                    response_action: Success,
                    broadcast_action: Some(BroadcastAction::Completed {
                        completer: completer_team.to_sendable(player_entries, completer),
                        completed: completed.to_sendable(),
                    }),
                }
                .into(),
                Err(err) => Error(err).into(),
            },
            None => Error(NotFound(format!("completer team with id {}", completer))).into(),
        }
    }

    fn get_state(&self, player_entries: &DBMirror<PlayerEntry>) -> InternEngineResponsePackage {
        SendState {
            teams: self
                .teams
                .iter()
                .enumerate()
                .map(|(i, t)| t.to_sendable(player_entries, i))
                .collect(),
            game: self.game.clone().map(|g| g.to_sendable()),
        }
        .into()
    }

    fn add_team(
        &mut self,
        name: String,
        discord_channel: Option<u64>,
        colour: Option<Colour>,
    ) -> InternEngineResponsePackage {
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
            self.teams.push(TeamEntry::new(
                name,
                Vec::new(),
                discord_channel,
                colour,
                self.config(),
            ));
            Success.into()
        }
    }

    fn start(
        &mut self,
        challenge_entries: &[DBEntry<ChallengeEntry>],
        zone_entries: &DBMirror<ZoneEntry>,
        session_id: u64,
        player_entries: &DBMirror<PlayerEntry>,
    ) -> InternEngineResponsePackage {
        match self.game {
            Some(_) => Error(GameInProgress).into(),
            None => {
                // Setup teams
                let num_catchers = self.config().num_catchers as usize;
                if num_catchers >= self.teams.len() {
                    return Error(BadData(format!(
                        "cannot start game with {} catchers \
                        when there are only {} teams",
                        num_catchers,
                        self.teams.len()
                    )))
                    .into();
                }
                let start_zone = self.config().start_zone;
                for team in &mut self.teams {
                    team.role = TeamRole::Runner;
                    team.bounty = 0;
                    team.points = 0;
                    team.periods = Vec::new();
                    team.zone_id = start_zone;
                }
                let team_ids = 0..self.teams.len();
                let catcher_ids = team_ids
                    .clone()
                    .choose_multiple(&mut thread_rng(), num_catchers);
                let runner_ids: Vec<usize> =
                    team_ids.filter(|i| !catcher_ids.contains(i)).collect();

                let config = self.config();
                for id in catcher_ids {
                    self.teams[id].start_catcher(&config);
                }
                for id in runner_ids {
                    self.teams[id].start_runner(&config, challenge_entries, zone_entries);
                }

                // Setup game
                let today = chrono::Local::now().date_naive();
                let game = InGame {
                    name: format!("{} am {}", self.name, today),
                    date: today,
                    mode: self.mode,
                };
                self.game = Some(game.clone());

                // Setup alarm
                let alarm = RuntimeRequest::CreateAlarm {
                    time: config.end_time,
                    payload: InternEngineCommand::Command(EngineCommand {
                        session: Some(session_id),
                        action: Stop,
                    }),
                };

                InternEngineResponsePackage {
                    response: EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(Started {
                            teams: self
                                .teams
                                .iter()
                                .enumerate()
                                .map(|(i, t)| t.to_sendable(player_entries, i))
                                .collect(),
                            game: game.to_sendable(),
                        }),
                    }
                    .into(),
                    runtime_requests: Some(vec![alarm]),
                }
            }
        }
    }

    fn stop(&mut self) -> InternEngineResponsePackage {
        if self.game.is_none() {
            return Error(GameNotRunning).into();
        }
        // Error(NotImplemented).into() // TODO
        // The following implementation is temporary and should be changed once the cached db is
        // reworked with dedicated structs for minimal autosaves.
        self.game = None;
        EngineResponse {
            response_action: Success,
            broadcast_action: Some(Ended),
        }
        .into()
    }

    fn get_events(&self) -> InternEngineResponsePackage {
        let mut events = Vec::new();
        for (team_id, team) in self.teams.iter().enumerate() {
            events.extend(
                team.periods
                    .iter()
                    .filter_map(|period| match &period.context {
                        PeriodContext::CompletedChallenge {
                            title,
                            description,
                            zone: _,
                            points,
                            photo: _,
                            id: _,
                        } => Some(Event::Complete {
                            challenge: Challenge {
                                title: title.clone(),
                                description: description.clone(),
                                points: *points,
                            },
                            completer_id: team_id,
                            time: period.end_time,
                        }),
                        PeriodContext::Catcher {
                            caught_team,
                            bounty,
                        } => Some(Event::Catch {
                            catcher_id: team_id,
                            caught_id: *caught_team,
                            bounty: *bounty,
                            time: period.end_time,
                        }),
                        PeriodContext::Trophy {
                            trophies: _,
                            points_spent: _,
                        }
                        | PeriodContext::Caught {
                            catcher_team: _,
                            bounty: _,
                        } => None,
                    }),
            );
        }
        SendEvents(events).into()
    }

    pub fn vroom(
        &mut self,
        command: EngineAction,
        session_id: u64,
        player_entries: &DBMirror<PlayerEntry>,
        challenge_entries: &DBMirror<ChallengeEntry>,
        zone_entries: &DBMirror<ZoneEntry>,
    ) -> InternEngineResponsePackage {
        match command {
            GetEvents => self.get_events(),
            GenerateTeamChallenges(id) => {
                self.generate_team_challenges(id, &challenge_entries.get_all(), zone_entries)
            }
            AddChallengeToTeam { team, challenge } => self.add_challenge_to_team(team, challenge),
            RenameTeam { team, new_name } => self.rename_team(team, new_name),
            MakeTeamCatcher(id) => self.make_team_catcher(id, player_entries),
            MakeTeamRunner(id) => self.make_team_runner(id, player_entries),
            SendLocation { player, location } => self.send_location(player, location),
            AssignPlayerToTeam { player, team } => {
                self.assign_player_to_team(player, team, session_id)
            }
            Catch { catcher, caught } => self.catch(
                catcher,
                caught,
                &challenge_entries.get_all(),
                zone_entries,
                player_entries,
            ),
            Complete {
                completer,
                completed,
            } => self.complete(
                completer,
                completed,
                &challenge_entries.get_all(),
                zone_entries,
                player_entries,
            ),
            GetState => self.get_state(player_entries),
            AddTeam {
                name,
                discord_channel,
                colour,
            } => self.add_team(name, discord_channel, colour),
            Start => self.start(
                &challenge_entries.get_all(),
                zone_entries,
                session_id,
                player_entries,
            ),
            Stop => self.stop(),
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
            AddChallengeSet(_) => Error(SessionSupplied).into(),
            GetChallengeSets => Error(SessionSupplied).into(),
            DeleteAllChallenges => Error(SessionSupplied).into(),
            GetAllZones => Error(SessionSupplied).into(),
            AddZone {
                zone: _,
                num_conn_zones: _,
                num_connections: _,
                train_through: _,
                mongus: _,
                s_bahn_zone: _,
            } => Error(SessionSupplied).into(),
            AddMinutesTo {
                from_zone: _,
                to_zone: _,
                minutes: _,
            } => Error(SessionSupplied).into(),
        }
    }
}
