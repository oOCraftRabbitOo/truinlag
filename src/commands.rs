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

impl From<ResponseAction> for EngineResponse {
    fn from(value: ResponseAction) -> Self {
        Self {
            response_action: value,
            broadcast_action: None,
        }
    }
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
    AddSession {
        name: String,
        mode: Mode,
    },
    AddPlayer {
        name: String,
        discord_id: Option<u64>,
        passphrase: String,
        session: Option<u64>,
    },
    AddTeam {
        name: String,
        discord_channel: Option<u64>,
        colour: Option<Colour>,
    },
    AssignPlayerToTeam {
        player: u64,
        team: Option<usize>,
    },
    SetPlayerSession {
        player: u64,
        session: Option<u64>,
    },
    SetPlayerName {
        player: u64,
        name: String,
    },
    SetPlayerPassphrase {
        player: u64,
        passphrase: String,
    },
    RemovePlayer {
        player: u64,
    },
    Catch {
        catcher: usize,
        caught: usize,
    },
    Complete {
        completer: usize,
        completed: usize,
    },
    SendLocation {
        player: u64,
        location: (f64, f64),
    },
    SetRawChallenge(RawChallenge),
    AddRawChallenge(RawChallenge),
    GetPlayerByPassphrase(String),
    GetRawChallenges,
    Start,
    Stop,
    Ping(Option<String>),
    GetState,
    MakeTeamCatcher(usize),
    MakeTeamRunner(usize),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ResponseAction {
    Error(Error),
    Team(Team),
    Player(Player),
    SendRawChallenges(Vec<RawChallenge>),
    SendState {
        teams: Vec<Team>,
        game: Option<Game>,
    },
    SendGlobalState {
        sessions: Vec<GameSession>,
        players: Vec<Player>,
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
    PlayerChangedSession {
        player: Player,
        from_session: Option<u64>,
        to_session: Option<u64>,
    },
    PlayerChangedTeam {
        session: u64,
        player: u64,
        from_team: Option<usize>,
        to_team: Option<usize>,
    },
    PlayerDeleted(Player),
    TeamMadeCatcher(Team),
    TeamMadeRunner(Team),
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
    BadData(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSessionSupplied => write!(f, "A sesssion specific command like 'addTeam' was called without a session being supplied."),
            Self::SessionSupplied => write!(f, "A session unspecific command like 'addPlayer' was called with a session."),
            Self::NotFound => write!(f, "Not Found"),
            Self::TeamExists(team) => write!(f, "Team {} already exists", team),
            Self::AlreadyExists => write!(f, "Already exists"),
            Self::GameInProgress => write!(f, "There is already a game in progress"),
            Self::GameNotRunning => write!(f, "There is no game in progress"),
            Self::AmbiguousData => write!(f, "Ambiguous data"),
            Self::InternalError => write!(f, "There was a truinlag-internal error"),
            Self::NotImplemented => write!(f, "Not yet implemented"),
            Self::BadData(text) => write!(f, "bad data: {}", text),
        }
    }
}
