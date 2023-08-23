use crate::syncy::{ClientCommand, EngineCommand};
use bincode;
use std::fs;
use std::future::Future;
use std::marker::Unpin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net;
use tokio::select;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinError;
use truinlag::error;
use truinlag::error::Result;
use truinlag::*;

/*
fn handle_requests(state: &mut GameState) -> Result<()> {
    //TODO: Proper error handling within this function, currently crashes too easily
    use std::io::{Read, Write};

    fn handle_client(mut stream: net::UnixStream, state: &mut GameState) -> Result<bool> {
        // Read data from client
        let mut buf: [u8; 1024] = [0; 1024];
        let bytes_read: usize = stream.read(&mut buf)?;

        // Deserialize the data using Serde and MessagePack
        let deserialized: commands::Command = bincode::deserialize_from(&buf[..bytes_read])?;

        let response = String::from("server not implemented");

        // Serialize the response data using Serde and MessagePack
        let serialized = bincode::serialize(&response).unwrap();

        // Send the response back to the client
        stream.write(&serialized).unwrap();

        Ok(true)
    }

    let listener =
        net::UnixListener::bind("/tmp/truinsocket").expect("should be able to bind to socket (maybe different session running or last session improperly terminated)");

    // Start accepting incoming connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // handle communication with each client
                if !handle_client(stream, state)? {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Error while accepting a new connection:\n{}", e);
            }
        }
    }

    Ok(())
}
*/

fn handle_command(command: commands::Command, state: &mut GameState) -> commands::Response {
    use truinlag::commands::Command;
    match command {
        Command::Catch { catcher, caught } => todo!(),
        Command::Complete {
            completer,
            completed,
        } => todo!(),
        Command::End => todo!(),
        Command::MakeCatcher(team) => todo!(),
        Command::MakeRunner(team) => todo!(),
        Command::Start(config) => todo!(),
        Command::Status => todo!(),
        Command::Stop => todo!(),
    }
}

fn get_config(filename: &str) -> Result<Config> {
    Ok(ron::from_str(&fs::read_to_string(&filename)?)?)
}

fn get_players(filename: &str) -> Result<Vec<Player>> {
    Ok(ron::from_str(&fs::read_to_string(&filename)?)?)
}

fn generate_teams<'a>(filename: &str, players: &'a Vec<Player>) -> Result<Vec<Team<'a>>> {
    let raw_teams: Vec<RawTeam> = ron::from_str(&fs::read_to_string(&filename)?)?;
    Ok(raw_teams
        .iter()
        .map(|raw_team| {
            Team::new(
                raw_team.name.clone(),
                players
                    .iter()
                    .filter(|player| {
                        raw_team.players.iter().any(|raw_player| {
                            match players.iter().find(|player| player.name == *raw_player) {
                                Some(_) => true,
                                None => false,
                            }
                        })
                    })
                    .collect(),
            )
        })
        .collect())
    // TODO: return LoadError::PlayerNotFound when necessary
}

fn start_game() {
    let config_file = "config.ron";
    let config = get_config(config_file).unwrap_or_else(|error| {
        println!(
            "Something went wrong reading config file at {}, loading default:\n{}",
            config_file, error
        );
        default_config()
    });

    let player_file = "players.ron";
    let players = get_players(player_file).unwrap_or_else(|error| {
        panic!(
            "Something went wrong reading player file at {}:\n{}",
            player_file, error
        );
    });

    let team_file = "teams.ron";
    let teams = generate_teams(team_file, &players).unwrap_or_else(|error| {
        panic!(
            "Something went wrong generating teams from {}:\n{}",
            team_file, error
        )
    });
}

/*
#[derive(Debug)]
enum EngineCommand {
    Command(commands::Command),
    BroadcastRequest(oneshot::Sender<broadcast::Receiver<ClientCommand>>),
    Shutdown,
}

#[derive(Clone, Debug)]
enum ClientCommand {
    Command(commands::Response),
    Shutdown,
}
*/

#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();

    Ok(())
}

async fn manager() -> Result<()> {
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
    async fn engine_parser(
        mut rx: broadcast::Receiver<ClientCommand>,
        stream: net::unix::OwnedWriteHalf,
    ) -> Result<()> {
        loop {
            match rx.recv().await? {
                ClientCommand::Shutdown => {
                    break;
                }
                ClientCommand::Command(command) => {
                    let serialized = bincode::serialize(&command)?;
                    stream.writable().await?;
                    stream.try_write(&serialized)?;
                }
            };
        }
        Ok(())
    }

    async fn client_parser(
        tx: mpsc::Sender<EngineCommand>,
        stream: net::unix::OwnedReadHalf,
    ) -> Result<()> {
        loop {
            let mut buf: [u8; 1024] = [0; 1024];

            stream.readable().await?;
            let bytes_read = stream.try_read(&mut buf)?;

            if bytes_read == 0 {
                break;
            }

            let command: commands::Command = bincode::deserialize_from(&buf[..bytes_read])?;

            tx.send(EngineCommand::Command(command)).await?;
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
