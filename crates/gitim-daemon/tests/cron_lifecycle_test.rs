#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `handle_disable_cron`, `handle_enable_cron`,
//! and `handle_delete_cron`.

mod common;

use std::sync::Arc;

use gitim_core::types::CronSpec;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

async fn setup_test_repo() -> (tempfile::TempDir, Arc<AppState>) {
    common::setup_repo_with_users(&["alice", "bob"]).await
}

async fn create_cron(state: Arc<AppState>, name: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_cron",
        "name": name,
        "schedule": "@daily",
        "target": "alice",
        "prompt": "hi",
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn enable_cron(state: Arc<AppState>, name: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "enable_cron",
        "name": name,
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn disable_cron(state: Arc<AppState>, name: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "disable_cron",
        "name": name,
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn delete_cron(state: Arc<AppState>, name: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "delete_cron",
        "name": name,
        "author": "alice",
    }))
    .unwrap();
    handle_request(req, state).await
}

fn read_spec(state: &Arc<AppState>, name: &str) -> CronSpec {
    let body =
        std::fs::read_to_string(state.repo_root.join(format!("crons/{}/spec.yaml", name))).unwrap();
    CronSpec::from_yaml(&body).unwrap()
}

fn git_log_subjects(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn count_commits(root: &std::path::Path) -> usize {
    git_log_subjects(root)
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

fn git_status_clean(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ─── disable / enable round trip ─────────────────────────────────────────────

#[tokio::test]
async fn disable_then_enable_roundtrip() {
    let (_tmp, state) = setup_test_repo().await;
    if let Some(e) = create_cron(state.clone(), "weekly").await.error {
        panic!("{}", e);
    }

    // Created enabled.
    let spec = read_spec(&state, "weekly");
    assert!(spec.enabled);
    let original_target = spec.target.as_str().to_string();
    let original_schedule = spec.schedule.clone();
    let original_prompt = spec.prompt.clone();
    let original_created_by = spec.created_by.as_str().to_string();
    let original_created_at = spec.created_at.clone();

    // Disable.
    let r = disable_cron(state.clone(), "weekly").await;
    assert!(r.ok, "disable failed: {:?}", r.error);
    let data = r.data.unwrap();
    assert_eq!(data["enabled"], false);
    assert_eq!(data["changed"], true);
    let spec2 = read_spec(&state, "weekly");
    assert!(!spec2.enabled);
    // Other fields unchanged.
    assert_eq!(spec2.target.as_str(), original_target);
    assert_eq!(spec2.schedule, original_schedule);
    assert_eq!(spec2.prompt, original_prompt);
    assert_eq!(spec2.created_by.as_str(), original_created_by);
    assert_eq!(spec2.created_at, original_created_at);

    // Enable again.
    let r = enable_cron(state.clone(), "weekly").await;
    assert!(r.ok, "enable failed: {:?}", r.error);
    let data = r.data.unwrap();
    assert_eq!(data["enabled"], true);
    assert_eq!(data["changed"], true);
    let spec3 = read_spec(&state, "weekly");
    assert!(spec3.enabled);
    assert_eq!(spec3.created_at, original_created_at);

    // Three commits since init: create + disable + enable.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("cron: create weekly by @alice"), "log: {log}");
    assert!(log.contains("cron: disable weekly by @alice"), "log: {log}");
    assert!(log.contains("cron: enable weekly by @alice"), "log: {log}");

    // Working tree clean.
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── idempotent paths produce no commit ──────────────────────────────────────

#[tokio::test]
async fn disable_already_disabled_idempotent() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "weekly").await;
    let r1 = disable_cron(state.clone(), "weekly").await;
    assert!(r1.ok && r1.data.unwrap()["changed"] == true);

    let before = count_commits(&state.repo_root);

    // Second disable: idempotent — no change, no commit.
    let r2 = disable_cron(state.clone(), "weekly").await;
    assert!(r2.ok);
    let data = r2.data.unwrap();
    assert_eq!(data["enabled"], false);
    assert_eq!(data["changed"], false);

    let after = count_commits(&state.repo_root);
    assert_eq!(before, after, "no new commit on idempotent disable");
}

#[tokio::test]
async fn enable_already_enabled_idempotent() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "weekly").await;
    let before = count_commits(&state.repo_root);

    // Already enabled; this should be a no-op.
    let r = enable_cron(state.clone(), "weekly").await;
    assert!(r.ok);
    let data = r.data.unwrap();
    assert_eq!(data["enabled"], true);
    assert_eq!(data["changed"], false);

    let after = count_commits(&state.repo_root);
    assert_eq!(before, after);
}

