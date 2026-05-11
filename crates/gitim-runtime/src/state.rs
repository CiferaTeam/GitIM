use std::path::{Path, PathBuf};

use gitim_agent_provider::ProviderUsage;
use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

/// Last seen `ProviderUsage` for a cumulative provider, paired with the
/// session id it belongs to. The runtime computes per-turn deltas by
/// subtracting this baseline from the next turn's ProviderUsage.
///
/// Cleared on `[[RESET]]` and whenever the session id changes, so the
/// baseline never bleeds across sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastSessionUsage {
    pub session_id: String,
    pub usage: ProviderUsage,
}

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

    /// Baseline used to convert cumulative provider usage to per-turn deltas
    /// in the token-statistics layer. Lives next to `session_token` because
    /// it shares the session lifecycle, not because it serves the session
    /// machinery itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_usage: Option<LastSessionUsage>,
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
        self.last_session_usage = None;
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
            last_session_usage: None,
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
    fn snapshot_serializes_without_detail_fields_when_absent() {
        let snap = SessionUsageSnapshot {
            session_id: "sid".into(),
            input_tokens: None,
            output_tokens: None,
            max_tokens: None,
            used_percent: 47.5,
            source: UsageSource::ProviderReported,
            updated_at: "2026-04-20T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&snap).expect("serialize");
        assert!(json.contains("\"session_id\":\"sid\""));
        assert!(json.contains("\"used_percent\":47.5"));
        assert!(json.contains("\"source\":\"provider_reported\""));
        assert!(!json.contains("input_tokens")); // skipped when None
    }

    #[test]
    fn last_session_usage_roundtrips() {
        let dir = TempDir::new().expect("tempdir");
        let gitim_dir = dir.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).expect("mkdir");

        let original = AgentState {
            cursor: None,
            session_token: Some("sess-cum".into()),
            session_usage: None,
            estimated_tokens: 0,
            usage_notice_pending: false,
            last_session_usage: Some(LastSessionUsage {
                session_id: "sess-cum".into(),
                usage: ProviderUsage {
                    input_tokens: Some(40_000),
                    output_tokens: Some(2_000),
                    used_percent: Some(0.42),
                    cache_read_tokens: Some(150_000),
                    cache_creation_tokens: Some(900),
                },
            }),
        };

        original.save(dir.path()).expect("save");
        let loaded = AgentState::load(dir.path()).expect("load");
        let baseline = loaded.last_session_usage.expect("baseline present");
        assert_eq!(baseline.session_id, "sess-cum");
        assert_eq!(baseline.usage.input_tokens, Some(40_000));
        assert_eq!(baseline.usage.cache_read_tokens, Some(150_000));
    }

    #[test]
    fn clear_session_drops_last_session_usage() {
        let mut state = AgentState {
            cursor: None,
            session_token: Some("sess-cum".into()),
            session_usage: None,
            estimated_tokens: 0,
            usage_notice_pending: false,
            last_session_usage: Some(LastSessionUsage {
                session_id: "sess-cum".into(),
                usage: ProviderUsage {
                    input_tokens: Some(1_000),
                    ..Default::default()
                },
            }),
        };
        state.clear_session();
        assert!(state.last_session_usage.is_none());
        assert!(state.session_token.is_none());
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
