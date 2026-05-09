//! Integration tests for `handle_archive_dm`, `handle_unarchive_dm`,
//! and `handle_list_archived_dms`.
//!
//! Pattern mirrors `archive_user_test.rs`: temp git repo + AppState
//! in-process, exercise via `handle_request`. No daemon process spawned.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Build a temp git repo with alice + bob + charlie registered, plus
/// an `dm/alice--bob.thread` populated with one line so archive has
/// something to move. Returns (_tmp, state) — keep _tmp alive for the
/// duration of the test.
async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    for h in ["alice", "bob", "charlie"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

    // dm/alice--bob.thread — one message so the file is non-empty (archive
    // operation moves an existing file; `git mv` on missing file errors out).
    let dm_dir = root.join("dm");
    std::fs::create_dir_all(&dm_dir).unwrap();
    std::fs::write(
        dm_dir.join("alice--bob.thread"),
        "[L000001][P000000][@alice][20260509T100000Z] hey bob\n",
    )
    .unwrap();

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
        *users = vec![
            "alice".to_string(),
            "bob".to_string(),
            "charlie".to_string(),
        ];
    }

    (tmp, state)
}

async fn archive_dm(
    state: Arc<AppState>,
    peer: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_dm",
        "peer": peer,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn unarchive_dm(
    state: Arc<AppState>,
    peer: &str,
    author: &str,
) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_dm",
        "peer": peer,
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn list_archived_dms(state: Arc<AppState>, author: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "list_archived_dms",
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

fn git_log_subjects(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "--pretty=%s"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn git_status_clean(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

// ─── 1. happy-path: archive + restore round trip ──────────────────────────────

#[tokio::test]
async fn test_archive_dm_round_trip() {
    let (_tmp, state) = setup_test_repo().await;

    // Pre-condition: dm/alice--bob.thread exists, no archive entry.
    assert!(state.repo_root.join("dm/alice--bob.thread").exists());
    assert!(!state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    // Archive (alice initiating).
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["archived_by"].as_str().unwrap(), "alice");
    assert_eq!(data["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    // File moved.
    assert!(
        !state.repo_root.join("dm/alice--bob.thread").exists(),
        "active dm should be gone"
    );
    assert!(
        state
            .repo_root
            .join("archive/dm/alice--bob.thread")
            .exists(),
        "archive dm should exist"
    );

    // Commit recorded.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: dm with @bob"), "log: {}", log);

    // list_archived_dms — alice sees her DM with bob.
    let la = list_archived_dms(state.clone(), "alice").await;
    assert!(la.ok);
    let dms = la.data.unwrap()["dms"].clone();
    let arr = dms.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["peer"].as_str().unwrap(), "bob");
    assert_eq!(arr[0]["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    // bob also sees the archived DM (sym sort) — peer is alice from his POV.
    let lb = list_archived_dms(state.clone(), "bob").await;
    assert!(lb.ok);
    let dms = lb.data.unwrap()["dms"].clone();
    let arr = dms.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["peer"].as_str().unwrap(), "alice");
    assert_eq!(arr[0]["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    // Now restore.
    let resp = unarchive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "unarchive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["unarchived_by"].as_str().unwrap(), "alice");
    assert_eq!(data["dm_pair_stem"].as_str().unwrap(), "alice--bob");

    assert!(state.repo_root.join("dm/alice--bob.thread").exists());
    assert!(!state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    // Both archive and restore commits present.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: dm with @bob"), "log: {}", log);
    assert!(
        log.contains("archive: restore dm with @bob"),
        "log: {}",
        log
    );

    // Working tree clean.
    assert!(
        git_status_clean(&state.repo_root).trim().is_empty(),
        "working tree should stay clean"
    );

    // After restore, list_archived_dms is empty for both participants.
    let la = list_archived_dms(state.clone(), "alice").await;
    assert!(la.data.unwrap()["dms"].as_array().unwrap().is_empty());
    let lb = list_archived_dms(state.clone(), "bob").await;
    assert!(lb.data.unwrap()["dms"].as_array().unwrap().is_empty());
}

// ─── 2. archive non-existent DM ───────────────────────────────────────────────

#[tokio::test]
async fn test_archive_nonexistent_dm() {
    let (_tmp, state) = setup_test_repo().await;

    let before_log = git_log_subjects(&state.repo_root);

    // alice has no DM with charlie — archive should error.
    let resp = archive_dm(state.clone(), "charlie", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("DM with @charlie not found"),
        "err: {}",
        err
    );

    // No git side-effects.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 3. archive an already-archived DM ────────────────────────────────────────

#[tokio::test]
async fn test_archive_already_archived_dm() {
    let (_tmp, state) = setup_test_repo().await;

    // First archive succeeds.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "first archive failed: {:?}", resp.error);

    let before_log = git_log_subjects(&state.repo_root);

    // Second archive must error cleanly.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("DM with @bob is already archived"),
        "err: {}",
        err
    );

    // No new git operations.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 4. list_archived_dms filters by caller participation ─────────────────────

#[tokio::test]
async fn test_list_archived_dms_filters_by_caller() {
    let (_tmp, state) = setup_test_repo().await;

    // alice archives her DM with bob.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // alice (participant) sees it.
    let la = list_archived_dms(state.clone(), "alice").await;
    assert!(la.ok);
    let arr_a = la.data.unwrap()["dms"].as_array().unwrap().clone();
    assert_eq!(arr_a.len(), 1);

    // bob (participant) sees it.
    let lb = list_archived_dms(state.clone(), "bob").await;
    assert!(lb.ok);
    let arr_b = lb.data.unwrap()["dms"].as_array().unwrap().clone();
    assert_eq!(arr_b.len(), 1);

    // charlie (third party) does NOT see it.
    let lc = list_archived_dms(state.clone(), "charlie").await;
    assert!(lc.ok);
    let arr_c = lc.data.unwrap()["dms"].as_array().unwrap().clone();
    assert!(
        arr_c.is_empty(),
        "charlie should not see alice<->bob DM, got {:?}",
        arr_c
    );
}

// ─── 5. unarchive non-existent / not-archived DM ──────────────────────────────

#[tokio::test]
async fn test_unarchive_dm_not_archived() {
    let (_tmp, state) = setup_test_repo().await;

    let before_log = git_log_subjects(&state.repo_root);

    // alice<->bob DM is still active — unarchive should error.
    let resp = unarchive_dm(state.clone(), "bob", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("DM with @bob is not archived"),
        "err: {}",
        err
    );

    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

#[tokio::test]
async fn test_unarchive_dm_missing_in_archive() {
    let (_tmp, state) = setup_test_repo().await;

    // alice has no DM with charlie at all (active or archived).
    let resp = unarchive_dm(state.clone(), "charlie", "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    // Active path doesn't exist → falls through to archive check, which
    // also doesn't exist → "not found in archive".
    assert!(
        err.contains("not found in archive"),
        "err: {}",
        err
    );
}

// ─── 6. archive commit failure rolls back git mv ──────────────────────────────

#[tokio::test]
async fn test_archive_dm_rolls_back_on_commit_failure() {
    let (_tmp, state) = setup_test_repo().await;

    // Pre-commit hook that always rejects, forcing commit failure after
    // git mv has moved the thread file.
    let hooks_dir = state.repo_root.join(".git/hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook_path = hooks_dir.join("pre-commit");
    std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let before_log = git_log_subjects(&state.repo_root);

    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(!resp.ok, "archive should fail when commit is rejected");
    let err = resp.error.unwrap();
    assert!(err.contains("rolled back"), "err: {}", err);

    // File still in active location after rollback.
    assert!(
        state.repo_root.join("dm/alice--bob.thread").exists(),
        "active dm must be back after rollback"
    );
    assert!(
        !state
            .repo_root
            .join("archive/dm/alice--bob.thread")
            .exists(),
        "archive dm must not remain after rollback"
    );

    // No commit recorded.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);

    // Working tree clean.
    assert!(
        git_status_clean(&state.repo_root).trim().is_empty(),
        "working tree should be clean after rollback"
    );

    // After removing the hook, retry succeeds.
    std::fs::remove_file(&hook_path).unwrap();
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(
        resp.ok,
        "retry after hook removal should succeed: {:?}",
        resp.error
    );
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());
    assert!(!state.repo_root.join("dm/alice--bob.thread").exists());
}

// ─── 7. either party can archive (decision B1: single-party archive) ──────────

#[tokio::test]
async fn test_either_party_can_archive_dm() {
    let (_tmp, state) = setup_test_repo().await;

    // alice archives, then unarchives.
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "alice archive failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());

    let resp = unarchive_dm(state.clone(), "alice", "bob").await;
    assert!(resp.ok, "bob unarchive failed: {:?}", resp.error);
    assert!(state.repo_root.join("dm/alice--bob.thread").exists());

    // Now bob archives the same DM independently.
    let resp = archive_dm(state.clone(), "alice", "bob").await;
    assert!(resp.ok, "bob archive failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/dm/alice--bob.thread")
        .exists());
    assert!(!state.repo_root.join("dm/alice--bob.thread").exists());

    // Commit log shows both authors.
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: dm with @bob"), "log: {}", log);
    assert!(
        log.contains("archive: restore dm with @alice"),
        "log: {}",
        log
    );
    assert!(log.contains("archive: dm with @alice"), "log: {}", log);
}

// ─── 8. list_archived_dms empty + sorted ──────────────────────────────────────

#[tokio::test]
async fn test_list_archived_dms_empty_then_sorted() {
    let (_tmp, state) = setup_test_repo().await;

    // Empty before any archive.
    let resp = list_archived_dms(state.clone(), "alice").await;
    assert!(resp.ok);
    let arr = resp.data.unwrap()["dms"].as_array().unwrap().clone();
    assert!(arr.is_empty());

    // Add a second DM file alice<->charlie so we can verify sort order.
    std::fs::write(
        state.repo_root.join("dm/alice--charlie.thread"),
        "[L000001][P000000][@alice][20260509T101000Z] hey charlie\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "dm/alice--charlie.thread"])
        .current_dir(&state.repo_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "add charlie dm"])
        .current_dir(&state.repo_root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();

    // Archive bob first, then charlie (insertion order != alphabetical).
    let resp = archive_dm(state.clone(), "bob", "alice").await;
    assert!(resp.ok, "archive bob failed: {:?}", resp.error);
    let resp = archive_dm(state.clone(), "charlie", "alice").await;
    assert!(resp.ok, "archive charlie failed: {:?}", resp.error);

    let resp = list_archived_dms(state.clone(), "alice").await;
    assert!(resp.ok);
    let arr = resp.data.unwrap()["dms"].as_array().unwrap().clone();
    assert_eq!(arr.len(), 2);
    // Sorted by peer alphabetically: bob < charlie.
    assert_eq!(arr[0]["peer"].as_str().unwrap(), "bob");
    assert_eq!(arr[1]["peer"].as_str().unwrap(), "charlie");
}
