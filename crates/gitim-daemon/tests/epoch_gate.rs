#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Tests for `AppState`'s epoch-status field, refresh on boot, and refresh
//! after each sync cycle (Subtask B of Phase A "Snapshot Pack" rollout).
//!
//! The fixture YAML matches the design at
//! `docs/plans/2026-05-06-git-history-snapshot-pack.md` ("元数据文件" section)
//! — the wire contract Subtask A's parser implements.
//!
//! Boot- and sync-path wiring tests deliberately bypass the full daemon
//! integration harness: building a redirected workspace + spawning a real
//! daemon process just to read `gitim.epoch.yaml` would multiply test cost
//! out of proportion to what's being verified. Instead these tests exercise
//! the refresh contract at the `AppState` API surface and verify the wiring
//! sites by inspection (see the report). The full sync-loop e2e is left as
//! a Phase A polish item once write gates land in Subtask C.

use std::path::Path;
use std::sync::Arc;

use gitim_core::epoch::EpochStatus;
use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;
use tempfile::TempDir;
use tokio::sync::broadcast;

const REDIRECTED_YAML: &str = r#"
schema_version: 1
status: redirected
epoch: 1
branch: main
redirect:
  target_epoch: 2
  target_branch: main-epoch-2
  target_commit: 1122334455667788990011223344556677889900
  snapshot_of: aabbccddeeff00112233445566778899aabbccdd
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;

const ACTIVE_YAML: &str = r#"
schema_version: 1
status: active
epoch: 2
branch: main-epoch-2
snapshot:
  source_branch: main
  source_commit: aabbccddeeff00112233445566778899aabbccdd
  commit: 1122334455667788990011223344556677889900
  created_at: "2026-05-06T00:00:00Z"
archive:
  tag: archive/epoch-1/aabbccdd
  bundle_sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
"#;

fn write_epoch_file(repo_root: &Path, yaml: &str) {
    std::fs::write(repo_root.join("gitim.epoch.yaml"), yaml).expect("write epoch yaml");
}

fn make_state(repo_root: &Path) -> Arc<AppState> {
    let (event_tx, _) = broadcast::channel(16);
    Arc::new(AppState::new(
        repo_root.to_path_buf(),
        Config::default(),
        event_tx,
        None,
    ))
}

#[test]
fn refresh_reads_redirected() {
    let tmp = TempDir::new().unwrap();
    write_epoch_file(tmp.path(), REDIRECTED_YAML);

    let state = make_state(tmp.path());
    state
        .refresh_epoch_status()
        .expect("redirected file must parse");

    assert!(
        state.is_redirected(),
        "redirected file should latch the flag"
    );
    let snap = state
        .epoch_status_snapshot()
        .expect("snapshot must be Some after refresh");
    assert_eq!(snap.status, EpochStatus::Redirected);
    let redir = snap.redirect.as_ref().expect("redirected carries redirect");
    assert_eq!(redir.target_branch, "main-epoch-2");
}

#[test]
fn refresh_no_file_is_not_redirected() {
    let tmp = TempDir::new().unwrap();
    // No gitim.epoch.yaml at all — legacy repo shape, valid Active.
    let state = make_state(tmp.path());
    state
        .refresh_epoch_status()
        .expect("missing file is not an error");

    assert!(!state.is_redirected());
    assert!(state.epoch_status_snapshot().is_none());
}

#[test]
fn refresh_active_is_not_redirected() {
    let tmp = TempDir::new().unwrap();
    write_epoch_file(tmp.path(), ACTIVE_YAML);

    let state = make_state(tmp.path());
    state
        .refresh_epoch_status()
        .expect("active file must parse");

    assert!(
        !state.is_redirected(),
        "active status must not latch the redirect flag"
    );
    let snap = state
        .epoch_status_snapshot()
        .expect("snapshot is Some for an Active file");
    assert_eq!(snap.status, EpochStatus::Active);
    assert_eq!(snap.epoch, 2);
    assert_eq!(snap.branch, "main-epoch-2");
}

/// Boot-path proof: a freshly-built `AppState` starts with no snapshot, then
/// a single `refresh_epoch_status` call — which is what `main.rs` performs
/// right after constructing the state Arc — picks up a redirected file that
/// was on disk before the daemon started.
///
/// Spawning a real daemon process just to assert this would balloon scope:
/// every other test in this file would need a git repo + sock + cleanup. The
/// boot wiring itself is verifiable by reading `main.rs`; this test just
/// guarantees the API the wiring depends on behaves as documented.
#[test]
fn boot_path_refresh_picks_up_redirected_file() {
    let tmp = TempDir::new().unwrap();
    write_epoch_file(tmp.path(), REDIRECTED_YAML);

    let state = make_state(tmp.path());
    // Pre-refresh: state is the "just built, not yet booted" shape.
    assert!(state.epoch_status_snapshot().is_none());
    assert!(!state.is_redirected());

    // This is the call `main.rs` makes once at startup.
    state.refresh_epoch_status().expect("boot refresh");

    assert!(state.is_redirected());
}

