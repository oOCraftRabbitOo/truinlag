use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EngineCommand {
    pub session: Option<u64>,
    pub action: EngineAction,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EngineCommandPackage {
    pub command: EngineCommand,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EngineResponse {
    pub response_action: ResponseAction,
    pub broadcast_action: Option<BroadcastAction>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ClientCommand {
    Broadcast(BroadcastAction),
    Response(ResponsePackage),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResponsePackage {
    pub action: ResponseAction,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum EngineAction {
    AddPlayer {
        name: String,
        discord_id: Option<u64>,
        passphrase: String,
    },
    AddTeam {
        name: String,
        players: Vec<u64>,
        discord_channel: Option<u64>,
        colour: Option<Colour>,
    },
    Catch {
        catcher: String,
        caught: String,
    },
    Complete {
        completer: String,
        completed: u32,
    },
    GetPlayerByPassphrase(String),
    Start,
    Stop,
    Ping(Option<String>),
    GetState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ResponseAction {
    Error(Error),
    Team(u64),
    Player(u64),
    SendState {
        teams: Vec<Team>,
        game: Option<Game>,
    },
    Success,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum BroadcastAction {
    Catch {
        catcher: Team,
        caught: Team,
    },
    Complete {
        completer: Team,
        completed: Challenge,
    },
    Start,
    End,
    Ping(Option<String>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Error {
    NoSessionSupplied,
    SessionSupplied,
    NotFound,
    TeamExists(String),
    AlreadyExists,
    GameInProgress,
    InternalError,
}
