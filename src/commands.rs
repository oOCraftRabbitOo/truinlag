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
        catcher: usize,
        caught: usize,
    },
    Complete {
        completer: usize,
        completed: usize,
    },
    Location {
        player: u64,
        location: (f64, f64),
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
    Team(Team),
    Player(Player),
    SendState {
        teams: Vec<Team>,
        game: Option<Game>,
    },
    Success,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum BroadcastAction {
    Caught {
        catcher: Team,
        caught: Team,
    },
    Completed {
        completer: Team,
        completed: Challenge,
    },
    Started,
    Ended,
    Pinged(Option<String>),
    Location {
        team: usize,
        location: (f64, f64),
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Error {
    NoSessionSupplied, // Session specific commands like catch or add_team need a session
    SessionSupplied,   // Global commands like AddPlayer cannot be run with a session supplied
    NotFound,          // Element you were looking for wasn't found
    TeamExists(String), // You cannot create a team if one with a similar name already exists
    AlreadyExists,     // Things that already exist cannot be created
    GameInProgress,    // Commands like AddTeam cannot be run if a game is in progress
    GameNotRunning,    // Commands like catch can only be run if a game is in progress
    AmbiguousData,     // If multiple matching objects exist, e.g. players with passphrase lol
    InternalError,     // Some sort of internal database error
    NotImplemented,    // Feature is not yet implemented
}
