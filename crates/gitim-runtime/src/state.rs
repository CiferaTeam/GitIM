use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

impl AgentState {
    pub fn state_path(repo_root: &Path) -> PathBuf {
        repo_root.join(".gitim/agent-state.json")
    }

    pub fn load(repo_root: &Path) -> Result<Self, RuntimeError> {
        let path = Self::state_path(repo_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)
            .map_err(|e| RuntimeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    pub fn save(&self, repo_root: &Path) -> Result<(), RuntimeError> {
        let path = Self::state_path(repo_root);
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| RuntimeError::Io(std::io::Error::other(e)))?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}
