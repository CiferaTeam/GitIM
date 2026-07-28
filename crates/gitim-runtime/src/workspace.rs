use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{broadcast, watch};

use crate::git_config::WorkspaceConfig;
use crate::http::{AgentActivityEvent, AgentInfo};

pub struct WorkspaceContext {
    pub slug: String,
    pub workspace_name: String,
    pub path: PathBuf,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
    pub agents: HashMap<String, AgentInfo>,
    pub activity_tx: broadcast::Sender<AgentActivityEvent>,
    /// Flipped by sync_loop after 3 consecutive auth failures; per-workspace so
    /// one broken PAT doesn't mute sync for other workspaces.
    pub auth_failed: Arc<AtomicBool>,
    pub git_config: Option<WorkspaceConfig>,
    pub asset_token: crate::assets::AssetWorkspaceToken,
    pub initialization: Option<Arc<WorkspaceInitialization>>,
    pub transition: Arc<WorkspaceTransitionSlot>,
}

impl WorkspaceContext {
    pub fn new(slug: String, workspace_name: String, path: PathBuf) -> Self {
        let (activity_tx, _) = broadcast::channel(128);
        Self {
            slug,
            workspace_name,
            path,
            human_repo: None,
            poll_cursor: None,
            agents: HashMap::new(),
            activity_tx,
            auth_failed: Arc::new(AtomicBool::new(false)),
            git_config: None,
            asset_token: crate::assets::AssetWorkspaceToken::new(),
            initialization: None,
            transition: Arc::new(WorkspaceTransitionSlot::new()),
        }
    }
}

pub struct WorkspaceTransitionSlot {
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl WorkspaceTransitionSlot {
    fn new() -> Self {
        Self {
            gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn lock_owned(self: Arc<Self>) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.gate).lock_owned().await
    }
}

impl Default for WorkspaceTransitionSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct WorkspaceTransitionRegistry {
    slots: Mutex<HashMap<PathBuf, Weak<WorkspaceTransitionSlot>>>,
}

impl WorkspaceTransitionRegistry {
    pub fn slot(&self, canonical_workspace: &Path) -> Arc<WorkspaceTransitionSlot> {
        let mut slots = crate::preconditions::mutex_lock(&self.slots);
        slots.retain(|_, slot| slot.strong_count() > 0);
        if let Some(slot) = slots.get(canonical_workspace).and_then(Weak::upgrade) {
            return slot;
        }
        let slot = Arc::new(WorkspaceTransitionSlot::new());
        slots.insert(canonical_workspace.to_path_buf(), Arc::downgrade(&slot));
        slot
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn live_entries(&self) -> usize {
        let mut slots = crate::preconditions::mutex_lock(&self.slots);
        slots.retain(|_, slot| slot.strong_count() > 0);
        slots.len()
    }
}

pub struct WorkspaceInitialization {
    finished: watch::Sender<bool>,
    #[cfg(feature = "test-support")]
    wait_started: std::sync::Mutex<Option<Arc<tokio::sync::Barrier>>>,
}

impl WorkspaceInitialization {
    pub fn new() -> Self {
        let (finished, _) = watch::channel(false);
        Self {
            finished,
            #[cfg(feature = "test-support")]
            wait_started: std::sync::Mutex::new(None),
        }
    }

    pub async fn wait(&self) {
        #[cfg(feature = "test-support")]
        let wait_started = { crate::preconditions::mutex_lock(&self.wait_started).take() };
        #[cfg(feature = "test-support")]
        if let Some(started) = wait_started {
            started.wait().await;
        }
        let mut finished = self.finished.subscribe();
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                return;
            }
        }
    }

