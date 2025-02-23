use super::{
    challenge::{ChallengeEntry, InOpenChallenge},
    runtime::InternEngineResponsePackage,
    team::TeamEntry,
    Config, DBEntry, InGame, PartialConfig, PlayerEntry, ZoneEntry,
};
use bonsaidb::core::schema::Collection;
use partially::Partial;
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
        zone_entries: &[DBEntry<ZoneEntry>],
    ) -> InternEngineResponsePackage {
        let config = self.config();
        match self.teams.get_mut(id) {
            None => Error(NotFound).into(),
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
            None => Error(NotFound).into(),
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
            None => Error(NotFound).into(),
            Some(team) => {
                team.name = new_name;
                Success.into()
            }
        }
    }

    fn make_team_catcher(
        &mut self,
        id: usize,
        player_entries: &[DBEntry<PlayerEntry>],
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound).into(),
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
        player_entries: &[DBEntry<PlayerEntry>],
    ) -> InternEngineResponsePackage {
        match self.teams.get_mut(id) {
            None => Error(NotFound).into(),
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
            None => Error(NotFound).into(),
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
                None => Error(NotFound).into(),
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

    fn catch(&mut self, catcher: usize, caught: usize) -> InternEngineResponsePackage {
        Error(NotImplemented).into()
    }

    fn complete(&mut self, completer: usize, completed: usize) -> InternEngineResponsePackage {
        match self.teams.get_mut(completer) {
            Some(completer) => match completer.challenges.get_mut(completed) {
                Some(_completed) => todo!(),
                None => todo!(),
            },
            None => todo!(),
        }
    }

    fn get_state(&self, player_entries: &[DBEntry<PlayerEntry>]) -> InternEngineResponsePackage {
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

    fn start(&mut self) -> InternEngineResponsePackage {
        match self.game {
            Some(_) => Error(GameInProgress).into(),
            None => {
                Error(NotImplemented).into() // TODO:
            }
        }
    }

    fn stop(&mut self) -> InternEngineResponsePackage {
        Error(NotImplemented).into() // TODO:
    }

    pub fn vroom(
        &mut self,
        command: EngineAction,
        session_id: u64,
        player_entries: &[DBEntry<PlayerEntry>],
        challenge_entries: &[DBEntry<ChallengeEntry>],
        zone_entries: &[DBEntry<ZoneEntry>],
    ) -> InternEngineResponsePackage {
        match command {
            GenerateTeamChallenges(id) => {
                self.generate_team_challenges(id, challenge_entries, zone_entries)
            }
            AddChallengeToTeam { team, challenge } => self.add_challenge_to_team(team, challenge),
            RenameTeam { team, new_name } => self.rename_team(team, new_name),
            MakeTeamCatcher(id) => self.make_team_catcher(id, player_entries),
            MakeTeamRunner(id) => self.make_team_runner(id, player_entries),
            SendLocation { player, location } => self.send_location(player, location),
            AssignPlayerToTeam { player, team } => {
                self.assign_player_to_team(player, team, session_id)
            }
            Catch { catcher, caught } => self.catch(catcher, caught),
            Complete {
                completer,
                completed,
            } => self.complete(completer, completed),
            GetState => self.get_state(player_entries),
            AddTeam {
                name,
                discord_channel,
                colour,
            } => self.add_team(name, discord_channel, colour),
            Start => self.start(),
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
