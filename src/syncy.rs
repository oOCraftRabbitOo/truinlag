use crate::commands;
use tokio::sync::{broadcast, oneshot};

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
