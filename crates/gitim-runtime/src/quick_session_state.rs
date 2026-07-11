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
                quarantine_corrupt_file(&path, &timestamp.to_string())?;
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
        temporary.persist(&path).map_err(|error| error.error)?;
        sync_parent_dir(parent)?;
        Ok(())
    }

    pub fn bump_context_generation(&mut self) -> u64 {
        self.context_generation = self.context_generation.saturating_add(1);
        self.context_generation
    }
}

fn quarantine_corrupt_file(path: &Path, timestamp: &str) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "quick session state path has no parent",
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "quick session state path has no UTF-8 file name",
            )
        })?;
    let base_name = format!("{file_name}.corrupt-{timestamp}");
    chmod_0600(path)?;
    for collision_index in 0_u64.. {
        let candidate = if collision_index == 0 {
            parent.join(&base_name)
        } else {
            parent.join(format!("{base_name}-{collision_index}"))
        };
        match std::fs::hard_link(path, &candidate) {
            Ok(()) => {
                std::fs::remove_file(path)?;
                sync_parent_dir(parent)?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "quick session quarantine suffix space exhausted",
    ))
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

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_quarantine_avoids_collision_and_keeps_every_file_private() {
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("session.state.json");
        std::fs::write(&source, b"new corrupt state").unwrap();
        let timestamp = "20260711T120000Z";
        let collision = temp
            .path()
            .join(format!("session.state.json.corrupt-{timestamp}"));
        std::fs::write(&collision, b"existing corrupt state").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::set_permissions(&collision, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let quarantined = quarantine_corrupt_file(&source, timestamp).unwrap();

        assert_eq!(
            quarantined.file_name().and_then(|name| name.to_str()),
            Some("session.state.json.corrupt-20260711T120000Z-1")
        );
        assert_eq!(
            std::fs::read(&collision).unwrap(),
            b"existing corrupt state"
        );
        assert_eq!(std::fs::read(&quarantined).unwrap(), b"new corrupt state");
        assert!(!source.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&collision).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&quarantined)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
