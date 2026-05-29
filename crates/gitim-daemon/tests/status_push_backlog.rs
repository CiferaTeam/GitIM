#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `Request::Status` surfaces push-backlog visibility: the count of
//! locally-committed-but-unpushed messages and whether the sync-loop auth
//! circuit has tripped. These let `gitim status` (and future UIs) tell when the
//! daemon is committing locally but failing to push — the failure mode that is
//! otherwise silent now that send acks on local commit, not on push.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::{AppState, PendingMessage};
use tempfile::TempDir;
use tokio::sync::broadcast;

fn make_state(repo_root: &Path) -> Arc<AppState> {
    let (event_tx, _) = broadcast::channel(16);
    Arc::new(AppState::new(
        repo_root.to_path_buf(),
        Config::default(),
        event_tx,
        None,
    ))
}

#[tokio::test]
async fn status_reports_pending_push_backlog() {
    let tmp = TempDir::new().unwrap();
    let state = make_state(tmp.path());
    // Three locally-committed messages awaiting a push that hasn't landed.
    {
        let mut pending = state.pending_push.write().unwrap();
        pending.push(PendingMessage {
            channel: "general".to_string(),
            line_number: 1,
        });
        pending.push(PendingMessage {
            channel: "general".to_string(),
            line_number: 2,
        });
        pending.push(PendingMessage {
            channel: "dm:a--b".to_string(),
            line_number: 5,
        });
    }

    let resp = handle_request(Request::Status, state).await;
    assert!(resp.ok, "status should succeed, got {:?}", resp);
    let data = resp.data.expect("status carries data");
    assert_eq!(
        data.get("pending_push_count").and_then(|v| v.as_u64()),
        Some(3),
        "pending_push_count must reflect the unpushed queue length"
    );
}

#[tokio::test]
async fn status_reports_auth_circuit_open() {
    let tmp = TempDir::new().unwrap();
    let state = make_state(tmp.path());
    state.auth_failed.store(true, Ordering::SeqCst);

    let resp = handle_request(Request::Status, state).await;
    let data = resp.data.expect("status carries data");
    assert_eq!(
        data.get("auth_circuit_open").and_then(|v| v.as_bool()),
        Some(true),
        "auth_circuit_open must mirror the tripped auth_failed flag"
    );
}

#[tokio::test]
async fn status_clean_daemon_reports_zero_backlog() {
    let tmp = TempDir::new().unwrap();
    let state = make_state(tmp.path());

    let resp = handle_request(Request::Status, state).await;
    let data = resp.data.expect("status carries data");
    assert_eq!(
        data.get("pending_push_count").and_then(|v| v.as_u64()),
        Some(0),
        "a clean daemon has no backlog"
    );
    assert_eq!(
        data.get("auth_circuit_open").and_then(|v| v.as_bool()),
        Some(false),
        "a clean daemon's auth circuit is closed"
    );
}
