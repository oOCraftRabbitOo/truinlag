use reqwest;
use ron;
use serde::{Deserialize, Serialize};
use std;
use std::io;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Ron(ron::Error),
    Reqwest(reqwest::Error),
    Bincode(bincode::Error),
    PlayerNotFound {
        player_name: String,
        team_name: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(err) => write!(f, "I/O error: {}", err),
            Error::Ron(err) => write!(f, "Deserialisation error: {}", err),
            Error::Reqwest(err) => write!(f, "http reqwest error: {}", err),
            Error::Bincode(err) => write!(f, "ipc en/decode error: {}", err),
            Error::PlayerNotFound {
                player_name,
                team_name,
            } => write!(
                f,
                "Player {} listed in team {} but couldn't be found",
                player_name, team_name
            ),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<ron::Error> for Error {
    fn from(error: ron::Error) -> Self {
        Error::Ron(error)
    }
}

impl From<ron::error::SpannedError> for Error {
    fn from(error: ron::error::SpannedError) -> Self {
        error.code.into()
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Reqwest(error)
    }
}

impl From<bincode::Error> for Error {
    fn from(error: bincode::Error) -> Self {
        Error::Bincode(error)
    }
}

impl std::error::Error for Error {}
