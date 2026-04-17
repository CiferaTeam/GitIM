//! Tests for `recover_agents_from_workspace` — the per-workspace scan that
//! `recover_from_config` delegates to on startup.
//!
//! Only error branches are covered here. The happy path (valid provider →
//! `ensure_daemon` + `start_agent_loop`) requires a real daemon binary and
//! is already covered by the poller / provision integration tests, plus
//! end-to-end manual runs.

use std::path::Path;
use std::sync::{Arc, Mutex};

use gitim_runtime::http::{recover_agents_from_workspace, RuntimeState, SharedRuntimeState};
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

fn fresh_state() -> SharedRuntimeState {
    Arc::new(Mutex::new(RuntimeState::default()))
}

#[tokio::test]
async fn test_recover_missing_provider_marks_error() {
    let tmp = TempDir::new().unwrap();
    // No "provider" key at all.
    write_agent(
        tmp.path(),
        "no-prov",
        serde_json::json!({
            "handler": "no-prov",
            "display_name": "No Provider",
        }),
    );

    let state = fresh_state();
    recover_agents_from_workspace(state.clone(), tmp.path()).await;

    let s = state.lock().unwrap();
    let info = s
        .agents
        .get("no-prov")
        .expect("agent should be registered even when provider is missing");
    assert_eq!(info.status, "error");
    let msg = info.error_message.as_deref().unwrap_or_default();
    assert!(msg.contains("Missing"), "error_message should mention Missing: {msg}");
    assert!(msg.contains("provider"), "error_message should mention provider: {msg}");
    assert!(info.loop_handle.is_none(), "loop_handle must be None on error");
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

    let state = fresh_state();
    recover_agents_from_workspace(state.clone(), tmp.path()).await;

    let s = state.lock().unwrap();
    let info = s
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
    // provider field is preserved so the UI can show what was actually in the file
    assert_eq!(info.provider.as_deref(), Some("gemini"));
    assert!(info.loop_handle.is_none());
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
