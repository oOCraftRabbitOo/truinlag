use std::num::NonZeroU32;

use crate::{
    challenge::InOpenChallenge,
    runtime::{
        InternEngineCommand, InternEngineResponse, InternEngineResponsePackage, RuntimeRequest,
    },
    team::{PeriodContext, TeamEntry},
    Config, EngineContext, InGame, PartialConfig, PastGame, PictureEntry, SessionContext,
};
use bonsaidb::core::schema::Collection;
use chrono::Timelike;
use geo::Distance;
use partially::Partial;
use rand::{rng, seq::IteratorRandom};
use serde::{Deserialize, Serialize};
use strsim::normalized_damerau_levenshtein as strcmp;
use truinlag::{
    commands::{BroadcastAction::*, EngineAction::*, Error::*, ResponseAction::*, *},
    *,
};

/// Multiple trainlag games should be able to run in parallel. Each instance that could be running
/// a game is represented by a session. So, e.g., Family TrainLag, OG TrainLag, etc.
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
    /// Retreives the session config.
    ///
    /// The session only saves a partial config, i.e. only overwrites for certain values. The
    /// `config()` method takes the session's partial config and extends it with the default
    /// values.
    pub fn config(&self) -> Config {
        let mut cfg = Config::default();
        cfg.apply_some(self.config.clone());
        cfg
    }

    /// Creates a new session
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

    /// Gets called by the engine on startup and returns runtime requests for all timers that need
    /// to be restarted.
    pub fn setup(&self) -> Vec<RuntimeRequest> {
        match &self.game {
            None => Vec::new(),
            Some(game) => {
                let mut requests = Vec::new();
                requests.push(game.timer.create_request());
                for team in &self.teams {
                    if let Some(hook) = &team.grace_period_end {
                        requests.push(hook.create_request());
                    }
                }
                requests
            }
        }
    }

    /// Converts the engine-internal session to a truinlag session
    pub fn to_sendable(&self, id: u64) -> GameSession {
        GameSession {
            name: self.name.clone(),
            mode: self.mode,
            id,
        }
    }

    /// Generates the session context from the engine context and the session id
    fn context<'a>(&self, context: EngineContext<'a>, session_id: u64) -> SessionContext<'a> {
        SessionContext {
            engine_context: context,
            config: self.config(),
            top_team_points: self.teams.iter().map(|t| t.points).max(),
            session_id,
        }
    }

    /// Gets called from a timer and makes a team leave its grace period
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

    /// Corresponds to an `EngineAction` and manually generates new challenges for a team
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

    /// Corresponds to an `EngineAction` and manually adds a challenge to a team
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

    /// Corresponds to an `EngineAction` and renames a team
    fn rename_team(&mut self, team: usize, new_name: String) -> InternEngineResponsePackage {
        match self.teams.get_mut(team) {
            None => Error(NotFound(format!("team with id {}", team))).into(),
            Some(team) => {
                team.name = new_name;
                Success.into()
            }
        }
    }

    /// Corresponds to an `EngineAction` and changes a given team's role to hunter
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

    /// Corresponds to an `EngineAction` and changes a given team's role to gatherer
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

    /// Corresponds to an `EngineAction` and processes a given players' location update
    fn send_location(
        &mut self,
        player: u64,
        location: DetailedLocation,
    ) -> InternEngineResponsePackage {
        if self.game.is_none() {
            // locations should't be tracked if no game is running. A success response is returned
            // anyways, since the trainlapp always shows all error responses as full-screen popups.
            // That would be extremely obnoxious pre– and post-game. We should probably rework the
            // app to stop sending locations at those times. Yeah, that'll be a TODO
            return Success.into();
        }
        match self
            .teams
            .iter_mut()
            .enumerate()
            .find(|(_, t)| t.players.contains(&player))
        {
            None => Error(NotFound(format!("team with the player with id {}", player))).into(),
            Some((team_id, team)) => match team.add_location(location, player) {
                Some(location) => EngineResponse {
                    response_action: Success,
                    broadcast_action: Some(Location {
                        team: team_id,
                        location,
                    }),
                }
                .into(),
                None => Success.into(),
            },
        }
    }

    /// Corresponds to an `EngineAction` and assigns a player to a team
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

    /// Corresponds to an `EngineAction` and processes one team catching another
    fn catch(
        &mut self,
        catcher: usize,
        caught: usize,
        period_id: usize,
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
                    Some(catcher_team) => {
                        if period_id != catcher_team.period_id() {
                            return Error(BadData(format!(
                                "period_id {} is invalid, should be {}",
                                period_id,
                                catcher_team.period_id()
                            )))
                            .into();
                        }
                        match catcher_team.role {
                            TeamRole::Catcher => match self.teams.get(caught) {
                                Some(caught_team) => match caught_team.role {
                                    TeamRole::Runner => {
                                        if caught_team.location().is_some()
                                            && catcher_team.location().is_some()
                                        {
                                            let caught_location =
                                                caught_team.location_as_point().unwrap();
                                            let catcher_location =
                                                catcher_team.location_as_point().unwrap();
                                            if geo::Geodesic
                                                .distance(caught_location, catcher_location)
                                                > 300.0
                                            {
                                                return Error(TeamsTooFar).into();
                                            }
                                        } else {
                                            eprintln!(
                                                "engine: session: no location for team {} or {}",
                                                catcher_team.name, caught_team.name
                                            )
                                        }
                                        bounty = caught_team.bounty;
                                        broadcast = Caught {
                                            catcher: catcher_team.to_sendable(catcher, context),
                                            caught: caught_team.to_sendable(caught, context),
                                        };
                                    }
                                    TeamRole::Catcher => {
                                        return Error(TeamIsCatcher(caught)).into()
                                    }
                                },
                                None => {
                                    return Error(NotFound(format!(
                                        "caught team with id {}",
                                        caught
                                    )))
                                    .into()
                                }
                            },
                            TeamRole::Runner => return Error(TeamIsRunner(catcher)).into(),
                        }
                    }
                    None => {
                        return Error(NotFound(format!("catcher team with id {}", catcher))).into()
                    }
                };
                let period_id;
                match self.teams.get_mut(catcher) {
                    Some(catcher_team) => {
                        period_id = catcher_team.periods.len();
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
                        response_action: Period(period_id),
                        broadcast_action: Some(broadcast),
                    }
                    .into(),
                    runtime_requests: Some(runtime_requests),
                }
            }
            None => Error(GameNotRunning).into(),
        }
    }

    /// Corresponds to an `EngineAction` and processes a team completing a challenge
    fn complete(
        &mut self,
        completer: usize,
        completed: usize,
        period_id: usize,
        context: &SessionContext,
    ) -> InternEngineResponsePackage {
        match self.game {
            Some(_) => match self.teams.get_mut(completer) {
                Some(completer_team) => {
                    if period_id != completer_team.period_id() {
                        return Error(BadData(format!(
                            "period_id {} is invalid, should be {}",
                            period_id,
                            completer_team.period_id()
                        )))
                        .into();
                    }
                    if completer_team
                        .periods
                        .last()
                        .map(|p| {
                            chrono::Local::now()
                                .num_seconds_from_midnight()
                                .abs_diff(p.end_time.num_seconds_from_midnight())
                                <= 30
                        })
                        .unwrap_or(false)
                    {
                        return Error(TooRapid).into();
                    }
                    match completer_team.complete_challenge(completed, context) {
                        Ok((completed, request)) => InternEngineResponsePackage {
                            response: InternEngineResponse::DirectResponse(EngineResponse {
                                response_action: Period(completer_team.periods.len() - 1),
                                broadcast_action: Some(BroadcastAction::Completed {
                                    completer: completer_team.to_sendable(completer, context),
                                    completed: completed.to_sendable(),
                                }),
                            }),
                            runtime_requests: request.map(|r| vec![r]),
                        },
                        Err(err) => Error(err).into(),
                    }
                }
                None => Error(NotFound(format!("completer team with id {}", completer))).into(),
            },
            None => Error(GameNotRunning).into(),
        }
    }

    /// Corresponds to an `EngineAction` and returns the current session state
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

    /// Corresponds to an `EngineAction` and adds a team
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

    /// Corresponds to an `EngineAction` and starts the game
    fn start(&mut self, context: &mut SessionContext) -> InternEngineResponsePackage {
        println!("starting game...");
        match self.game {
            Some(_) => Error(GameInProgress).into(),
            None => {
                let config = self.config();

                // checking for whether some config values are conflicting
                if (context.engine_context.challenge_db.count() as u64) < config.num_challenges {
                    return Error(TooFewChallenges).into();
                }

                if context
                    .engine_context
                    .zone_db
                    .find(|z| z.zone == config.centre_zone)
                    .is_none()
                {
                    return Error(NotFound(format!("centre zone {}", config.centre_zone))).into();
                }
                if context
                    .engine_context
                    .zone_db
                    .find(|z| z.zone == config.start_zone)
                    .is_none()
                {
                    return Error(NotFound(format!("start zone {}", config.start_zone))).into();
                }

                let set = config
                    .challenge_sets
                    .iter()
                    .find(|&&s| context.engine_context.challenge_set_db.get(s).is_none());
                if let Some(bad_set) = set {
                    return Error(NotFound(format!("challenge set with id {}", bad_set))).into();
                }

                let num_catchers = config.num_catchers as usize;
                if num_catchers >= self.teams.len() {
                    return Error(BadData(format!(
                        "cannot start game with {} catchers \
                        when there are only {} teams",
                        num_catchers,
                        self.teams.len()
                    )))
                    .into();
                }

                // Set up teams
                let team_ids = 0..self.teams.len();
                let catcher_ids = team_ids.clone().choose_multiple(&mut rng(), num_catchers);
                let runner_ids: Vec<usize> =
                    team_ids.filter(|i| !catcher_ids.contains(i)).collect();
                for id in catcher_ids {
                    if let Err(err) = self.teams[id].start_catcher(context) {
                        return Error(err).into();
                    }
                }
                // artificially disable underdog bonus for initial challenge generation. Otherwise,
                // some teams may have loads of points from the last game that would spill over.
                context.top_team_points = None;
                for id in runner_ids {
                    if let Err(err) = self.teams[id].start_runner(context) {
                        return Error(err).into();
                    }
                }

                // Set up alarm
                let (request, timer) = context.engine_context.timer_tracker.alarm(
                    chrono::Local::now().with_time(config.end_time).unwrap(),
                    InternEngineCommand::Command(Box::new(EngineCommand {
                        session: Some(context.session_id),
                        action: Stop,
                    })),
                );

                // Set up game
                // This has to be done last, as many commands' runnability depends on whether a
                // game is running. Some of the previous steps can return errors, so they are done
                // earlier.
                let now = chrono::Local::now();
                let today = now.date_naive();
                let game = InGame {
                    name: format!("{} am {}", self.name, today),
                    start_time: now,
                    mode: self.mode,
                    timer,
                };
                self.game = Some(game.clone());

                // Return response
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

    /// Corresponds to an `EngineAction` and stops the game. This is called automatically by a
    /// timer that is set up when starting the game.
    fn stop(&mut self, context: &mut SessionContext) -> InternEngineResponsePackage {
        // check whether game is actually running
        if self.game.is_none() {
            return Error(GameNotRunning).into();
        }

        // cancel game end timer
        let mut requests = Vec::new();
        // this check is unnecessary, since it was done above already, but it avoids a clone
        if let Some(game) = &self.game {
            requests.push(game.timer.cancel_request());
        }

        // extract and save past game
        let past_game = PastGame::new_now(self.game.take().unwrap(), self.teams.clone());
        context.engine_context.past_game_db.add(past_game);

        // reset teams
        for team in &mut self.teams {
            // let _ = team.reset(context);
            // we can ignore the potential error here, since the start zone is not relevant when
            // stopping the game.
            if let Some(timer) = &team.grace_period_end {
                requests.push(timer.cancel_request());
            }
        }

        // return response
        InternEngineResponsePackage {
            response: InternEngineResponse::DirectResponse(EngineResponse {
                response_action: Success,
                broadcast_action: Some(Ended),
            }),
            runtime_requests: Some(requests),
        }
    }

    /// Compiles all events from all teams into a list of events ordered by time
    fn gather_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        for (team_id, team) in self.teams.iter().enumerate() {
            events.extend(team.periods.iter().filter_map(|period| {
                match &period.context {
                    PeriodContext::CompletedChallenge {
                        title,
                        description,
                        zone: _,
                        points,
                        id: _,
                    } => Some(Event::Complete {
                        challenge: Challenge {
                            title: title.clone(),
                            description: description.clone(),
                            points: *points,
                        },
                        completer_id: team_id,
                        picture_ids: period.pictures.clone(),
                        time: period.end_time,
                        location: team
                            .locations
                            .get(period.location_end_index)
                            .unwrap()
                            .clone(),
                    }),
                    PeriodContext::Catcher {
                        caught_team,
                        bounty,
                    } => Some(Event::Catch {
                        catcher_id: team_id,
                        caught_id: *caught_team,
                        bounty: *bounty,
                        picture_ids: period.pictures.clone(),
                        time: period.end_time,
                        location: team
                            .locations
                            .get(period.location_end_index)
                            .unwrap()
                            .clone(),
                    }),
                    PeriodContext::Trophy {
                        trophies: _,
                        points_spent: _,
                    }
                    | PeriodContext::Caught {
                        catcher_team: _,
                        bounty: _,
                    } => None,
                }
            }));
        }
        events.sort();
        events
    }

    /// Corresponds to an `EngineAction` and returns all events in a list ordered by time
    fn get_events(&self) -> InternEngineResponsePackage {
        SendEvents(self.gather_events()).into()
    }

    /// Corresponds to an `EngineAction` and updates a team's picture
    fn upload_team_picture(
        &mut self,
        team_id: usize,
        picture: RawPicture,
        context: SessionContext,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(team_id) {
            Some(_team) => {
                let session_id = context.session_id;
                InternEngineResponse::DelayedLoopback(tokio::spawn(async move {
                    let pfp =
                        tokio::task::block_in_place(|| PictureEntry::new_profile(picture.into()))
                            .map_err(|err| {
                                eprintln!("Engine: couldn't convert picture: {}", err);
                                PictureProblem
                            });
                    InternEngineCommand::MadeTeamProfile {
                        session_id,
                        team_id,
                        pfp,
                    }
                }))
                .into()
            }
            None => Error(NotFound(format!("team with id {team_id}"))).into(),
        }
    }

    fn upload_period_pictures(
        &mut self,
        team_id: usize,
        period_id: usize,
        pictures: Vec<RawPicture>,
        context: SessionContext,
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(team_id) {
            None => Error(NotFound(format!("team with id {team_id}"))).into(),
            Some(team) => match team.periods.get_mut(period_id) {
                None => Error(NotFound(format!(
                    "period with index {} in team {}",
                    period_id, team.name
                )))
                .into(),
                Some(period) => {
                    let ids = pictures.into_iter().map(|picture| {
                        context
                            .engine_context
                            .picture_db
                            .add(PictureEntry::new_challenge_picture(picture))
                    });
                    period.pictures.extend(ids);
                    Success.into()
                }
            },
        }
    }

    fn send_locations(&self, context: SessionContext) -> InternEngineResponsePackage {
        SendLocations(
            self.teams
                .iter()
                .enumerate()
                .map(|(index, team)| (team.to_sendable(index, &context), team.locations.clone()))
                .collect(),
        )
        .into()
    }

    fn send_past_locations(
        &self,
        team_id: usize,
        of_past_seconds: Option<NonZeroU32>,
    ) -> InternEngineResponsePackage {
        let team = match self.teams.get(team_id) {
            Some(team) => team,
            None => return Error(NotFound(format!("team with id {team_id}"))).into(),
        };
        let target_time =
            of_past_seconds.map(|secs| chrono::Local::now().timestamp() - secs.get() as i64);
        SendPastLocations {
            team_id,
            locations: match target_time {
                Some(secs) => team
                    .locations
                    .split_at(team.locations.partition_point(|loc| loc.timestamp < secs))
                    .1
                    .to_vec(),
                None => team.locations.clone(),
            },
        }
        .into()
    }

    fn send_game_config(&self) -> InternEngineResponsePackage {
        SendGameConfig(self.config().into()).into()
    }

    fn set_game_config(&mut self, new_config: PartialGameConfig) -> InternEngineResponsePackage {
        self.config.apply_some(new_config.into());
        Success.into()
    }

    /// The core method of the session that processes commands with sessions.
    ///
    /// The method takes a command to process, as well as a session id. This should be the id of
    /// the session that this method is called on and it is required because that id is not
    /// actually known to the session. Additionally, context of type `EngineContext` is required in
    /// order to access challenges, zones, players, etc.
    pub fn vroom(
        &mut self,
        command: EngineAction,
        session_id: u64,
        context: EngineContext,
    ) -> InternEngineResponsePackage {
        let mut context = self.context(context, session_id);
        match command {
            SetGameConfig(new_config) => self.set_game_config(new_config),
            GetGameConfig => self.send_game_config(),
            GetPastLocations {
                team_id,
                of_past_seconds,
            } => self.send_past_locations(team_id, of_past_seconds),
            GetLocations => self.send_locations(context),
            UploadPeriodPictures {
                pictures,
                team,
                period,
            } => self.upload_period_pictures(team, period, pictures, context),
            UploadTeamPicture { team_id, picture } => {
                self.upload_team_picture(team_id, picture, context)
            }
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
            Catch {
                catcher,
                caught,
                period_id,
            } => self.catch(catcher, caught, period_id, &mut context),
            Complete {
                completer,
                completed,
                period_id,
            } => self.complete(completer, completed, period_id, &context),
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
            UploadPlayerPicture {
                player_id: _,
                picture: _,
            } => Error(SessionSupplied).into(),
            GetThumbnails(_) => Error(SessionSupplied).into(),
            GetPictures(_) => Error(SessionSupplied).into(),
        }
    }
}
