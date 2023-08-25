use crate::commands::{Command, Response};
use crate::error::Result;
use bincode;
use bytes::Bytes;
use futures::prelude::*;
use futures::SinkExt;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};

pub struct Connection {
    transport: Framed<UnixStream, LengthDelimitedCodec>,
}

impl Connection {
    pub async fn create(addr: String) -> Result<Connection> {
        let sock = UnixStream::connect(addr).await?;
        let transport = Framed::new(sock, LengthDelimitedCodec::new());

        Ok(Connection { transport })
    }

    pub async fn send(&mut self, command: &Command) -> Result<()> {
        let buf = Bytes::from(bincode::serialize(command)?);
        self.transport.send(buf).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Response> {
        // The problem here is that .next() returns an option. How do i tell the user that the host
        // is down? I think imma go for Result<Option<Response>>.
        todo!();
    }

    pub async fn close(&mut self) -> Result<()> {
        self.transport.close().await?;
        Ok(())
    }

    pub async fn split(self) -> (RecvConnection, SendConnection) {
        todo!();
    }
}

pub struct SendConnection {
    transport: FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
}

pub struct RecvConnection {
    transport: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
}
