//! Integration tests for `handle_unarchive_channel`.
//!
//! Flow: set up a git-backed repo with two users, create a channel as alice,
//! archive it (using the existing archive_channel handler as the fixture), then
//! exercise the new unarchive_channel handler across the five scenarios below.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Build a temp git repo with alice + bob registered, and a single channel "dev"
/// created by alice. Returns (_tmp, state) — keep _tmp alive for the test.
async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    std::fs::write(
        root.join("users/bob.meta.yaml"),
        "display_name: Bob\nrole: dev\nintroduction: hello\n",
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
    // Make the working tree non-empty before first commit so the repo has a HEAD
    // consistent with our downstream handlers expecting the dir tracked.
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

    // Create channel "dev" as alice via the real handler so the git history mirrors
    // production reality (create → archive → unarchive).
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": "dev",
        "author": "alice",
    }))
    .unwrap();
    let resp = handle_request(req, state.clone()).await;
    assert!(resp.ok, "setup: create_channel failed: {:?}", resp.error);

    (tmp, state)
}

async fn archive_dev_as(state: Arc<AppState>, author: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_channel",
        "channel": "dev",
        "author": author,
    }))
    .unwrap();
    handle_request(req, state).await
}

async fn unarchive_dev_as(state: Arc<AppState>, author: &str) -> gitim_daemon::api::Response {
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "unarchive_channel",
        "channel": "dev",
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

// ─── 1. happy path ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_unarchive_channel_happy_path() {
    let (_tmp, state) = setup_test_repo().await;

    // Archive first.
    let resp = archive_dev_as(state.clone(), "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);
    assert!(state
        .repo_root
        .join("archive/channels/dev.meta.yaml")
        .exists());
    assert!(!state.repo_root.join("channels/dev.meta.yaml").exists());

    let mut rx = state.event_tx.subscribe();

    // Now unarchive.
    let resp = unarchive_dev_as(state.clone(), "alice").await;
    assert!(resp.ok, "unarchive failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    assert_eq!(data["channel"].as_str().unwrap(), "dev");
    assert_eq!(data["unarchived_by"].as_str().unwrap(), "alice");

    // Files are back in channels/.
    assert!(
        state.repo_root.join("channels/dev.meta.yaml").exists(),
        "meta should be restored to active location"
    );
    assert!(
        state.repo_root.join("channels/dev.thread").exists(),
        "thread should be restored to active location"
    );

    // Archive copies gone.
    assert!(
        !state.repo_root.join("archive/channels/dev.meta.yaml").exists(),
        "meta should be removed from archive"
    );
    assert!(
        !state
            .repo_root
            .join("archive/channels/dev.thread")
            .exists(),
        "thread should be removed from archive"
    );

    // Commit history contains both archive and unarchive ops (in reverse order).
    let log = git_log_subjects(&state.repo_root);
    assert!(log.contains("archive: #dev by @alice"), "log: {}", log);
    assert!(
        log.contains("unarchive: #dev by @alice"),
        "log: {}",
        log
    );

    // SSE event emitted.
    let ev = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("event channel closed");
    match ev {
        Event::ChannelUnarchived {
            channel,
            author,
            timestamp,
        } => {
            assert_eq!(channel, "dev");
            assert_eq!(author, "alice");
            assert!(!timestamp.is_empty());
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

// ─── 2. archive source missing ────────────────────────────────────────────────

#[tokio::test]
async fn test_unarchive_channel_source_missing() {
    let (_tmp, state) = setup_test_repo().await;
    // Do NOT archive. Try to unarchive straight away.

    let before_log = git_log_subjects(&state.repo_root);

    let resp = unarchive_dev_as(state.clone(), "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("archive source does not exist"),
        "err: {}",
        err
    );

    // No git side-effects.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log, "no new commits should be created");
    assert!(
        git_status_clean(&state.repo_root).trim().is_empty(),
        "working tree should stay clean"
    );
}

// ─── 3. non-creator caller ────────────────────────────────────────────────────

#[tokio::test]
async fn test_unarchive_channel_non_creator() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = archive_dev_as(state.clone(), "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    let before_log = git_log_subjects(&state.repo_root);

    let resp = unarchive_dev_as(state.clone(), "bob").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("only channel creator can unarchive"),
        "err: {}",
        err
    );

    // Archive side intact.
    assert!(state
        .repo_root
        .join("archive/channels/dev.meta.yaml")
        .exists());
    assert!(state
        .repo_root
        .join("archive/channels/dev.thread")
        .exists());
    assert!(!state.repo_root.join("channels/dev.meta.yaml").exists());

    // No new commit.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
    assert!(git_status_clean(&state.repo_root).trim().is_empty());
}

// ─── 4. name conflict ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_unarchive_channel_name_conflict() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = archive_dev_as(state.clone(), "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Re-create a channel of the same name at the active location (simulating the
    // "name recycled while archived" scenario).
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": "dev",
        "author": "alice",
    }))
    .unwrap();
    // create_channel currently refuses if the name exists in archive. Work around
    // by manually writing an active meta file instead — this exercises the
    // conflict branch of unarchive_channel directly.
    let _ = req; // silence unused warning
    std::fs::create_dir_all(state.repo_root.join("channels")).unwrap();
    std::fs::write(
        state.repo_root.join("channels/dev.meta.yaml"),
        "display_name: dev\ncreated_by: alice\ncreated_at: 20260101T000000Z\nintroduction: ''\nmembers: []\n",
    )
    .unwrap();

    let before_log = git_log_subjects(&state.repo_root);

    let resp = unarchive_dev_as(state.clone(), "alice").await;
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert!(
        err.contains("already exists in active location"),
        "err: {}",
        err
    );

    // Archive files still present.
    assert!(state
        .repo_root
        .join("archive/channels/dev.meta.yaml")
        .exists());
    assert!(state
        .repo_root
        .join("archive/channels/dev.thread")
        .exists());

    // No new commit.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log);
}

