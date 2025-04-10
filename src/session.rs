use crate::{
    challenge::InOpenChallenge,
    runtime::{InternEngineCommand, InternEngineResponse, InternEngineResponsePackage},
    team::{PeriodContext, TeamEntry},
    Config, EngineContext, InGame, PartialConfig, PastGame, SessionContext,
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

    fn context<'a>(&self, context: EngineContext<'a>, session_id: u64) -> SessionContext<'a> {
        SessionContext {
            engine_context: context,
            config: self.config(),
            top_team_points: self.teams.iter().map(|t| t.points).max(),
            session_id,
        }
    }

    pub fn team_left_grace_period(
        &mut self,
        team_id: usize,
        session_id: u64,
        context: EngineContext,
    ) -> InternEngineResponsePackage {
        let context = self.context(context, session_id);
        if let Some(team) = self.teams.get_mut(team_id) {
            team.grace_period_end = None;
            EngineResponse {
                response_action: Success,
                broadcast_action: Some(TeamLeftGracePeriod(team.to_sendable(team_id, &context))),
            }
            .into()
        } else {
            Success.into() // do nothing
        }
    }

    fn generate_team_challenges(
        &mut self,
        id: usize,
        context: &SessionContext,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound(format!("team with id {}", id))).into(),
            Some(team) => {
                team.generate_challenges(context);
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
        context: &SessionContext,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound(format!("team with id {}", id))).into(),
            Some(team) => match team.role {
                TeamRole::Catcher => Success.into(),
                TeamRole::Runner => {
                    team.role = TeamRole::Catcher;
                    EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(TeamMadeCatcher(team.to_sendable(id, context))),
                    }
                    .into()
                }
            },
        }
    }

    fn make_team_runner(
        &mut self,
        id: usize,
        context: &SessionContext,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound(format!("team with id {}", id))).into(),
            Some(team) => match team.role {
                TeamRole::Runner => Success.into(),
                TeamRole::Catcher => {
                    team.role = TeamRole::Runner;
                    EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(TeamMadeRunner(team.to_sendable(id, context))),
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
            .position(|t| t.players.iter().any(|&p| p == player))
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
        context: &SessionContext,
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
                            session: context.session_id,
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
                    session: context.session_id,
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
        context: &mut SessionContext,
    ) -> InternEngineResponsePackage {
        match self.game {
            Some(_) => {
                if caught == catcher {
                    return Error(BadData("a team cannot catch itself".into())).into();
                }
                let bounty;
                let broadcast;
                let mut runtime_requests = Vec::new();
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
                                        catcher: catcher_team.to_sendable(catcher, context),
                                        caught: caught_team.to_sendable(caught, context),
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
                        runtime_requests
                            .push(catcher_team.have_caught(bounty, caught, catcher, context));
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
                InternEngineResponsePackage {
                    response: EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(broadcast),
                    }
                    .into(),
                    runtime_requests: Some(runtime_requests),
                }
            }
            None => Error(GameNotRunning).into(),
        }
    }

    fn complete(
        &mut self,
        completer: usize,
        completed: usize,
        context: &SessionContext,
    ) -> InternEngineResponsePackage {
        match self.game {
            Some(_) => match self.teams.get_mut(completer) {
                Some(completer_team) => match completer_team.complete_challenge(completed, context)
                {
                    Ok(completed) => EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(BroadcastAction::Completed {
                            completer: completer_team.to_sendable(completer, context),
                            completed: completed.to_sendable(),
                        }),
                    }
                    .into(),
                    Err(err) => Error(err).into(),
                },
                None => Error(NotFound(format!("completer team with id {}", completer))).into(),
            },
            None => Error(GameNotRunning).into(),
        }
    }

    fn get_state(&self, context: &SessionContext) -> InternEngineResponsePackage {
        SendState {
            teams: self
                .teams
                .iter()
                .enumerate()
                .map(|(i, t)| t.to_sendable(i, context))
                .collect(),
            events: self.gather_events(),
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

    fn start(&mut self, context: &mut SessionContext) -> InternEngineResponsePackage {
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
                let team_ids = 0..self.teams.len();
                let catcher_ids = team_ids
                    .clone()
                    .choose_multiple(&mut thread_rng(), num_catchers);
                let runner_ids: Vec<usize> =
                    team_ids.filter(|i| !catcher_ids.contains(i)).collect();

                let config = self.config();
                for id in catcher_ids {
                    if let Err(err) = self.teams[id].start_catcher(context) {
                        return Error(err).into();
                    }
                }
                for id in runner_ids {
                    if let Err(err) = self.teams[id].start_runner(context) {
                        return Error(err).into();
                    }
                }

                // Setup alarm
                let (request, timer) = context.engine_context.timer_tracker.alarm(
                    chrono::Local::now().with_time(config.end_time).unwrap(),
                    InternEngineCommand::Command(EngineCommand {
                        session: Some(context.session_id),
                        action: Stop,
                    }),
                );

                // Setup game
                let now = chrono::Local::now();
                let today = now.date_naive();
                let game = InGame {
                    name: format!("{} am {}", self.name, today),
                    start_time: now,
                    mode: self.mode,
                    timer,
                };
                self.game = Some(game.clone());

                InternEngineResponsePackage {
                    response: EngineResponse {
                        response_action: Success,
                        broadcast_action: Some(Started {
                            teams: self
                                .teams
                                .iter()
                                .enumerate()
                                .map(|(i, t)| t.to_sendable(i, context))
                                .collect(),
                            game: game.to_sendable(),
                        }),
                    }
                    .into(),
                    runtime_requests: Some(vec![request]),
                }
            }
        }
    }

    fn stop(&mut self, context: &mut SessionContext) -> InternEngineResponsePackage {
        if self.game.is_none() {
            return Error(GameNotRunning).into();
        }

        let mut requests = Vec::new();
        if let Some(game) = &self.game {
            requests.push(game.timer.cancel_request());
        }
        let past_game = PastGame::new_now(self.game.take().unwrap(), self.teams.clone());
        context.engine_context.past_game_db.add(past_game);

        for team in &mut self.teams {
            let _ = team.reset(context);
            // we can ignore the potential error here, since the start zone is not relevant when
            // stopping the game.
            if let Some(timer) = &team.grace_period_end {
                requests.push(timer.cancel_request());
            }
        }

        InternEngineResponsePackage {
            response: InternEngineResponse::DirectResponse(EngineResponse {
                response_action: Success,
                broadcast_action: Some(Ended),
            }),
            runtime_requests: Some(requests),
        }
    }

    fn gather_events(&self) -> Vec<Event> {
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
        events.sort();
        events
    }

    fn get_events(&self) -> InternEngineResponsePackage {
        SendEvents(self.gather_events()).into()
    }

    pub fn vroom(
        &mut self,
        command: EngineAction,
        session_id: u64,
        context: EngineContext,
    ) -> InternEngineResponsePackage {
        let mut context = self.context(context, session_id);
        match command {
            GetEvents => self.get_events(),
            GenerateTeamChallenges(id) => self.generate_team_challenges(id, &context),
            AddChallengeToTeam { team, challenge } => self.add_challenge_to_team(team, challenge),
            RenameTeam { team, new_name } => self.rename_team(team, new_name),
            MakeTeamCatcher(id) => self.make_team_catcher(id, &context),
            MakeTeamRunner(id) => self.make_team_runner(id, &context),
            SendLocation { player, location } => self.send_location(player, location),
            AssignPlayerToTeam { player, team } => {
                self.assign_player_to_team(player, team, &context)
            }
            Catch { catcher, caught } => self.catch(catcher, caught, &mut context),
            Complete {
                completer,
                completed,
            } => self.complete(completer, completed, &context),
            GetState => self.get_state(&context),
            AddTeam {
                name,
                discord_channel,
                colour,
            } => self.add_team(name, discord_channel, colour),
            Start => self.start(&mut context),
            Stop => self.stop(&mut context),
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
            UploadChallengePictures(_) => Error(SessionSupplied).into(),
        }
    }
}
