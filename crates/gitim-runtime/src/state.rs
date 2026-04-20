use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    ProviderReported,
    RuntimeEstimated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsageSnapshot {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    pub used_percent: f64,
    pub source: UsageSource,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<SessionUsageSnapshot>,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub estimated_tokens: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub usage_notice_pending: bool,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
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

    /// Clear all fields tied to the current provider session. Called on
    /// `[[RESET]]` detection and on session failure.
    pub fn clear_session(&mut self) {
        self.session_token = None;
        self.session_usage = None;
        self.estimated_tokens = 0;
        self.usage_notice_pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn state_roundtrips_new_fields() {
        let dir = TempDir::new().expect("tempdir");
        let gitim_dir = dir.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).expect("mkdir");

        let original = AgentState {
            cursor: Some("c1".into()),
            session_token: Some("sess-abc".into()),
            session_usage: Some(SessionUsageSnapshot {
                session_id: "sess-abc".into(),
                input_tokens: Some(128_000),
                output_tokens: Some(512),
                max_tokens: Some(200_000),
                used_percent: 64.0,
                source: UsageSource::ProviderReported,
                updated_at: "2026-04-20T12:00:00Z".into(),
            }),
            estimated_tokens: 125_400,
            usage_notice_pending: false,
        };

        original.save(dir.path()).expect("save");
        let loaded = AgentState::load(dir.path()).expect("load");
        assert_eq!(loaded.session_token, original.session_token);
        let snap = loaded.session_usage.expect("snapshot present");
        assert_eq!(snap.session_id, "sess-abc");
        assert_eq!(snap.used_percent, 64.0);
        assert!(matches!(snap.source, UsageSource::ProviderReported));
        assert_eq!(loaded.estimated_tokens, 125_400);
    }

    #[test]
    fn legacy_state_without_new_fields_loads() {
        let dir = TempDir::new().expect("tempdir");
        let gitim_dir = dir.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).expect("mkdir");
        let legacy = r#"{"cursor":"old","session_token":"sess-old"}"#;
        std::fs::write(gitim_dir.join("agent-state.json"), legacy).expect("write");

        let state = AgentState::load(dir.path()).expect("load");
        assert_eq!(state.cursor.as_deref(), Some("old"));
        assert!(state.session_usage.is_none());
        assert_eq!(state.estimated_tokens, 0);
        assert!(!state.usage_notice_pending);
    }
}
