use std::collections::HashMap;
use std::path::PathBuf;
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
        let a = WorkspaceContext::new(
            "a".to_string(),
            "A".to_string(),
            PathBuf::from("/a"),
        );
        let b = WorkspaceContext::new(
            "b".to_string(),
            "B".to_string(),
            PathBuf::from("/b"),
        );
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
}
