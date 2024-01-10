use crate::commands::{BroadcastAction, EngineCommand, EngineResponse, ResponseAction};
use std::path;

pub fn vroom(command: EngineCommand) -> EngineResponse {
    EngineResponse {
        response_action: ResponseAction::Success,
        broadcast_action: Some(BroadcastAction::Success),
    }
}

trait Session {}

struct Engine {
    sessions: Vec<Box<dyn Session>>,
}
