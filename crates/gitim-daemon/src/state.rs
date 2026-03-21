use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use gitim_core::types::{Config, ThreadFile};
use gitim_sync::git::GitStorage;
use crate::api::Event;

pub type SharedState = Arc<AppState>;

#[derive(Clone, Debug)]
pub struct PendingMessage {
    pub channel: String,
    pub line_number: u64,
}

pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub git_storage: GitStorage,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
    pub event_tx: broadcast::Sender<Event>,
    pub current_user: RwLock<Option<String>>,
    pub pending_push: std::sync::RwLock<Vec<PendingMessage>>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config, event_tx: broadcast::Sender<Event>, current_user: Option<String>) -> Self {
        let git_storage = GitStorage::new(&repo_root);
        Self {
            repo_root,
            config,
            git_storage,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
            event_tx,
            current_user: RwLock::new(current_user),
            pending_push: std::sync::RwLock::new(Vec::new()),
        }
    }
}
