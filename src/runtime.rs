use crate::commands;
use crate::error;
use crate::error::Result;
use std::future::Future;
use std::marker::Unpin;
use std::sync::{Arc, Mutex};
use tokio::net;
use tokio::select;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinError;
use tokio::time::Duration;

#[derive(Debug)]
pub enum EngineCommand {
    Command(commands::Command),
    BroadcastRequest(oneshot::Sender<broadcast::Receiver<ClientCommand>>),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum ClientCommand {
    Command(commands::Response),
    Shutdown,
}

pub async fn manager() -> Result<()> {
    let (mpsc_tx, mpsc_rx) = mpsc::channel::<EngineCommand>(1024);

    let mpsc_tx_staller = mpsc_tx.clone();
    // this is just a bad workaround to ensure that the mpsc channel is not closed if all clients
    // disconnect (update: the manager needs this channel too, so this is not true anymore)

    let (broadcast_tx, _broadcast_rx_staller) = broadcast::channel::<ClientCommand>(1024);

    let (oneshot_tx, oneshot_rx) = oneshot::channel::<()>();

    println!("Manager: starting engine");
    let engine_handle =
        tokio::spawn(async move { engine(mpsc_rx, broadcast_tx, oneshot_tx).await });

    let io_tasks = Arc::new(Mutex::new(Vec::<
        Box<dyn Future<Output = Result<(), JoinError>> + Unpin>,
    >::new()));

    let io_tasks_2 = io_tasks.clone();

    let socket = "/tmp/truinsocket";

    println!("Manager: binding to socket {}", socket);
    let listener = net::UnixListener::bind(socket).expect(
        "Manager: cannot bind to socket (maybe other session running, session improperly terminated, etc.)",
    );

    let accept_connections = async move {
        async fn make_io_task(
            stream: net::UnixStream,
            sender: mpsc::Sender<EngineCommand>,
            tasks: Arc<Mutex<Vec<Box<dyn Future<Output = Result<(), JoinError>> + Unpin>>>>,
            addr: tokio::net::unix::SocketAddr,
        ) -> Result<()> {
            let (broadcast_rx_tx, broadcast_rx_rx) = oneshot::channel();
            sender
                .send(EngineCommand::BroadcastRequest(broadcast_rx_tx))
                .await?;
            let broadcast_rx = broadcast_rx_rx.await?;

            let io_handle = tokio::spawn(async move {
                io(sender, broadcast_rx, stream, addr).await;
            });

            let mut tasks = tasks.lock().map_err(|_| error::Error::MutexError)?;

            tasks.push(Box::new(io_handle));

            Ok(())
        }

        println!("Manager: starting to accept new connections");

        loop {
            let stream = listener.accept().await;

            match stream {
                Ok((stream, addr)) => {
                    println!("Manager: accepted new connection: {:?}", addr);
                    make_io_task(stream, mpsc_tx_staller.clone(), io_tasks_2.clone(), addr)
                        .await
                        .unwrap_or_else(|err| {
                            eprintln!(
                                "Manager: Encountered an error creating new i/o task, continuing: {}",
                                err
                            )
                        });
                }
                Err(err) => eprintln!(
                    "Manager: Error accepting new connection, continuing: {}",
                    err
                ),
            };
        }
    };

    let wait_for_shutdown = async move {
        oneshot_rx.await.unwrap_or_else(|err| {
            eprintln!(
                "Manager: The engine dropped the oneshot_tx, shutting down: {}",
                err
            )
        });
    };

    select! {
        _ = accept_connections => {
            eprintln!("Manager: The loop for accepting connections stopped (bad), shutting down");
        }
        _ = wait_for_shutdown => {
            eprintln!("Manager: shutdown triggered, stopping to accept connections");
        }
    };

    println!("Manager: shutting down");

    let await_io_tasks = async move {
        let mut io_tasks = io_tasks.lock().unwrap();

        println!("Manager: awaiting io tasks");

        for task in io_tasks.iter_mut() {
            match task.await {
                Ok(_) => {}
                Err(err) => eprintln!("Manager: error joining io task: {}", err),
            }
        }

        println!("Manager: awaiting engine");

        match engine_handle.await {
            Ok(res) => res.unwrap_or_else(|err| eprintln!("Manager: engine error: {}", err)),
            Err(err) => eprintln!("Manager: engine panicked: {}", err),
        }
    };

    let timeout = async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        eprintln!("Manager: could not await all tasks within three seconds, aborting")
    };

