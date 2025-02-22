use crate::commands::{
    BroadcastAction, ClientCommand, EngineAction, EngineAction::*, EngineCommand,
    EngineCommandPackage, ResponseAction, ResponsePackage,
};
use crate::*;
use bytes::Bytes;
use error::{Error, Result};
use futures::prelude::*;
use futures::SinkExt;
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

pub mod error;

struct SendRequest {
    command: EngineCommand,
    response_channel: oneshot::Sender<ResponseAction>,
}

#[derive(Debug)]
struct ResponseInfo {
    id: u64,
    channel: oneshot::Sender<ResponseAction>,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum DistributorMessage {
    Command(ClientCommand),
    ResponseInfo(ResponseInfo),
    Err(Error),
}

async fn connectinator<R, W>(
    mut send_req_recv: mpsc::Receiver<SendRequest>,
    broadcast_send: mpsc::Sender<BroadcastAction>,
    socket_read: R,
    socket_write: W,
) -> Result<()>
where
    R: tokio::io::AsyncRead + std::marker::Unpin + std::marker::Send + 'static,
    W: tokio::io::AsyncWrite + std::marker::Unpin + std::marker::Send + 'static,
{
    let (command_send, mut dist_msg_recv) = mpsc::channel::<DistributorMessage>(1024);
    let response_info_send = command_send.clone();
    let res_inf_send_exp = "receiver should only be dropped once distributor shuts down, which also causes send_manager to shut down.";

    let send_manager = tokio::spawn(async move {
        let mut id = 0;
        let mut transport = FramedWrite::new(socket_write, LengthDelimitedCodec::new());
        while let Some(send_req) = send_req_recv.recv().await {
            response_info_send
                .send(DistributorMessage::ResponseInfo(ResponseInfo {
                    id,
                    channel: send_req.response_channel,
                }))
                .await
                .expect(res_inf_send_exp);
            let package = EngineCommandPackage {
                command: send_req.command,
                id,
            };
            let serialized =
                bincode::serialize(&package).expect("EngineCommand should always be serializable");
            if let Err(_err) = transport.send(Bytes::from(serialized)).await {
                response_info_send
                    .send(DistributorMessage::Err(Error::Disconnect))
                    .await
                    .expect(res_inf_send_exp);
                return Err(Error::Disconnect);
            }
            id += 1;
        }
        Ok(())
    });

    let receive_manager = tokio::spawn(async move {
        let mut transport = FramedRead::new(socket_read, LengthDelimitedCodec::new());
        while let Some(message) = transport.next().await {
            match message {
                Ok(message) => {
                    let command: ClientCommand = bincode::deserialize(&message).map_err(|err| {
                        Error::InvalidSignal(format!("deserialisation error: {}", err))
                    })?;
                    command_send
                        .send(DistributorMessage::Command(command))
                        .await
                        .map_err(|_| Error::Disconnect)?;
                }
                Err(_err) => {
                    command_send
                        .send(DistributorMessage::Err(Error::Disconnect))
                        .await
                        .map_err(|_| Error::Disconnect)?;
                    return Err(Error::Disconnect);
                }
            }
        }
        Ok(())
    });

    let distributor = tokio::spawn(async move {
        let mut info_cache = std::vec::Vec::<ResponseInfo>::new();
        let mut msg_cache = std::vec::Vec::<ResponsePackage>::new();
        while let Some(message) = dist_msg_recv.recv().await {
            match message {
                DistributorMessage::ResponseInfo(info) => {
                    if let Some(message_index) = msg_cache.iter().position(|pkg| pkg.id == info.id)
                    {
                        info.channel
                            .send(msg_cache.swap_remove(message_index).action)
                            .ok();
                    } else {
                        info_cache.push(info);
                    };
                }
                DistributorMessage::Command(command) => match command {
                    ClientCommand::Broadcast(msg) => {
                        broadcast_send.send(msg).await.expect(
                            "Receiver handle can only be dropped if JoinHandle is dropped too",
                        );
                    }
                    ClientCommand::Response(msg) => {
                        if let Some(info_index) = info_cache.iter().position(|inf| inf.id == msg.id)
                        {
                            info_cache
                                .swap_remove(info_index)
                                .channel
                                .send(msg.action.clone())
                                .ok();
                        } else {
                            msg_cache.push(msg);
                        }
                    }
                },
                DistributorMessage::Err(_error) => {
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = distributor => {},
        _ = receive_manager => {},
    };

    send_manager.abort();

    Ok(())
}

pub async fn connect(address: Option<&str>) -> Result<(SendConnection, InactiveRecvConnection)> {
    let (socket_read, socket_write) = UnixStream::connect(address.unwrap_or("/tmp/truinsocket"))
        .await
        .map_err(|err| Error::Connection(err))?
        .into_split();
    insert_connection(socket_read, socket_write).await
}

pub async fn insert_connection<R, W>(
    read: R,
    write: W,
) -> Result<(SendConnection, InactiveRecvConnection)>
where
    R: tokio::io::AsyncRead + std::marker::Unpin + std::marker::Send + 'static,
    W: tokio::io::AsyncWrite + std::marker::Unpin + std::marker::Send + 'static,
{
    let (broadcast_send, broadcast_recv) = mpsc::channel(1024);
    let (send_req_send, send_req_recv) = mpsc::channel(1024);
    let handle =
        tokio::spawn(
            async move { connectinator(send_req_recv, broadcast_send, read, write).await },
        );

    Ok((
        SendConnection { send_req_send },
        RecvConnection {
            broadcast_recv,
            handle,
        }
        .deactivate()
        .await,
    ))
}

#[derive(Clone)]
pub struct SendConnection {
    send_req_send: mpsc::Sender<SendRequest>,
}

impl SendConnection {
    pub async fn send(&mut self, command: EngineCommand) -> Result<ResponseAction> {
        let (resp_send, resp_recv) = oneshot::channel();
        let package = SendRequest {
            command,
            response_channel: resp_send,
        };
        self.send_req_send
            .send(package)
            .await
            .map_err(|_| Error::Disconnect)?;
        resp_recv.await.map_err(|_| Error::Disconnect)
    }

    pub async fn get_zones(&mut self) -> Result<Vec<Zone>> {
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::GetAllZones,
            })
            .await
        {
            Ok(response) => match response {
                ResponseAction::SendZones(zones) => Ok(zones),
                ResponseAction::Error(err) => Err(Error::Truinlag(err)),
                other => Err(Error::InvalidSignal(format!("{:?}", other))),
            },
            Err(err) => Err(err),
        }
    }

    pub async fn delete_all_challenges(&mut self) -> Result<()> {
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::DeleteAllChallenges,
            })
            .await
        {
            Ok(response) => match response {
                ResponseAction::Success => Ok(()),
                ResponseAction::Error(err) => Err(Error::Truinlag(err)),
                other => Err(Error::InvalidSignal(format!("{:?}", other))),
            },
            Err(err) => Err(err),
        }
    }

    pub async fn get_global_state(&mut self) -> Result<(Vec<GameSession>, Vec<Player>)> {
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::GetState,
            })
            .await
        {
            Ok(thing) => match thing {
                ResponseAction::SendGlobalState { sessions, players } => Ok((sessions, players)),
                other => Err(Error::InvalidSignal(format!("{:?}", other))),
            },
            Err(err) => Err(err),
        }
    }

    pub async fn get_raw_challenges(&mut self) -> Result<Vec<RawChallenge>> {
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::GetRawChallenges,
            })
            .await?
        {
            ResponseAction::Error(err) => Err(Error::Truinlag(err)),
            ResponseAction::SendRawChallenges(challenges) => Ok(challenges),
            other => Err(Error::InvalidSignal(format!("{:?}", other))),
        }
    }

    pub async fn set_raw_challenge<C>(&mut self, challenge: C) -> Result<()>
    where
        C: Into<InputChallenge>,
    {
        let challenge = challenge.into();
        if challenge.id.is_none() {
            return Err(Error::InvalidSignal("Provided challenge has no ID".into()));
        }
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::SetRawChallenge(challenge),
            })
            .await?
        {
            ResponseAction::Error(err) => Err(Error::Truinlag(err)),
            ResponseAction::Success => Ok(()),
            other => Err(Error::InvalidSignal(format!("{:?}", other))),
        }
    }

    pub async fn add_raw_challenge<C>(&mut self, challenge: C) -> Result<()>
    where
        C: Into<InputChallenge>,
    {
        let challenge = challenge.into();
        if challenge.id.is_some() {
            return Err(Error::InvalidSignal(
                "supplied challenge must have an ID".into(),
            ));
        }
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::AddRawChallenge(challenge),
            })
            .await?
        {
            ResponseAction::Error(err) => Err(Error::Truinlag(err)),
            ResponseAction::Success => Ok(()),
            other => Err(Error::InvalidSignal(format!("{:?}", other))),
        }
    }

    pub async fn add_challenge_set(&mut self, name: String) -> Result<()> {
        match self
            .send(EngineCommand {
                session: None,
                action: EngineAction::AddChallengeSet(name),
            })
            .await?
        {
            ResponseAction::Error(err) => Err(Error::Truinlag(err)),
            ResponseAction::Success => Ok(()),
            other => Err(Error::InvalidSignal(format!("{:?}", other))),
        }
    }

    pub async fn get_challenge_sets(&mut self) -> Result<Vec<ChallengeSet>> {
        match self
            .send(EngineCommand {
                session: None,
                action: GetChallengeSets,
            })
            .await?
        {
            ResponseAction::Error(err) => Err(Error::Truinlag(err)),
            ResponseAction::SendChallengeSets(sets) => Ok(sets),
            other => Err(Error::InvalidSignal(format!("{:?}", other))),
        }
    }
}

