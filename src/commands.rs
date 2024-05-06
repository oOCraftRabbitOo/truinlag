use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EngineCommand {
    pub session: Option<u64>,
    pub action: EngineAction,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Mode {
    Traditional,
    Gfrorefurz,
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
    Start,
    Stop,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ResponseAction {
    Catch {
        catcher: String,
        caught: String,
    },
    Complete {
        completer: String,
        completed: String,
    },
    End,
    MakeCatcher(String),
    MakeRunner(String),
    Error(Error),
    AddedPlayer(u64),
    Success,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum BroadcastAction {
    Catch {
        catcher: String,
        caught: String,
    },
    Complete {
        completer: String,
        completed: String,
    },
    Start,
    End,
    Crash,
    Success,
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
