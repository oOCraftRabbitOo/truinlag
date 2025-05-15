use crate::runtime::{EngineSignal, IOSignal};
use async_broadcast as broadcast;
use std::io;
use tokio::sync::{mpsc, oneshot};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Reqwest(reqwest::Error),
    Bincode(bincode::Error),
    BroadcastRecv(broadcast::RecvError),
    BroadcastSend(Box<broadcast::SendError<IOSignal>>),
    MpscSend(mpsc::error::SendError<EngineSignal>),
    ClientCommandSend(Box<mpsc::error::SendError<IOSignal>>),
    MpscOneshotRecvSend(mpsc::error::SendError<oneshot::Receiver<IOSignal>>),
    OneshotRecv(oneshot::error::RecvError),
    IDontCareAnymore,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(err) => write!(f, "I/O error: {}", err),
            Error::Reqwest(err) => write!(f, "http reqwest error: {}", err),
            Error::MpscSend(err) => write!(f, "Couldn't send message to engine: {}", err),
            Error::ClientCommandSend(err) => write!(f, "mpsc send error: {}", err),
            Error::OneshotRecv(err) => write!(f, "Couldn't recv message from engine: {}", err),
            Error::BroadcastSend(err) => write!(f, "Couldn't send broadcast: {}", err),
            Error::MpscOneshotRecvSend(err) => {
                write!(f, "Couldn't send the oneshot_recv: {}", err)
            }
            Error::IDontCareAnymore => write!(f, "a miscellaneous error occured"),
            Error::Bincode(err) => write!(
                f,
                "ipc en/decode error, client might be incompatible: {}",
                err
            ),
            Error::BroadcastRecv(err) => {
                write!(f, "error receiving message from engine {}", err)
            }
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
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

impl From<broadcast::RecvError> for Error {
    fn from(error: broadcast::RecvError) -> Self {
        Error::BroadcastRecv(error)
    }
}

impl From<mpsc::error::SendError<EngineSignal>> for Error {
    fn from(error: mpsc::error::SendError<EngineSignal>) -> Self {
        Error::MpscSend(error)
    }
}

impl From<mpsc::error::SendError<IOSignal>> for Error {
    fn from(error: mpsc::error::SendError<IOSignal>) -> Self {
        Error::ClientCommandSend(Box::new(error))
    }
}

impl From<mpsc::error::SendError<oneshot::Receiver<IOSignal>>> for Error {
    fn from(error: mpsc::error::SendError<oneshot::Receiver<IOSignal>>) -> Self {
        Error::MpscOneshotRecvSend(error)
    }
}

impl From<oneshot::error::RecvError> for Error {
    fn from(error: oneshot::error::RecvError) -> Self {
        Error::OneshotRecv(error)
    }
}

impl From<broadcast::SendError<IOSignal>> for Error {
    fn from(error: broadcast::SendError<IOSignal>) -> Self {
        Error::BroadcastSend(Box::new(error))
    }
}

impl std::error::Error for Error {}