pub struct RecvConnection {
    broadcast_recv: mpsc::Receiver<BroadcastAction>,
    handle: tokio::task::JoinHandle<Result<()>>,
}

impl RecvConnection {
    pub async fn recv(&mut self) -> Option<BroadcastAction> {
        self.broadcast_recv.recv().await
    }

    pub async fn disconnect(self) {
        self.handle.abort()
    }

    pub async fn deactivate(self) -> InactiveRecvConnection {
        let broadcast_recv = Arc::new(Mutex::new(self.broadcast_recv));
        let inner_recv = broadcast_recv.clone();
        let eater_handle = tokio::spawn(async move {
            let mut inner_recv = inner_recv.lock().await;
            while inner_recv.recv().await.is_some() {}
        });
        InactiveRecvConnection {
            broadcast_recv,
            eater_handle,
            handle: self.handle,
        }
    }
}

pub struct InactiveRecvConnection {
    broadcast_recv: Arc<Mutex<mpsc::Receiver<BroadcastAction>>>,
    eater_handle: tokio::task::JoinHandle<()>,
    handle: tokio::task::JoinHandle<Result<()>>,
}

impl InactiveRecvConnection {
    pub async fn activate(self) -> RecvConnection {
        self.eater_handle.abort();
        let _ = self.eater_handle.await;
        let broadcast_recv = Arc::into_inner(self.broadcast_recv).unwrap().into_inner();
        RecvConnection {
            handle: self.handle,
            broadcast_recv,
        }
    }

    pub async fn disconnect(self) {
        self.eater_handle.abort();
        self.handle.abort();
    }
}
