#[derive(Debug)]
pub enum Error {
    Disconnect,
    InvalidSignal,
    Connection(std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Disconnect => write!(f, "Disconnected from the Truinlag engine."),
            Error::InvalidSignal => write!(
                f,
                "Received invalid signal from engine (not able to deserialise to ClientCommand)."
            ),
            Error::Connection(err) => write!(f, "Couldn't connect: {}", err),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Connection(error)
    }
}

impl std::error::Error for Error {}
