use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::broadcast;

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
        }
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
    remove_run_files(repo);
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
    remove_run_files(repo);
}

/// Remove a daemon's pid + socket files after killing it. Without this, a fresh
/// runtime's `ensure_daemon` can read the stale pid, treat a not-yet-reaped
/// zombie as "alive" via `kill(pid, 0)`, skip the spawn, and then block in
/// `wait_for_socket` on the dead socket until timeout. Best-effort — a missing
/// file is fine.
fn remove_run_files(repo: &Path) {
    let run_dir = repo.join(".gitim/run");
    let _ = std::fs::remove_file(run_dir.join("gitim.pid"));
    let _ = std::fs::remove_file(run_dir.join("gitim.sock"));
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
        });
        assert!(rx_a.try_recv().is_err());
    }

    #[test]
    fn kill_pid_at_blocking_removes_stale_run_files() {
        // After killing a daemon we must delete its pid + sock files. Otherwise
        // a fresh runtime's `ensure_daemon` reads the stale pid, finds it
        // "alive" (kill(pid,0) on a not-yet-reaped zombie), skips the spawn, and
        // then blocks 15s in wait_for_socket on the dead socket. Cleaning the
        // files forces a clean re-spawn.
        let tmp = tempfile::TempDir::new().unwrap();
        let run_dir = tmp.path().join(".gitim/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        // A pid that cannot be alive: above macOS pid_max (99998) and the common
        // Linux default (32768). kill(2) on it is a harmless ESRCH.
        std::fs::write(run_dir.join("gitim.pid"), "999999").unwrap();
        std::fs::write(run_dir.join("gitim.sock"), b"").unwrap();

        kill_pid_at_blocking(tmp.path());

        assert!(
            !run_dir.join("gitim.pid").exists(),
            "stale pid file must be removed so ensure_daemon re-spawns"
        );
        assert!(
            !run_dir.join("gitim.sock").exists(),
            "stale socket must be removed so wait_for_socket doesn't hang on it"
        );
    }
}
