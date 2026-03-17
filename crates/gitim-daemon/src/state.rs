use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use gitim_core::types::{Config, ThreadFile};
use crate::api::Event;

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
    pub event_tx: broadcast::Sender<Event>,
    pub current_user: Option<String>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config, event_tx: broadcast::Sender<Event>, current_user: Option<String>) -> Self {
        Self {
            repo_root,
            config,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
            event_tx,
            current_user,
        }
    }
}
