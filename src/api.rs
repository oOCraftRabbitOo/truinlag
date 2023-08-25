use crate::commands::{Command, Response};
use crate::error::Result;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub struct Connection {
    transport: Framed<UnixStream, LengthDelimitedCodec>,
}

impl Connection {
    pub async fn create(addr: String) -> Result<Connection> {
        todo!();
    }

    pub async fn send(command: Command) -> Result<()> {
        todo!();
    }

    pub async fn recv() -> Result<Response> {
        todo!()
    }
}
