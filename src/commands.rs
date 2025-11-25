use std::num::NonZeroU32;

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
        period_id: usize,
    },
    Complete {
        completer: usize,
        completed: usize,
        period_id: usize,
    },
    SendLocation {
        player: u64,
        location: DetailedLocation,
    },
    SetRawChallenge(InputChallenge),
    AddRawChallenge(InputChallenge),
    GetPlayerByPassphrase(String),
    GetRawChallenges,
    Start,
    Stop,
    Ping(Option<String>),
    GetState,
    MakeTeamCatcher(usize),
    MakeTeamRunner(usize),
    AddChallengeToTeam {
        team: usize,
        challenge: Challenge,
    },
    RenameTeam {
        team: usize,
        new_name: String,
    },
    GenerateTeamChallenges(usize),
    AddChallengeSet(String),
    GetChallengeSets,
    DeleteAllChallenges,
    GetAllZones,
    AddZone {
        zone: u64,
        num_conn_zones: u64,
        num_connections: u64,
        train_through: bool,
        mongus: bool,
        s_bahn_zone: bool,
    },
    AddMinutesTo {
        from_zone: u64,
        to_zone: u64,
        minutes: u64,
    },
    GetEvents,
    UploadPeriodPictures {
        pictures: Vec<RawPicture>,
        team: usize,
        period: usize,
    },
    UploadTeamPicture {
        team_id: usize,
        picture: RawPicture,
    },
    UploadPlayerPicture {
        player_id: u64,
        picture: RawPicture,
    },
    GetThumbnails(Vec<u64>),
    GetPictures(Vec<u64>),
    GetLocations,
    GetPastLocations {
        team_id: usize,
        of_past_seconds: Option<NonZeroU32>,
    },
    GetGameConfig,
    SetGameConfig(PartialGameConfig),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ResponseAction {
    Error(Error),
    Team(Team),
    Player(Player),
    SendRawChallenges(Vec<RawChallenge>),
    SendState {
        teams: Vec<Team>,
        events: Vec<Event>,
        game: Option<Game>,
    },
    SendGlobalState {
        sessions: Vec<GameSession>,
        players: Vec<Player>,
    },
    Success,
    SendChallengeSets(Vec<ChallengeSet>),
    SendZones(Vec<Zone>),
    SendEvents(Vec<Event>),
    UploadedPictures(Vec<u64>),
    Period(usize),
    Pictures(Vec<Picture>),
    SendLocations(Vec<(Team, Vec<MinimalLocation>)>),
    SendPastLocations {
        team_id: usize,
        locations: Vec<MinimalLocation>,
    },
    SendGameConfig(GameConfig),
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
    Started {
        teams: Vec<Team>,
        game: Game,
    },
    Ended,
    Pinged(Option<String>),
    Location {
        team: usize,
        location: DetailedLocation,
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
    TeamLeftGracePeriod(Team),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Error {
    NoSessionSupplied, // Session specific commands like catch or add_team need a session
    SessionSupplied,   // Global commands like AddPlayer cannot be run with a session supplied
    NotFound(String),  // Element you were looking for wasn't found
    TeamExists(String), // You cannot create a team if one with a similar name already exists
    AlreadyExists,     // Things that already exist cannot be created
    GameInProgress,    // Commands like AddTeam cannot be run if a game is in progress
    GameNotRunning,    // Commands like catch can only be run if a game is in progress
    AmbiguousData,     // If multiple matching objects exist, e.g. players with passphrase lol
    InternalError,     // Some sort of internal database error
    NotImplemented,    // Feature is not yet implemented
    TeamIsRunner(usize), // A relevant team is runner, but has to be catcher
    TeamIsCatcher(usize), // A relevant team is catcher, but has to be runner
    TeamsTooFar,       // Two relevant teams are too far away from each other
    BadData(String),
    TextError(String), // Some other kind of error with a custom text
    PictureProblem,    // An Image-related error
    TooRapid,          // When requests are sent too rapidly
    TooFewChallenges,  // When there are too few challenges to start a game
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSessionSupplied => write!(f, "A sesssion specific command (like 'addTeam') was called without a session being supplied."),
            Self::SessionSupplied => write!(f, "A session unspecific command (like 'addPlayer') was called with a session."),
            Self::NotFound(ctx) => write!(f, "{} was not found", ctx),
            Self::TeamExists(team) => write!(f, "Team {} already exists", team),
            Self::AlreadyExists => write!(f, "Already exists"),
            Self::GameInProgress => write!(f, "There is already a game in progress"),
            Self::GameNotRunning => write!(f, "There is no game in progress"),
            Self::AmbiguousData => write!(f, "Ambiguous data"),
            Self::InternalError => write!(f, "There was a truinlag-internal error"),
            Self::NotImplemented => write!(f, "Not yet implemented"),
            Self::TeamIsRunner(team) => write!(f, "team {} is runner", team),
            Self::TeamIsCatcher(team) => write!(f, "team {} is catcher", team),
            Self::TeamsTooFar => write!(f, "the teams are too far away from each other"),
            Self::BadData(text) => write!(f, "bad data: {}", text),
            Self::TextError(text) => write!(f, "{}", text),
            Self::PictureProblem => write!(f, "there was a problem processing an image"),
            Self::TooRapid => write!(f, "not enough time has passed since the last request, hold your horses"),
            Self::TooFewChallenges => write!(f, "there are not enough challenges to start a game in the challenge db"),
        }
    }
}

impl std::error::Error for Error {}
