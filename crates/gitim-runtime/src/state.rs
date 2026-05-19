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
    /// Set after the runtime has injected the pressure-relief preamble for the
    /// current provider session. This is distinct from `usage_notice_pending`:
    /// providers with estimator-only pressure can lose their visible
    /// `session_usage` snapshot once the estimate overflows the display budget,
    /// but the reset prompt must still remain one-shot instead of re-arming on
    /// every turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub usage_notice_sent: bool,

    /// Set by the `[[RESET]]` branch after `clear_session()`. The next
    /// `run_once` consumes it: if the poll cycle has no external changes,
    /// the runtime still kicks one cold-start turn with a synthetic
    /// "post-reset continuation" preamble so the agent can read its memory
    /// file and naturally pick up unfinished work — instead of sleeping
    /// forever until an external mention arrives.
    ///
    /// Cleared by `clear_session()` like the rest of the session-scoped
    /// state: PATCH model / provider failure should not leave a stray
    /// continuation signal armed. The reset branch's "clear-then-set"
    /// sequence is the only place this flag is supposed to outlive a
    /// `clear_session()` call.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub post_reset_pending: bool,

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

    /// Clear all fields tied to the current provider session, plus any
    /// cross-cycle pressure/continuation signals (`usage_notice_pending`,
    /// `usage_notice_sent`, `post_reset_pending`). Called on `[[RESET]]`
    /// detection, on session failure, and from PATCH-agent paths that
    /// effectively start fresh (model / system_prompt change).
    ///
    /// The reset branch is the one place where a continuation signal is
    /// supposed to outlive a `clear_session()`: it re-arms
    /// `post_reset_pending = true` *after* this call so the next cycle
    /// can self-wake.
    pub fn clear_session(&mut self) {
        self.session_token = None;
        self.session_usage = None;
        self.estimated_tokens = 0;
        self.usage_notice_pending = false;
        self.usage_notice_sent = false;
        self.post_reset_pending = false;
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
            usage_notice_sent: false,
            post_reset_pending: false,
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
            usage_notice_sent: false,
            post_reset_pending: false,
            last_session_usage: Some(LastSessionUsage {
                session_id: "sess-cum".into(),
                usage: ProviderUsage {
                    input_tokens: Some(40_000),
                    output_tokens: Some(2_000),
                    used_percent: Some(0.42),
                    cache_read_tokens: Some(150_000),
                    cache_creation_tokens: Some(900),
                    context_tokens: None,
                    context_window_tokens: None,
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
            usage_notice_sent: false,
            post_reset_pending: false,
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
    fn post_reset_pending_roundtrips() {
        let dir = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".gitim")).expect("mkdir");
        let state = AgentState {
            post_reset_pending: true,
            ..AgentState::default()
        };
        state.save(dir.path()).expect("save");
        let loaded = AgentState::load(dir.path()).expect("load");
        assert!(loaded.post_reset_pending);
    }

    #[test]
    fn post_reset_pending_skips_when_false_serialization() {
        // Default-false should not leak into the on-disk JSON — keeps the
        // file clean for the overwhelming majority of agents that never
        // RESET.
        let state = AgentState::default();
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(
            !json.contains("post_reset_pending"),
            "default-false post_reset_pending must skip serialize, got: {json}"
        );
    }

    #[test]
    fn clear_session_also_clears_post_reset_pending() {
        // Defensive: a stray post_reset_pending should not survive a
        // PATCH-model / provider-failure path. The RESET branch re-arms it
        // *after* clear_session — that's the only place it's supposed to
        // outlive the wipe.
        let mut state = AgentState {
            cursor: Some("c".into()),
            session_token: Some("s".into()),
            session_usage: None,
            estimated_tokens: 100,
            usage_notice_pending: true,
            usage_notice_sent: true,
            post_reset_pending: true,
            last_session_usage: None,
        };
        state.clear_session();
        assert!(state.session_token.is_none());
        assert!(!state.usage_notice_pending);
        assert!(
            !state.usage_notice_sent,
            "clear_session must allow a fresh session to receive its own notice"
        );
        assert!(
            !state.post_reset_pending,
            "clear_session must wipe post_reset_pending; reset branch re-arms after"
        );
    }

    #[test]
    fn reset_branch_sequence_leaves_post_reset_armed() {
        // Models the agent_loop reset branch:
        //   clear_session()         // wipe everything
        //   post_reset_pending=true // re-arm continuation
        //   save / load             // survives the cycle boundary
        let dir = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".gitim")).expect("mkdir");
        let mut state = AgentState {
            cursor: Some("c".into()),
            session_token: Some("s".into()),
            session_usage: None,
            estimated_tokens: 100,
            usage_notice_pending: true,
            usage_notice_sent: true,
            post_reset_pending: false,
            last_session_usage: None,
        };
        state.clear_session();
        state.post_reset_pending = true;
        state.save(dir.path()).expect("save");

        let loaded = AgentState::load(dir.path()).expect("load");
        assert!(loaded.session_token.is_none(), "session wiped");
        assert!(
            loaded.post_reset_pending,
            "continuation re-armed for next cycle"
        );
        assert_eq!(
            loaded.cursor.as_deref(),
            Some("c"),
            "poller cursor preserved"
        );
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
        assert!(
            !state.usage_notice_sent,
            "field added 2026-05-19 — legacy state must default to false"
        );
        assert!(
            !state.post_reset_pending,
            "field added 2026-05-12 — legacy state must default to false"
        );
    }
}
