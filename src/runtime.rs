use crate::engine;
use crate::error;
use crate::error::Result;
use async_broadcast as broadcast;
use std::future::Future;
use std::marker::Unpin;
use std::sync::{Arc, Mutex};
use tokio::net;
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinError;
use tokio::time::Duration;
use truinlag::commands;
use truinlag::commands::*;

#[derive(Debug)]
pub enum EngineSignal {
    Command {
        command: commands::EngineCommandPackage,
        channel: oneshot::Sender<IOSignal>,
    },
    BroadcastRequest(oneshot::Sender<broadcast::Receiver<IOSignal>>),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum IOSignal {
    Command(commands::ClientCommand),
    Shutdown,
}

pub async fn manager() -> Result<()> {
    let (mpsc_tx, mpsc_rx) = mpsc::channel::<EngineSignal>(1024);

    let mpsc_tx_staller = mpsc_tx.clone();
    // this is just a bad workaround to ensure that the mpsc channel is not closed if all clients
    // disconnect (update: the manager needs this channel too, so this is not true anymore)

    let (broadcast_tx, broadcast_rx_staller) = broadcast::broadcast::<IOSignal>(1024);
    let _broadcast_rx_staller = broadcast_rx_staller.deactivate();

    let (oneshot_tx, oneshot_rx) = oneshot::channel::<()>();

    println!("Manager: starting engine");
    let engine_handle =
        tokio::spawn(async move { engine(mpsc_rx, broadcast_tx, oneshot_tx).await });

    println!("Manager: starting ctrlc");
    let ctrlc_tx = mpsc_tx_staller.clone();
    let ctrlc_handle = tokio::spawn(async move { ctrlc(ctrlc_tx).await });

    // I could have just used an mpsc channel I think. That's all I do with this mutex.
    // I dump the io task futures into there and then, later, I take them all out.
    // Why did I use a mutex???
    // I guess this way there is no hard limit on the amount of connections...
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
            sender: mpsc::Sender<EngineSignal>,
            tasks: Arc<Mutex<Vec<Box<dyn Future<Output = Result<(), JoinError>> + Unpin>>>>,
            addr: tokio::net::unix::SocketAddr,
        ) -> Result<()> {
            let (broadcast_rx_tx, broadcast_rx_rx) = oneshot::channel();
            sender
                .send(EngineSignal::BroadcastRequest(broadcast_rx_tx))
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

        println!("Manager: awaiting ctrlc");

        ctrlc_handle
            .await
            .unwrap_or_else(|err| eprint!("Manager: error awaiting ctrlc: {}", err));
    };

    let timeout = async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        eprintln!("Manager: could not await all tasks within three seconds, aborting")
    };

    select! {
        _ = await_io_tasks => {}
        _ = timeout => {}
    };

    println!("Manager: removing socket file");

    tokio::fs::remove_file(socket)
        .await
        .expect("couldn't remove socket file while shutting down");

    println!("cya");

    Ok(())
}

async fn engine(
    mut mpsc_handle: mpsc::Receiver<EngineSignal>,
    broadcast_handle: broadcast::Sender<IOSignal>,
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
            EngineSignal::Command {
                command: package,
                channel,
            } => {
                let response = engine::vroom(package.command);
                if let Some(action) = response.broadcast_action {
                    let message = IOSignal::Command(ClientCommand::Broadcast(action));
                    if broadcast_handle.is_full() {
                        println!(
                            "Engine: broadcast full, {} receivers",
                            broadcast_handle.receiver_count()
                        )
                    }
                    if let Err(err) = broadcast_handle.broadcast(message).await {
                        println!("{}: {}", broadcast_send_error, err);
                    };
                }
                channel.send(IOSignal::Command(ClientCommand::Response(ResponsePackage {
                    action: response.response_action,
                    id: package.id
                }))).unwrap_or_else(|_err| println!("Engine: Couldn't send response to IO task, assuming client disconnect and continuing"));
            }
            EngineSignal::BroadcastRequest(oneshot_sender) => {
                oneshot_sender
                    .send(broadcast_handle.new_receiver())
                    .unwrap_or_else(move |_handle| {
                        println!("Engine: couldn't send broadcast handle")
                    });
            }
            EngineSignal::Shutdown => {
                println!("Engine: shutdown signal received");
                break;
            }
        };
    }

    oneshot_handle.send(()).expect("Engine: The oneshot channel should not close before something is sent, manager has no reason to drop it");
    if broadcast_handle.is_full() {
        println!(
            "Engine: broadcast full, {} receivers",
            broadcast_handle.receiver_count()
        )
    }
    let _schmeceiver = broadcast_handle.new_receiver(); //sending doesn't work otherwise
    println!(
        "Engine: {} broadcast receivers",
        broadcast_handle.receiver_count()
    );
    broadcast_handle
        .broadcast(IOSignal::Shutdown)
        .await
        .expect(broadcast_send_error);

    Ok(())
}