// ─── 5. commit failure triggers full rollback ────────────────────────────────

#[tokio::test]
async fn test_unarchive_channel_rolls_back_on_commit_failure() {
    let (_tmp, state) = setup_test_repo().await;
    let resp = archive_dev_as(state.clone(), "alice").await;
    assert!(resp.ok, "archive failed: {:?}", resp.error);

    // Install a pre-commit hook that rejects all commits, forcing commit failure
    // after git mv has already moved both files into the active location.
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

    let resp = unarchive_dev_as(state.clone(), "alice").await;
    assert!(!resp.ok, "unarchive should fail when commit is rejected");
    let err = resp.error.unwrap();
    assert!(err.contains("rolled back"), "err should mention rollback: {}", err);

    // Both files must be back in archive/ (rollback reversed BOTH mvs).
    assert!(
        state
            .repo_root
            .join("archive/channels/dev.meta.yaml")
            .exists(),
        "meta should be back in archive after rollback"
    );
    assert!(
        state
            .repo_root
            .join("archive/channels/dev.thread")
            .exists(),
        "thread should be back in archive after rollback"
    );

    // Active location must not hold either file (rollback complete, not partial).
    assert!(
        !state.repo_root.join("channels/dev.meta.yaml").exists(),
        "meta must not remain in active location"
    );
    assert!(
        !state.repo_root.join("channels/dev.thread").exists(),
        "thread must not remain in active location"
    );

    // No commit was created.
    let after_log = git_log_subjects(&state.repo_root);
    assert_eq!(before_log, after_log, "no commit should be recorded");

    // Working tree clean after full rollback.
    let status = git_status_clean(&state.repo_root);
    assert!(
        status.trim().is_empty(),
        "working tree should be clean after rollback, got: {}",
        status
    );

    // After removing the hook, a retry succeeds.
    std::fs::remove_file(&hook_path).unwrap();
    let resp = unarchive_dev_as(state.clone(), "alice").await;
    assert!(
        resp.ok,
        "retry after hook removal should succeed: {:?}",
        resp.error
    );
    assert!(state.repo_root.join("channels/dev.meta.yaml").exists());
    assert!(state.repo_root.join("channels/dev.thread").exists());
    assert!(!state
        .repo_root
        .join("archive/channels/dev.meta.yaml")
        .exists());
}
