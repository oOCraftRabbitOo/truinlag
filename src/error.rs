use crate::syncy::EngineCommand;
use reqwest;
use ron;
use std;
use std::io;
use tokio;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Ron(ron::Error),
    Reqwest(reqwest::Error),
    Bincode(bincode::Error),
    BroadcastRecvError(broadcast::error::RecvError),
    MpscSendError(mpsc::error::SendError<EngineCommand>),
    OneshotRecvError(oneshot::error::RecvError),
    JoinError(JoinError),
    MutexError,
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
            Error::MpscSendError(err) => write!(f, "mpsc send error: {}", err),
            Error::OneshotRecvError(err) => write!(f, "oneshot recv error: {}", err),
            Error::JoinError(err) => write!(f, "error joining task: {}", err),
            Error::MutexError => write!(
                f,
                "Error acquiring mutex lock, other thread might have panicked"
            ),
            Error::Bincode(err) => write!(
                f,
                "ipc en/decode error, client might be incompatible: {}",
                err
            ),
            Error::BroadcastRecvError(err) => {
                write!(f, "error receiving message from engine {}", err)
            }
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

impl From<broadcast::error::RecvError> for Error {
    fn from(error: broadcast::error::RecvError) -> Self {
        Error::BroadcastRecvError(error)
    }
}

impl From<mpsc::error::SendError<EngineCommand>> for Error {
    fn from(error: mpsc::error::SendError<EngineCommand>) -> Self {
        Error::MpscSendError(error)
    }
}

impl From<oneshot::error::RecvError> for Error {
    fn from(error: oneshot::error::RecvError) -> Self {
        Error::OneshotRecvError(error)
    }
}

impl std::error::Error for Error {}
