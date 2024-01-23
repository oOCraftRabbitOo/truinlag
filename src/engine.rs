use crate::commands;
use crate::commands::{
    BroadcastAction, EngineAction, EngineCommand, EngineResponse, Mode, ResponseAction,
};
use bonsaidb::core::connection::AsyncStorageConnection;
use bonsaidb::core::schema::{Collection, Schema, SerializedCollection};
use bonsaidb::local::config::Builder;
use bonsaidb::local::{config, AsyncDatabase, AsyncStorage};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub fn vroom(command: EngineCommand) -> EngineResponse {
    EngineResponse {
        response_action: ResponseAction::Success,
        broadcast_action: Some(BroadcastAction::Success),
    }
}

struct Player {}

#[derive(Schema)]
#[schema(name="engine", collections=[SessionEntry])]
struct EngineSchema {}

#[derive(Collection, Serialize, Deserialize)]
#[collection(name = "session")]
struct SessionEntry {
    name: String,
    mode: Mode,
}

#[derive(Schema)]
#[schema(name="session", collections=[])]
struct SessionSchema {}

struct Session {
    name: String,
    db: AsyncDatabase,
    mode: Mode,
}

pub struct Engine {
    db: AsyncDatabase,
    store: AsyncStorage,
    sessions: Vec<Session>,
}

impl Session {
    async fn init(entry: SessionEntry, store: &AsyncStorage) -> Self {
        let db = store
            .create_database::<SessionSchema>(&entry.name, true)
            .await
            .unwrap();
        Session {
            name: entry.name,
            db,
            mode: entry.mode,
        }
    }

    async fn vroom(&self, command: EngineAction) -> EngineResponse {
        todo!();
    }
}

impl Engine {
    pub async fn init(storage_path: &Path) -> Self {
        let store = AsyncStorage::open(
            config::StorageConfiguration::new(storage_path)
                .with_schema::<EngineSchema>()
                .unwrap()
                .with_schema::<SessionSchema>()
                .unwrap(),
        )
        .await
        .unwrap();
        let db = store
            .create_database::<EngineSchema>("engine", true)
            .await
            .unwrap();

        let sessions = futures::future::join_all(
            SessionEntry::all_async(&db)
                .await
                .unwrap()
                .into_iter()
                .map(|doc| Session::init(doc.contents, &store)),
        )
        .await;

        Engine {
            store,
            db,
            sessions,
        }
    }

    pub async fn vroom(&self, command: EngineCommand) -> EngineResponse {
        match command.session {
            Some(name) => match self.sessions.iter().find(|item| item.name == name) {
                Some(session) => session.vroom(command.action).await,
                None => EngineResponse {
                    response_action: ResponseAction::Error(commands::Error::SessionNotFound(name)),
                    broadcast_action: None,
                },
            },
            None => todo!(),
        }
    }
}
