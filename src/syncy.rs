use crate::commands;
use tokio::sync::{broadcast, oneshot};

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
