use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EngineCommand {
    pub session: Option<String>,
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
    Catch { catcher: String, caught: String },
    Complete { completer: String, completed: u32 },
    Start,
    Stop,
    Status,
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
    Start(Config),
    End,
    MakeCatcher(String),
    MakeRunner(String),
    Status {
        raw_challenges: Vec<RawChallenge>,
        players: Vec<Player>,
        teams: Vec<TeamOwningPlayers>,
    },
    Error(Error),
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
    SessionNotFound(String),
}