// ─── enable / disable on archived ────────────────────────────────────────────

#[tokio::test]
async fn enable_archived_returns_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "weekly").await;
    let r = delete_cron(state.clone(), "weekly").await;
    assert!(r.ok);

    let r = enable_cron(state.clone(), "weekly").await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn disable_archived_returns_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "weekly").await;
    let r = delete_cron(state.clone(), "weekly").await;
    assert!(r.ok);

    let r = disable_cron(state.clone(), "weekly").await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("not_found"));
}

// ─── delete ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_active() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "weekly").await;

    let r = delete_cron(state.clone(), "weekly").await;
    assert!(r.ok, "delete failed: {:?}", r.error);
    let data = r.data.unwrap();
    assert_eq!(data["name"], "weekly");
    assert_eq!(data["deleted_by"], "alice");

    // Active gone.
    assert!(
        !state.repo_root.join("crons/weekly").exists(),
        "crons/weekly should not exist"
    );
    // Archive populated.
    assert!(state
        .repo_root
        .join("archive/crons/weekly/spec.yaml")
        .exists());

    // Commit + working tree clean.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("cron: delete weekly by @alice"), "log: {log}");
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

#[tokio::test]
async fn delete_with_history() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "frequent").await;

    // Seed some thread files BEFORE delete — they should move with the
    // directory.
    let dir = state.repo_root.join("crons/frequent");
    for ts in [
        "2026-05-01T09-00-00Z",
        "2026-05-02T09-00-00Z",
        "2026-05-03T09-00-00Z",
    ] {
        std::fs::write(
            dir.join(format!("{}.thread", ts)),
            format!("[L000001][P000000][@system][{}] cron(frequent): hi\n", ts),
        )
        .unwrap();
    }
    // git add + commit them so `git mv` can pick them up cleanly.
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "seed runs",
        ])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();

    let r = delete_cron(state.clone(), "frequent").await;
    assert!(r.ok, "delete failed: {:?}", r.error);

    // History under archive/crons/frequent/.
    let archive_dir = state.repo_root.join("archive/crons/frequent");
    for ts in [
        "2026-05-01T09-00-00Z",
        "2026-05-02T09-00-00Z",
        "2026-05-03T09-00-00Z",
    ] {
        assert!(
            archive_dir.join(format!("{}.thread", ts)).exists(),
            "archive should contain {}.thread",
            ts
        );
    }
    assert!(archive_dir.join("spec.yaml").exists());

    // Active dir is gone.
    assert!(!state.repo_root.join("crons/frequent").exists());

    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

#[tokio::test]
async fn delete_already_archived_returns_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    create_cron(state.clone(), "weekly").await;
    let r = delete_cron(state.clone(), "weekly").await;
    assert!(r.ok);

    // Second delete: not_found because active path is gone.
    let r2 = delete_cron(state.clone(), "weekly").await;
    assert!(!r2.ok);
    assert_eq!(r2.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn delete_missing_returns_not_found() {
    let (_tmp, state) = setup_test_repo().await;
    let r = delete_cron(state.clone(), "ghost").await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn delete_orphaned_active_and_archive_refuses() {
    let (_tmp, state) = setup_test_repo().await;
    // Create active.
    create_cron(state.clone(), "ghost").await;
    // Manually plant an archive entry simulating a botched prior delete /
    // sync conflict. The handler must refuse rather than blindly overwrite.
    let archive_dir = state.repo_root.join("archive/crons/ghost");
    std::fs::create_dir_all(&archive_dir).unwrap();
    std::fs::write(archive_dir.join("spec.yaml"), "stale\n").unwrap();

    let r = delete_cron(state.clone(), "ghost").await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("archive_conflict"));
}

// ─── name validation passes through ──────────────────────────────────────────

#[tokio::test]
async fn delete_invalid_name() {
    let (_tmp, state) = setup_test_repo().await;
    let r = delete_cron(state.clone(), "WeeklyReport").await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("invalid_name"));
}

#[tokio::test]
async fn enable_invalid_name() {
    let (_tmp, state) = setup_test_repo().await;
    let r = enable_cron(state.clone(), "WeeklyReport").await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("invalid_name"));
}
