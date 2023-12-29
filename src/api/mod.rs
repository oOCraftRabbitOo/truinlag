use crate::commands::{
    BroadcastAction, ClientCommand, EngineCommand, EngineCommandPackage, ResponseAction,
    ResponsePackage,
};
use bytes::Bytes;
use error::{Error, Result};
use futures::prelude::*;
use futures::SinkExt;
use std::sync::Arc;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
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
enum DistributorMessage {
    Command(ClientCommand),
    ResponseInfo(ResponseInfo),
    Err(Error),
}

async fn connectinator(
    mut send_req_recv: mpsc::Receiver<SendRequest>,
    broadcast_send: mpsc::Sender<BroadcastAction>,
    socket_read: OwnedReadHalf,
    socket_write: OwnedWriteHalf,
) -> Result<()> {
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
                    let command: ClientCommand =
                        bincode::deserialize(&message).map_err(|_err| Error::InvalidSignal)?;
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
    let (broadcast_send, broadcast_recv) = mpsc::channel(1024);
    let (send_req_send, send_req_recv) = mpsc::channel(1024);
    let handle = tokio::spawn(async move {
        connectinator(send_req_recv, broadcast_send, socket_read, socket_write).await
    });

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
        Ok(resp_recv.await.map_err(|_| Error::Disconnect)?)
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
            while let Some(_) = inner_recv.recv().await {}
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