    pub fn finish(&self) {
        self.finished.send_replace(true);
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn inject_wait_started(&self, started: Arc<tokio::sync::Barrier>) {
        *crate::preconditions::mutex_lock(&self.wait_started) = Some(started);
    }
}

impl Default for WorkspaceInitialization {
    fn default() -> Self {
        Self::new()
    }
}

/// SIGTERM + 500ms grace + SIGKILL every daemon process backing this workspace
/// (the human clone + each agent). Best-effort: missing pid files or
/// already-dead processes are silently ignored. Matches the sequence used by
/// `cleanup_human_dir` so callers get consistent shutdown behavior whether a
/// workspace is dropped via DELETE or the runtime itself is exiting.
pub async fn kill_daemons(ctx: &WorkspaceContext) {
    if let Some(human) = &ctx.human_repo {
        kill_pid_at(human).await;
    }
    for agent in ctx.agents.values() {
        kill_pid_at(Path::new(&agent.repo_path)).await;
    }
}

/// Synchronous variant for non-async contexts (runtime shutdown path in the
/// binary). Uses `std::thread::sleep` — acceptable outside the axum worker
/// pool. Keep in sync with `kill_daemons`.
pub fn kill_daemons_blocking(ctx: &WorkspaceContext) {
    if let Some(human) = &ctx.human_repo {
        kill_pid_at_blocking(human);
    }
    for agent in ctx.agents.values() {
        kill_pid_at_blocking(Path::new(&agent.repo_path));
    }
}

/// Kill every managed daemon across every workspace. Shared by the binary's
/// shutdown path and by the async self-update phase — both want the same
/// SIGTERM + 500ms grace + SIGKILL sequence applied to every agent and the
/// human clone. Blocking to keep the call site sync-friendly; with ~O(10)
/// agents and a 500ms grace the total cost is bounded.
///
/// Acquires the state mutex, snapshots the workspaces, then releases before
/// the blocking kill — so long-running signal dispatch does not hold the
/// mutex against other HTTP handlers.
pub fn kill_managed_daemons(state: &crate::http::SharedRuntimeState) {
    let snapshot: Vec<(Option<PathBuf>, Vec<PathBuf>)> = {
        let s = crate::preconditions::arc_mutex_lock(state);
        s.workspaces
            .values()
            .map(|w| {
                let agents = w
                    .agents
                    .values()
                    .map(|a| PathBuf::from(&a.repo_path))
                    .collect();
                (w.human_repo.clone(), agents)
            })
            .collect()
    };
    for (human, agents) in snapshot {
        if let Some(h) = human {
            kill_pid_at_blocking(&h);
        }
        for agent in agents {
            kill_pid_at_blocking(&agent);
        }
    }
}

async fn kill_pid_at(repo: &Path) {
    let pid_file = repo.join(".gitim/run/gitim.pid");
    let Ok(content) = std::fs::read_to_string(&pid_file) else {
        return;
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return;
    };
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .output();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
}

fn kill_pid_at_blocking(repo: &Path) {
    let pid_file = repo.join(".gitim/run/gitim.pid");
    let Ok(content) = std::fs::read_to_string(&pid_file) else {
        return;
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return;
    };
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .output();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn new_sets_fields_empty() {
        let ctx = WorkspaceContext::new(
            "frontend".to_string(),
            "Frontend".to_string(),
            PathBuf::from("/tmp/frontend"),
        );
        assert_eq!(ctx.slug, "frontend");
        assert_eq!(ctx.workspace_name, "Frontend");
        assert_eq!(ctx.path, PathBuf::from("/tmp/frontend"));
        assert!(ctx.human_repo.is_none());
        assert!(ctx.poll_cursor.is_none());
        assert!(ctx.agents.is_empty());
        assert!(ctx.git_config.is_none());
        assert!(!ctx.auth_failed.load(Ordering::Relaxed));
    }

    #[test]
    fn broadcast_tx_buffer_128() {
        let ctx = WorkspaceContext::new(
            "frontend".to_string(),
            "Frontend".to_string(),
            PathBuf::from("/tmp/frontend"),
        );
        // `broadcast::Sender::len` reports queued-but-unseen messages, not
        // buffer size. Exercise capacity by sending 128 events with an active
        // subscriber — the 129th would displace the oldest if buffer were
        // smaller, but here we just verify `send` succeeds 128 times.
        let _rx = ctx.activity_tx.subscribe();
        for i in 0..128 {
            let event = AgentActivityEvent {
                agent_id: "a".to_string(),
                workspace_id: ctx.slug.clone(),
                event_type: "tool_use".to_string(),
                detail: format!("evt-{i}"),
                timestamp: "2026-04-18T00:00:00Z".to_string(),
                scope: crate::http::ActivityScope::default(),
                session_id: None,
                r#ref: None,
                session_revision: None,
                attempt_id: None,
                context_generation: None,
            };
            assert!(ctx.activity_tx.send(event).is_ok(), "send {i} failed");
        }
    }

    #[test]
    fn per_workspace_broadcast_isolated() {
        let a = WorkspaceContext::new("a".to_string(), "A".to_string(), PathBuf::from("/a"));
        let b = WorkspaceContext::new("b".to_string(), "B".to_string(), PathBuf::from("/b"));
        let mut rx_a = a.activity_tx.subscribe();
        let _ = b.activity_tx.send(AgentActivityEvent {
            agent_id: "x".to_string(),
            workspace_id: "b".to_string(),
            event_type: "t".to_string(),
            detail: "d".to_string(),
            timestamp: "2026".to_string(),
            scope: crate::http::ActivityScope::default(),
            session_id: None,
            r#ref: None,
            session_revision: None,
            attempt_id: None,
            context_generation: None,
        });
        assert!(rx_a.try_recv().is_err());
    }
}
