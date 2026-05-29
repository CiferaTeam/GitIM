//! Background task that snapshots per-agent `is_working` every
//! `SAMPLING_INTERVAL` and persists per-agent saturation buckets to disk.
//!
//! Design constraint: `RuntimeState` uses `std::sync::Mutex`, so the
//! sampler MUST lock only long enough to clone the Arc + handler strings,
//! then drop the lock before doing any IO. See
//! `docs/plans/saturation-sampler/00-requirements.md`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::http::SharedRuntimeState;
use crate::saturation_log::AgentSaturationLog;

/// Production sampler interval. 5 minutes balances signal density
/// (12 samples per hour means by_hour ratio has 8.3% resolution) against
/// IO frequency. Override via `SaturationSampler::with_interval` in tests.
pub const SAMPLING_INTERVAL: Duration = Duration::from_secs(300);

/// One agent's address + working flag captured under the RuntimeState lock.
/// We keep `workspace_root` (PathBuf) and `handler` (String) by value so the
/// snapshot stays valid after the lock drops.
#[derive(Debug, Clone)]
pub struct AgentSnapshot {
    pub workspace_root: PathBuf,
    pub handler: String,
    pub working: bool,
}

/// Capture the working state of every known agent across every workspace.
/// Returns an empty vec when no workspaces/agents are registered.
///
/// Lock policy: holds the std::sync::Mutex only for the duration of this
/// function. No IO, no await — purely clones small data out. N=20 agents
/// finish in microseconds.
pub fn take_snapshot(state: &SharedRuntimeState) -> Vec<AgentSnapshot> {
    let s = match state.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut out = Vec::new();
    for ctx in s.workspaces.values() {
        for info in ctx.agents.values() {
            out.push(AgentSnapshot {
                workspace_root: ctx.path.clone(),
                handler: info.handler.clone(),
                working: info.is_working.load(Ordering::Relaxed),
            });
        }
    }
    out
}