    select! {
        _ = await_io_tasks => {}
        _ = timeout => {}
    };

    println!("cya");

    Ok(())
}

async fn engine(
    mut mpsc_handle: mpsc::Receiver<EngineCommand>,
    broadcast_handle: broadcast::Sender<ClientCommand>,
    oneshot_handle: oneshot::Sender<()>,
) -> Result<()> {
    let broadcast_send_error =
        "Engine: The broadcast channel should never be closed because of `_broadcast_rx_staller`";
    loop {
        match mpsc_handle
            .recv()
            .await
            .expect("Engine: Thanks to `mpsc_tx_staller`, the channel should never be closed.")
        {
            EngineCommand::Command(command) => {
                broadcast_handle
                    .send(ClientCommand::Command(commands::Response::Error {
                        command,
                        error: commands::Error::EngineFaliure,
                    }))
                    .expect(broadcast_send_error);
            }
            EngineCommand::BroadcastRequest(oneshot_sender) => {
                oneshot_sender
                    .send(broadcast_handle.subscribe())
                    .unwrap_or_else(move |_handle| {});
            }
            EngineCommand::Shutdown => {
                broadcast_handle
                    .send(ClientCommand::Shutdown)
                    .expect(broadcast_send_error);
                break;
            }
        };
    }

    oneshot_handle.send(()).expect("Engine: The oneshot channel should not close before something is sent, manager has no reason to drop it");
    broadcast_handle
        .send(ClientCommand::Shutdown)
        .expect(broadcast_send_error);

    Ok(())
}

async fn io(
    tx: mpsc::Sender<EngineCommand>,
    rx: broadcast::Receiver<ClientCommand>,
    stream: net::UnixStream,
    addr: tokio::net::unix::SocketAddr,
) {
    use bytes::Bytes;
    use futures::prelude::*;
    use futures::SinkExt;
    use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

    async fn engine_parser(
        mut rx: broadcast::Receiver<ClientCommand>,
        stream: net::unix::OwnedWriteHalf,
    ) -> Result<()> {
        let mut transport = FramedWrite::new(stream, LengthDelimitedCodec::new());

        loop {
            match rx.recv().await? {
                ClientCommand::Shutdown => {
                    break;
                }
                ClientCommand::Command(command) => {
                    let serialized = bincode::serialize(&command)?;
                    transport.send(Bytes::from(serialized)).await?;
                }
            };
        }
        Ok(())
    }

    async fn client_parser(
        tx: mpsc::Sender<EngineCommand>,
        stream: net::unix::OwnedReadHalf,
    ) -> Result<()> {
        let mut transport = FramedRead::new(stream, LengthDelimitedCodec::new());

        while let Some(message) = transport.next().await {
            match message {
                Ok(val) => {
                    let command: commands::Command = bincode::deserialize(&val)?;
                    tx.send(EngineCommand::Command(command)).await?;
                }
                Err(err) => return Err(err.into()),
            }
        }

        Ok(())
    }

    async fn wrapper(
        tx: mpsc::Sender<EngineCommand>,
        rx: broadcast::Receiver<ClientCommand>,
        stream: net::UnixStream,
    ) -> Result<()> {
        let (read_stream, write_stream) = stream.into_split();

        select! {
            res = client_parser(tx, read_stream) => res?,
            res = engine_parser(rx, write_stream) => res?,
        }

        Ok(())
    }

    wrapper(tx, rx, stream)
        .await
        .unwrap_or_else(|error| eprintln!("IO {:?}: {}", addr, error));
}
