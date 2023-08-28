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

    pub async fn recv(&mut self) -> Result<Option<Response>> {
        match self.transport.next().await {
            Some(message) => Ok(Some(bincode::deserialize(&message?)?)),
            None => Ok(None),
        }
    }

    pub async fn close(mut self) -> Result<()> {
        self.transport.close().await?;
        Ok(())
    }

    pub async fn split(self) -> (RecvConnection, SendConnection) {
        let mut sock = self.transport.into_inner();
        let (read_half, write_half) = sock.split();

        todo!();
    }
}

pub struct SendConnection {
    transport: FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
}

impl SendConnection {
    pub async fn create(addr: String) -> Result<SendConnection> {
        let sock = UnixStream::connect(addr).await?;
        let (_, sock) = sock.into_split();
        let transport = FramedWrite::new(sock, LengthDelimitedCodec::new());

        Ok(SendConnection { transport })
    }

    pub async fn send(&mut self, command: &Command) -> Result<()> {
        let buf = Bytes::from(bincode::serialize(command)?);
        self.transport.send(buf).await?;
        Ok(())
    }

    pub async fn close(mut self) -> Result<()> {
        self.transport.close().await?;
        Ok(())
    }
}

pub struct RecvConnection {
    transport: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
}

impl RecvConnection {
    pub async fn create(addr: String) -> Result<RecvConnection> {
        let sock = UnixStream::connect(addr).await?;
        let (sock, _) = sock.into_split();
        let transport = FramedRead::new(sock, LengthDelimitedCodec::new());

        Ok(RecvConnection { transport })
    }

    pub async fn recv(&mut self) -> Result<Option<Response>> {
        match self.transport.next().await {
            Some(message) => Ok(Some(bincode::deserialize(&message?)?)),
            None => Ok(None),
        }
    }
}
