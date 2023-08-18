use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Command {
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
    /*
    AssignTeam {
        player: String,
        team: String,
    },
    */
    Status,
    Stop,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Response {
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
    Error {
        command: Command,
        error: Error,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Error {
    TeamNotFound(String),
    ChallengeNotFound(String),
    EngineFaliure,
}
