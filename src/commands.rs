use super::*;
use serde::{Serialize, Deserialize}

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
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
    }
}

#[derive(Serialize, Deserialize)]
pub enum Error {
    TeamNotFound(String),
    ChallengeNotFound(String),
}