/// Sync-loop proof: the `on_synced` closure body's refresh contract is that
/// calling `refresh_epoch_status` on the captured `Arc<AppState>` reflects a
/// file that appeared on disk between sync cycles. We construct the state
/// with no epoch file present, then write one, then call the same method
/// the closure invokes — and assert the state observes the change.
///
/// A full sync-loop e2e would require a remote git repo + real fetch cycle,
/// which is a Phase A polish item.
#[test]
fn sync_callback_refresh_observes_new_redirected_file() {
    let tmp = TempDir::new().unwrap();
    let state = make_state(tmp.path());

    // First sync cycle: no epoch file → state is "Active by default".
    state.refresh_epoch_status().expect("initial refresh");
    assert!(!state.is_redirected());

    // Operator publishes the redirect between sync cycles.
    write_epoch_file(tmp.path(), REDIRECTED_YAML);

    // The sync_loop's on_synced closure body calls this on the captured Arc.
    state.refresh_epoch_status().expect("post-sync refresh");
    assert!(state.is_redirected());
}

// --------------------------------------------------------------------------
// Status response exposes epoch_status_snapshot.
//
// `Request::Status` returns `Response::success(data)` where `data` is the
// serialized `StatusResponse` payload. The handler appends an `epoch` field
// to that JSON object carrying the parsed `gitim.epoch.yaml` content (the
// same `EpochFile` shape `AppState::epoch_status_snapshot` returns).
//
// - Active YAML → `data.epoch.status == "active"`, snapshot populated, no
//   redirect.
// - Redirected YAML → `data.epoch.status == "redirected"`, redirect
//   populated, no snapshot.
// - No YAML at all → field omitted (backward-compat: old clients see the
//   same object shape they always did).
// --------------------------------------------------------------------------

#[tokio::test]
async fn status_exposes_active_epoch() {
    let tmp = TempDir::new().unwrap();
    write_epoch_file(tmp.path(), ACTIVE_YAML);
    let state = make_state(tmp.path());
    state.refresh_epoch_status().expect("refresh active file");

    let resp = handle_request(Request::Status, state.clone()).await;
    assert!(resp.ok, "status request should succeed, got {:?}", resp);

    let data = resp.data.expect("status response carries data");
    let epoch = data
        .get("epoch")
        .expect("active YAML on disk → status.data.epoch must be present")
        .clone();

    assert_eq!(
        epoch.get("status").and_then(|v| v.as_str()),
        Some("active"),
        "epoch.status should serialize to 'active', got {:?}",
        epoch.get("status")
    );
    assert_eq!(
        epoch.get("epoch").and_then(|v| v.as_u64()),
        Some(2),
        "epoch.epoch should match the fixture's epoch field"
    );
    assert_eq!(
        epoch.get("branch").and_then(|v| v.as_str()),
        Some("main-epoch-2"),
        "epoch.branch should match the fixture's branch"
    );
    assert!(
        epoch.get("snapshot").map(|v| !v.is_null()).unwrap_or(false),
        "active fixture carries a snapshot block, got {:?}",
        epoch.get("snapshot")
    );
    assert!(
        epoch.get("redirect").map(|v| v.is_null()).unwrap_or(true),
        "active fixture must not carry redirect, got {:?}",
        epoch.get("redirect")
    );
}

#[tokio::test]
async fn status_exposes_redirected_epoch() {
    let tmp = TempDir::new().unwrap();
    write_epoch_file(tmp.path(), REDIRECTED_YAML);
    let state = make_state(tmp.path());
    state
        .refresh_epoch_status()
        .expect("refresh redirected file");

    let resp = handle_request(Request::Status, state.clone()).await;
    assert!(resp.ok, "status request should succeed, got {:?}", resp);

    let data = resp.data.expect("status response carries data");
    let epoch = data
        .get("epoch")
        .expect("redirected YAML → status.data.epoch must be present")
        .clone();

    assert_eq!(
        epoch.get("status").and_then(|v| v.as_str()),
        Some("redirected"),
        "epoch.status should serialize to 'redirected'"
    );
    let redirect = epoch
        .get("redirect")
        .expect("redirected fixture carries a redirect block");
    assert_eq!(
        redirect.get("target_branch").and_then(|v| v.as_str()),
        Some("main-epoch-2"),
        "redirect.target_branch should round-trip from YAML"
    );
    assert_eq!(
        redirect.get("target_commit").and_then(|v| v.as_str()),
        Some("1122334455667788990011223344556677889900"),
        "redirect.target_commit should round-trip from YAML"
    );
    assert!(
        epoch.get("snapshot").map(|v| v.is_null()).unwrap_or(true),
        "redirected fixture must not carry snapshot, got {:?}",
        epoch.get("snapshot")
    );
}

#[tokio::test]
async fn status_omits_epoch_when_no_file() {
    // No `gitim.epoch.yaml` on disk → snapshot is None → wire `epoch` field
    // is omitted entirely. This keeps the response shape byte-identical for
    // legacy repos and pre-observability clients.
    let tmp = TempDir::new().unwrap();
    let state = make_state(tmp.path());
    state.refresh_epoch_status().expect("refresh no-file path");
    assert!(state.epoch_status_snapshot().is_none());

    let resp = handle_request(Request::Status, state.clone()).await;
    assert!(resp.ok, "status request should succeed, got {:?}", resp);

    let data = resp.data.expect("status response carries data");
    assert!(
        data.get("epoch").is_none(),
        "no epoch file → status data must omit `epoch` key, got {:?}",
        data.get("epoch")
    );
    // Sanity: pre-existing fields still serialize.
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("running"),
        "baseline status field should still be present"
    );
}
