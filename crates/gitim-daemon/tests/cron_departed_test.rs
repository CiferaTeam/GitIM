//! Integration tests for the departed-author guard on cron mutation
//! handlers (create / enable / disable / delete) and the delete/fire
//! race re-check in `cron_engine::fire`.
//!
//! Pattern mirrors `cron_lifecycle_test.rs` — temp git repo + AppState,
//! exercised via `handle_request`. The guards key off
//! `archive/users/<author>.meta.yaml` existence; tests fabricate that
//! file directly instead of running the real depart pipeline (which
//! would do additional unrelated work).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::{Config, CronSpec, Handler};
use gitim_daemon::api::Request;
use gitim_daemon::cron_engine::{fire, FireRequest};
use gitim_daemon::cron_paths::format_thread_filename_ts;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in ["alice", "bob"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["init"]);
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "init"]);

    let (tx, _) = broadcast::channel(100);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string(), "bob".to_string()];
    }
    (tmp, state)
}

/// Mark `<handler>` as departed by writing
/// `archive/users/<handler>.meta.yaml` directly. Mirrors what the depart
/// pipeline's Phase 4 git-mv would leave on disk.
fn mark_departed(state: &AppState, handler: &str) {
    let archive_dir = state.repo_root.join("archive/users");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(
        archive_dir.join(format!("{}.meta.yaml", handler)),
        format!("display_name: {}\nrole: dev\nintroduction: hi\n", handler),
    )
    .unwrap();
}

async fn create_cron_as(
    state: Arc<AppState>,
    name: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_cron",
        "name": name,
        "schedule": "@daily",
        "target": "alice",
        "prompt": "hi",
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn enable_cron_as(
    state: Arc<AppState>,
    name: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "enable_cron",
        "name": name,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn disable_cron_as(
    state: Arc<AppState>,
    name: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "disable_cron",
        "name": name,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn delete_cron_as(
    state: Arc<AppState>,
    name: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "delete_cron",
        "name": name,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

#[tokio::test]
async fn cron_create_rejects_departed_author() {
    let (_tmp, state) = setup_test_repo().await;
    mark_departed(&state, "alice");
    let resp = create_cron_as(state.clone(), "weekly", "alice").await;
    assert!(!resp.ok, "departed author must not create cron");
    assert_eq!(
        resp.error_code.as_deref(),
        Some("self_departed"),
        "expected self_departed code, got {:?} / msg: {:?}",
        resp.error_code,
        resp.error
    );
}

#[tokio::test]
async fn cron_enable_rejects_departed_author() {
    let (_tmp, state) = setup_test_repo().await;
    // Spec exists; alice originally created it.
    create_cron_as(state.clone(), "weekly", "alice").await;
    // Set alice disabled first via the still-active path so toggle has
    // something meaningful to do.
    disable_cron_as(state.clone(), "weekly", "alice").await;
    // Alice now departs.
    mark_departed(&state, "alice");
    let resp = enable_cron_as(state.clone(), "weekly", "alice").await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("self_departed"));
}

#[tokio::test]
async fn cron_disable_rejects_departed_author() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron_as(state.clone(), "weekly", "alice").await;
    mark_departed(&state, "alice");
    let resp = disable_cron_as(state.clone(), "weekly", "alice").await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("self_departed"));
}

#[tokio::test]
async fn cron_delete_rejects_departed_author() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron_as(state.clone(), "weekly", "alice").await;
    mark_departed(&state, "alice");
    let resp = delete_cron_as(state.clone(), "weekly", "alice").await;
    assert!(!resp.ok);
    assert_eq!(resp.error_code.as_deref(), Some("self_departed"));
}

/// `cron_engine::fire` must re-check `spec.yaml` existence under the
/// commit_lock — between scan_due and fire, a delete_cron may have
/// archived the spec dir. Without the re-check, `create_dir_all` would
/// resurrect an empty dir and the write would land an orphan thread.
#[tokio::test]
async fn fire_skips_when_spec_deleted_under_lock() {
    let (_tmp, state) = setup_test_repo().await;

    let spec_name = "weekly";
    let spec = CronSpec {
        version: 1,
        schedule: "@daily".to_string(),
        timezone: None,
        target: Handler::new("alice").unwrap(),
        prompt: "hi".to_string(),
        enabled: true,
        created_by: Handler::new("alice").unwrap(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        extra: BTreeMap::new(),
    };

    // Build the FireRequest without ever creating the spec dir on disk.
    // This simulates the post-delete state: scan_due saw the dir at
    // some point (we don't enforce that here — we test the fire-side
    // race guard directly), but by the time fire() runs the dir is
    // gone.
    let fire_ts = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    let request = FireRequest {
        spec_name: spec_name.to_string(),
        spec,
        theoretical_ts: fire_ts,
    };

    let result = fire(&state, request).await;
    assert!(
        result.is_ok(),
        "fire should return Ok on missing spec, got {:?}",
        result.err()
    );

    // No active dir was resurrected.
    let active_dir = state.repo_root.join("crons").join(spec_name);
    assert!(
        !active_dir.exists(),
        "fire must NOT create active dir for a deleted spec"
    );
    // Belt-and-braces: no orphan thread file landed where the dir would
    // have been.
    let stem = format_thread_filename_ts(fire_ts);
    let orphan = active_dir.join(format!("{stem}.thread"));
    assert!(
        !orphan.exists(),
        "no thread file should be written when spec is missing"
    );
}