/// Apply one tick's snapshot to disk. Each entry loads-accumulate-saves
/// independently so one agent's IO failure doesn't poison the rest.
///
/// `now_iso` / `today` / `now_hour` come from a single `Utc::now()` instant
/// so a tick that straddles midnight stays internally consistent.
///
/// Failure counter: every save error bumps
/// `RuntimeState.saturation_save_failures` (best-effort, never blocks).
pub fn tick_once(
    snapshot: &[AgentSnapshot],
    today: &str,
    now_hour: &str,
    now_iso: &str,
    state: &SharedRuntimeState,
) {
    for entry in snapshot {
        let mut log = AgentSaturationLog::load_or_default(&entry.workspace_root, &entry.handler);
        log.accumulate(today, now_hour, entry.working, now_iso);
        if let Err(e) = log.save(&entry.workspace_root, today) {
            tracing::warn!(
                handler = %entry.handler,
                error = %e,
                "failed to save saturation log"
            );
            if let Ok(s) = state.lock() {
                s.saturation_save_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Background ticker. `interval` defaults to `SAMPLING_INTERVAL` for
/// production; tests use `with_interval(Duration::from_millis(...))` to
/// drive multiple ticks in under a second.
///
/// Spawn via `SaturationSampler::spawn(state)` from `run_shell`. Returns an
/// `AbortHandle` so the runtime can stop it during a graceful shutdown
/// (currently unused — runtime process exit is the only stop path).
pub struct SaturationSampler {
    interval: Duration,
    state: SharedRuntimeState,
    shutdown: Arc<AtomicBool>,
}

impl SaturationSampler {
    pub fn new(state: SharedRuntimeState) -> Self {
        Self::with_interval(state, SAMPLING_INTERVAL)
    }

    pub fn with_interval(state: SharedRuntimeState, interval: Duration) -> Self {
        Self {
            interval,
            state,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn the sampler loop on the current tokio runtime. The returned
    /// `Arc<AtomicBool>` flips to true and ends the loop after the
    /// currently-running tick finishes.
    pub fn spawn(self) -> Arc<AtomicBool> {
        let shutdown = self.shutdown.clone();
        let shutdown_for_task = shutdown.clone();
        let interval = self.interval;
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Tokio interval fires immediately on first .tick(). Skip the
            // first tick so the first sample happens after one full
            // interval, giving recovery time to register agents.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if shutdown_for_task.load(Ordering::Relaxed) {
                    break;
                }
                let snapshot = take_snapshot(&state);
                if snapshot.is_empty() {
                    continue;
                }
                let now = chrono::Utc::now();
                let today = now.format("%Y-%m-%d").to_string();
                let now_hour = now.format("%Y-%m-%dT%H").to_string();
                let now_iso = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
                tick_once(&snapshot, &today, &now_hour, &now_iso, &state);
            }
        });
        shutdown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{AgentInfo, RuntimeState};
    use crate::workspace::WorkspaceContext;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    fn make_agent(handler: &str, working: bool) -> AgentInfo {
        AgentInfo {
            id: handler.to_string(),
            handler: handler.to_string(),
            display_name: handler.to_string(),
            status: "running".into(),
            last_activity: None,
            messages_processed: 0,
            repo_path: String::new(),
            provider: None,
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: HashMap::new(),
            error_message: None,
            session_usage: None,
            llm_provider: None,
            llm_model: None,
            usage_summary: None,
            saturation_summary: None,
            is_working: Arc::new(AtomicBool::new(working)),
            loop_handle: None,
        }
    }

    fn make_state(workspace_root: PathBuf, agents: Vec<AgentInfo>) -> SharedRuntimeState {
        let (tx, _) = broadcast::channel(16);
        let mut agent_map = HashMap::new();
        for a in agents {
            agent_map.insert(a.id.clone(), a);
        }
        let ctx = WorkspaceContext {
            slug: "test".into(),
            workspace_name: "test".into(),
            path: workspace_root,
            human_repo: None,
            poll_cursor: None,
            agents: agent_map,
            activity_tx: tx,
            auth_failed: Arc::new(AtomicBool::new(false)),
            git_config: None,
        };
        let mut rs = RuntimeState::default();
        rs.workspaces.insert("test".into(), ctx);
        Arc::new(Mutex::new(rs))
    }

    #[test]
    fn snapshot_empty_state_returns_empty_vec() {
        let rs = Arc::new(Mutex::new(RuntimeState::default()));
        assert!(take_snapshot(&rs).is_empty());
    }

    #[test]
    fn snapshot_captures_working_flag_per_agent() {
        let dir = TempDir::new().unwrap();
        let state = make_state(
            dir.path().to_path_buf(),
            vec![make_agent("alice", true), make_agent("bob", false)],
        );
        let mut snap = take_snapshot(&state);
        snap.sort_by(|a, b| a.handler.cmp(&b.handler));
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].handler, "alice");
        assert!(snap[0].working);
        assert_eq!(snap[1].handler, "bob");
        assert!(!snap[1].working);
    }

    #[test]
    fn tick_once_writes_one_file_per_agent() {
        let dir = TempDir::new().unwrap();
        let state = make_state(
            dir.path().to_path_buf(),
            vec![make_agent("alice", true), make_agent("bob", false)],
        );
        let snap = take_snapshot(&state);
        tick_once(
            &snap,
            "2026-05-21",
            "2026-05-21T12",
            "2026-05-21T12:00:00Z",
            &state,
        );
        let a = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert_eq!(a.totals.total_samples, 1);
        assert_eq!(a.totals.working_samples, 1);
        let b = AgentSaturationLog::load_or_default(dir.path(), "bob");
        assert_eq!(b.totals.total_samples, 1);
        assert_eq!(b.totals.working_samples, 0);
    }

    #[test]
    fn tick_once_accumulates_across_calls() {
        let dir = TempDir::new().unwrap();
        let state = make_state(dir.path().to_path_buf(), vec![make_agent("alice", true)]);
        let snap = take_snapshot(&state);
        for h in 8..=11 {
            tick_once(
                &snap,
                "2026-05-21",
                &format!("2026-05-21T{h:02}"),
                &format!("2026-05-21T{h:02}:00:00Z"),
                &state,
            );
        }
        let a = AgentSaturationLog::load_or_default(dir.path(), "alice");
        assert_eq!(a.totals.total_samples, 4);
        assert_eq!(a.by_hour.len(), 4);
    }
}
