use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::error::RuntimeError;
use crate::state::{LastSessionUsage, SessionUsageSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct QuickSessionRuntimeState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<SessionUsageSnapshot>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub estimated_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_usage: Option<LastSessionUsage>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempted_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_input_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_line: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub context_generation: u64,
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl QuickSessionRuntimeState {
    pub fn state_path(workspace_root: &Path, session_id: &str) -> Result<PathBuf, RuntimeError> {
        gitim_core::types::validate_quick_session_id(session_id).map_err(|error| {
            RuntimeError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                error.to_string(),
            ))
        })?;
        Ok(workspace_root
            .join(".gitim-runtime/quick-sessions")
            .join(format!("{session_id}.state.json")))
    }

    pub fn load(workspace_root: &Path, session_id: &str) -> Result<Self, RuntimeError> {
        let path = Self::state_path(workspace_root, session_id)?;
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_str(&content) {
            Ok(state) => Ok(state),
            Err(_) => {
                let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.fZ");
                let quarantine = path.with_file_name(format!(
                    "{}.corrupt-{timestamp}",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("state.json")
                ));
                std::fs::rename(path, quarantine)?;
                Ok(Self::default())
            }
        }
    }

    pub fn save(&self, workspace_root: &Path, session_id: &str) -> Result<(), RuntimeError> {
        let path = Self::state_path(workspace_root, session_id)?;
        let parent = path.parent().ok_or_else(|| {
            RuntimeError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "quick session state path has no parent",
            ))
        })?;
        std::fs::create_dir_all(parent)?;
        let serialized = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(&serialized)?;
        temporary.as_file_mut().sync_all()?;
        chmod_0600(temporary.path())?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn bump_context_generation(&mut self) -> u64 {
        self.context_generation = self.context_generation.saturating_add(1);
        self.context_generation
    }
}

#[cfg(unix)]
fn chmod_0600(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn chmod_0600(_path: &Path) -> io::Result<()> {
    Ok(())
}
