//! Tests for `recover_agents_for_workspace` — the per-workspace scan that
//! `recover_from_config` delegates to on startup.
//!
//! Only error branches are covered here. The happy path (valid provider →
//! `ensure_daemon` + `start_agent_loop`) requires a real daemon binary and
//! is already covered by the poller / provision integration tests, plus
//! end-to-end manual runs.

use std::path::Path;
use std::sync::{Arc, Mutex};

use gitim_runtime::http::{recover_agents_for_workspace, RuntimeState, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;
use tempfile::TempDir;

/// Write a minimal me.json into `<workspace>/<handler>/.gitim/me.json`.
fn write_agent(workspace: &Path, handler: &str, me: serde_json::Value) {
    let dir = workspace.join(handler);
    std::fs::create_dir_all(dir.join(".gitim")).unwrap();
    std::fs::write(
        dir.join(".gitim/me.json"),
        serde_json::to_string_pretty(&me).unwrap(),
    )
    .unwrap();
}

fn fresh_state_with_ws(slug: &str, path: &Path) -> SharedRuntimeState {
    let state = Arc::new(Mutex::new(RuntimeState::default()));
    {
        let mut s = state.lock().unwrap();
        s.workspaces.insert(
            slug.to_string(),
            WorkspaceContext::new(slug.to_string(), slug.to_string(), path.to_path_buf()),
        );
    }
    state
}

#[tokio::test]
async fn test_recover_missing_provider_marks_error() {
    let tmp = TempDir::new().unwrap();
    write_agent(
        tmp.path(),
        "no-prov",
        serde_json::json!({
            "handler": "no-prov",
            "display_name": "No Provider",
        }),
    );

    let state = fresh_state_with_ws("test-ws", tmp.path());
    recover_agents_for_workspace(state.clone(), "test-ws", tmp.path()).await;

    let s = state.lock().unwrap();
    let ctx = s.workspaces.get("test-ws").expect("ws present");
    let info = ctx
        .agents
        .get("no-prov")
        .expect("agent should be registered even when provider is missing");
    assert_eq!(info.status, "error");
    let msg = info.error_message.as_deref().unwrap_or_default();
    assert!(
        msg.contains("Missing"),
        "error_message should mention Missing: {msg}"
    );
    assert!(
        msg.contains("provider"),
        "error_message should mention provider: {msg}"
    );
    assert!(
        info.loop_handle.is_none(),
        "loop_handle must be None on error"
    );
}

#[tokio::test]
async fn test_recover_unknown_provider_marks_error() {
    let tmp = TempDir::new().unwrap();
    write_agent(
        tmp.path(),
        "gem-prov",
        serde_json::json!({
            "handler": "gem-prov",
            "display_name": "Gemini Provider",
            "provider": "gemini",
        }),
    );

    let state = fresh_state_with_ws("test-ws", tmp.path());
    recover_agents_for_workspace(state.clone(), "test-ws", tmp.path()).await;

    let s = state.lock().unwrap();
    let ctx = s.workspaces.get("test-ws").expect("ws present");
    let info = ctx
        .agents
        .get("gem-prov")
        .expect("agent should be registered even with unknown provider");
    assert_eq!(info.status, "error");
    let msg = info.error_message.as_deref().unwrap_or_default();
    assert!(
        msg.contains("Unsupported"),
        "error_message should mention Unsupported: {msg}"
    );
    assert!(
        msg.contains("gemini"),
        "error_message should echo the bad provider value: {msg}"
    );
    assert_eq!(info.provider.as_deref(), Some("gemini"));
    assert!(info.loop_handle.is_none());
}

#[tokio::test]
async fn test_recover_missing_provider_broadcasts_error_event() {
    let tmp = TempDir::new().unwrap();
    write_agent(
        tmp.path(),
        "no-prov",
        serde_json::json!({
            "handler": "no-prov",
            "display_name": "No Provider",
        }),
    );

    let state = fresh_state_with_ws("test-ws", tmp.path());
    let mut rx = {
        let s = state.lock().unwrap();
        s.workspaces
            .get("test-ws")
            .expect("ws present")
            .activity_tx
            .subscribe()
    };

    recover_agents_for_workspace(state.clone(), "test-ws", tmp.path()).await;

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("activity event should arrive within 1s")
        .expect("broadcast channel should deliver the event");

    assert_eq!(event.event_type, "error", "event_type should be error");
    assert_eq!(event.agent_id, "no-prov", "agent_id should match handler");
    assert_eq!(event.workspace_id, "test-ws", "workspace_id should be set");
    assert!(
        event.detail.contains("Missing"),
        "detail should mention Missing: {}",
        event.detail
    );
    assert!(!event.timestamp.is_empty(), "timestamp should be set");
}

// Note: a `test_recover_valid_provider_starts_normally` case would need to
// drive ensure_daemon + start_agent_loop, both of which spawn real
// subprocesses and real IPC sockets. Set-up cost outweighs value here
// because:
//   1. `agents/add` already exercises the same provider=claude / provider=codex
//      path through `start_agent_loop` and is covered by end-to-end runs.
//   2. The classification logic we care about in this task (three-way split
//      on the provider field) is fully observable on the error branches.
// If a regression sneaks into the happy-path branch we will see it in the
// poller/provision integration tests (which also go through ensure_daemon).
