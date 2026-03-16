use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use gitim_core::types::{Config, ThreadFile};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config) -> Self {
        Self {
            repo_root,
            config,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
        }
    }
}
