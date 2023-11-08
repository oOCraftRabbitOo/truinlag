use crate::commands::{BroadcastAction, EngineCommand, EngineResponse, ResponseAction};

pub fn vroom(command: EngineCommand) -> EngineResponse {
    EngineResponse {
        response_action: ResponseAction::Success,
        broadcast_action: Some(BroadcastAction::Success),
    }
}