// This function is a confusing unreadable mess.
async fn io(
    tx: mpsc::Sender<EngineSignal>,
    rx: broadcast::Receiver<IOSignal>,
    stream: net::UnixStream,
    addr: tokio::net::unix::SocketAddr,
) {
    use bytes::Bytes;
    use futures::prelude::*;
    use futures::SinkExt;
    use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

    async fn engine_parser(
        mut rx: mpsc::Receiver<IOSignal>,
        stream: net::unix::OwnedWriteHalf,
    ) -> Result<()> {
        let mut transport = FramedWrite::new(stream, LengthDelimitedCodec::new());

        loop {
            match rx.recv().await.ok_or(error::Error::IDontCareAnymore)? {
                IOSignal::Shutdown => {
                    break;
                }
                IOSignal::Command(command) => {
                    let serialized = bincode::serialize(&command)?;
                    transport.send(Bytes::from(serialized)).await?;
                }
            };
        }
        Ok(())
    }

    async fn broadcast_fwd(
        mut rx: broadcast::Receiver<IOSignal>,
        tx: mpsc::Sender<IOSignal>,
    ) -> Result<()> {
        loop {
            tx.send(rx.recv().await?).await?;
        }
    }

    async fn response_fwd(
        mut rx: mpsc::Receiver<oneshot::Receiver<IOSignal>>,
        tx: mpsc::Sender<IOSignal>,
    ) -> Result<()> {
        loop {
            tx.send(
                rx.recv()
                    .await
                    .ok_or(error::Error::IDontCareAnymore)?
                    .await?,
            )
            .await?;
        }
    }

    async fn client_parser(
        tx: mpsc::Sender<EngineSignal>,
        recv_tx: mpsc::Sender<oneshot::Receiver<IOSignal>>,
        stream: net::unix::OwnedReadHalf,
    ) -> Result<()> {
        let mut transport = FramedRead::new(stream, LengthDelimitedCodec::new());

        while let Some(message) = transport.next().await {
            match message {
                Ok(val) => {
                    let (oneshot_send, oneshot_recv) = oneshot::channel();
                    let command: commands::EngineCommandPackage = bincode::deserialize(&val)?;
                    tx.send(EngineSignal::Command {
                        command,
                        channel: oneshot_send,
                    })
                    .await?;
                    recv_tx.send(oneshot_recv).await?;
                }
                Err(err) => return Err(err.into()),
            }
        }

        Ok(())
    }

    async fn wrapper(
        engine_tx: mpsc::Sender<EngineSignal>,
        engine_rx: broadcast::Receiver<IOSignal>,
        stream: net::UnixStream,
    ) -> Result<()> {
        let (read_stream, write_stream) = stream.into_split();

        let (client_tx, client_rx) = mpsc::channel(1024);
        let (recv_tx, recv_rx) = mpsc::channel(1024);
        let broadcast_relay_tx = client_tx.clone();

        select! {
            res = client_parser(engine_tx, recv_tx, read_stream) => res?,
            res = engine_parser(client_rx, write_stream) => res?,
            res = response_fwd(recv_rx, client_tx) => res?,
            res = broadcast_fwd(engine_rx, broadcast_relay_tx) => res?
        }

        Ok(())
    }

    match wrapper(tx, rx, stream).await {
        Ok(_) => println!("IO {:?}: terminated without error", addr),
        Err(err) => eprintln!("IO {:?}: {}", addr, err),
    }
}

async fn ctrlc(tx: mpsc::Sender<EngineSignal>) {
    tokio::signal::ctrl_c()
        .await
        .expect("should be able to await ctrl-c");
    println!("Keyboard interrupt");
    let _ = tx.send(EngineSignal::Shutdown).await;
}
