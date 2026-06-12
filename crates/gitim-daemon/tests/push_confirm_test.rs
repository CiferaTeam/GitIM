#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::Handler;
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_git_capture(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// Set up a local-only git repo (no remote) with GitIM structure.
/// Returns (TempDir, AppState).
async fn setup_no_remote() -> (TempDir, Arc<AppState>) {
    common::setup_repo_with_channel("general").await
}

/// Set up a bare repo + clone with GitIM structure, suitable for push tests.
/// Returns (bare_dir, clone_dir, AppState).
async fn setup_with_remote() -> (TempDir, TempDir, Arc<AppState>) {
    let bare_dir = TempDir::new().unwrap();
    let clone_dir = TempDir::new().unwrap();

    common::init_bare_and_clone(bare_dir.path(), clone_dir.path());

    let root = clone_dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    common::write_alice(&root);
    common::write_channel_meta(&root, "general", "alice", &[]);

    common::run_git(&root, &["add", "."]);
    common::run_git(&root, &["commit", "-m", "initial structure"]);
    common::run_git(&root, &["push", "-u", "origin", "HEAD"]);

    let state = common::make_state(root, Some("alice"), &["alice"]).await;

    (bare_dir, clone_dir, state)
}

fn send_request(channel: &str, body: &str) -> Request {
    Request::Send {
        channel: channel.to_string(),
        body: body.to_string(),
        reply_to: None,
        author: Some("alice".to_string()),
    }
}

/// Clone the bare repo into a fresh tempdir and return (TempDir, path).
fn clone_bare(bare_path: &Path) -> TempDir {
    let verify = TempDir::new().unwrap();
    common::run_git(
        verify.path().parent().unwrap(),
        &[
            "clone",
            bare_path.to_str().unwrap(),
            verify.path().to_str().unwrap(),
        ],
    );
    verify
}

// ---------------------------------------------------------------------------
// Test 1: No-remote — send returns "committed" immediately
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_no_remote_returns_committed() {
    let (_tmp, state) = setup_no_remote().await;

    // Confirm has_remote is false
    assert!(!state.has_remote, "local-only repo should have no remote");

    let resp = handle_request(send_request("general", "hello no-remote"), state).await;
    assert!(resp.ok, "send should succeed");

    let data = resp.data.unwrap();
    assert_eq!(
        data["status"], "committed",
        "without remote, status should be committed"
    );
    assert_eq!(data["line_number"], 1);
    assert_eq!(data["channel"], "general");
}

// ---------------------------------------------------------------------------
// Test 2: With-remote — send returns "committed" + commit_id immediately,
// regardless of whether push has completed. (Push status is observable via
// sync_loop log / future SSE events, not inline in the send response.)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_with_remote_returns_committed() {
    let (_bare_dir, clone_dir, state) = setup_with_remote().await;

    assert!(state.has_remote, "cloned repo should have remote");

    // Spawn the sync loop — push will eventually happen but the response
    // must NOT depend on it.
    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let resp = handle_request(send_request("general", "hello push"), state.clone()).await;
    assert!(resp.ok, "send should succeed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(
        data["status"], "committed",
        "with remote, status should be committed (push is async)"
    );
    let commit_id = data["commit_id"]
        .as_str()
        .expect("commit_id should be present");
    assert!(!commit_id.is_empty(), "commit_id should be non-empty");

    // commit_id must equal the local HEAD captured under commit_lock.
    let local_head = run_git_capture(clone_dir.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        commit_id, local_head,
        "commit_id should equal local HEAD at time of commit"
    );

    assert_eq!(data["line_number"], 1);
    assert_eq!(data["channel"], "general");
}

// ---------------------------------------------------------------------------
// Test 3: Sequential sends — both return "committed" with correct line_numbers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sequential_sends_return_committed() {
    let (_bare_dir, _clone_dir, state) = setup_with_remote().await;

    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // First message
    let resp1 = handle_request(send_request("general", "msg one"), state.clone()).await;
    assert!(resp1.ok, "first send should succeed: {:?}", resp1.error);
    let data1 = resp1.data.unwrap();
    assert_eq!(data1["status"], "committed");
    assert_eq!(data1["line_number"], 1);
    assert!(data1["commit_id"].as_str().is_some_and(|s| !s.is_empty()));

    // Second message
    let resp2 = handle_request(send_request("general", "msg two"), state.clone()).await;
    assert!(resp2.ok, "second send should succeed: {:?}", resp2.error);
    let data2 = resp2.data.unwrap();
    assert_eq!(data2["status"], "committed");
    assert_eq!(data2["line_number"], 2);
    assert!(data2["commit_id"].as_str().is_some_and(|s| !s.is_empty()));

    // The two commit_ids should differ (two distinct commits).
    assert_ne!(data1["commit_id"], data2["commit_id"]);
}

// ---------------------------------------------------------------------------
// Test 3b: Push eventually reaches the remote (sync_loop responsibility,
// verified independently of send response).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_loop_eventually_pushes_committed_messages() {
    let (bare_dir, _clone_dir, state) = setup_with_remote().await;

    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let resp = handle_request(send_request("general", "syncs eventually"), state.clone()).await;
    assert!(resp.ok);

    // Wait for the sync loop to push the message to the bare remote.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_count = 0usize;
    while std::time::Instant::now() < deadline {
        let verify = clone_bare(bare_dir.path());
        let path = verify.path().join("channels/general.thread");
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if let Ok(file) = parse_thread(&content) {
                last_count = file.messages().len();
                if last_count >= 1 && file.messages()[0].body == "syncs eventually" {
                    return;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("sync_loop did not push message within 10s (saw {last_count} messages on remote)");
}

// ---------------------------------------------------------------------------
// Test 4: Push conflict — daemon rebases and still returns "pushed"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_conflict_still_succeeds() {
    let (_bare_dir, _clone_dir, state) = setup_with_remote().await;

    // Create a second clone that will push a conflicting commit
    let rival = clone_bare(_bare_dir.path());
    common::run_git(rival.path(), &["config", "user.email", "rival@test.com"]);
    common::run_git(rival.path(), &["config", "user.name", "rival"]);

    // Rival writes a message directly to the thread and pushes
    let bob = Handler::new("bob").unwrap();
    let rival_msg = format_message(1, 0, &bob, "20260325T120000Z", "rival msg");
    std::fs::write(rival.path().join("channels/general.thread"), &rival_msg).unwrap();
    // Also create bob's user file so the thread content is valid
    std::fs::create_dir_all(rival.path().join("users")).ok();
    common::write_user(rival.path(), "bob", "Bob", "dev", "hi");
    common::run_git(rival.path(), &["add", "."]);
    common::run_git(rival.path(), &["commit", "-m", "rival: bob msg"]);
    common::run_git(rival.path(), &["push"]);

    // Now spawn sync loop and send from the daemon
    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let resp = handle_request(
        send_request("general", "alice after conflict"),
        state.clone(),
    )
    .await;
    assert!(
        resp.ok,
        "send should succeed after conflict: {:?}",
        resp.error
    );

    let data = resp.data.unwrap();
    assert_eq!(
        data["status"], "committed",
        "send returns committed regardless of conflict — rebase + push are sync_loop's job"
    );
    assert!(
        data["commit_id"].as_str().is_some_and(|s| !s.is_empty()),
        "commit_id should be non-empty (local HEAD at commit time)"
    );

    // Wait for sync_loop to rebase + push both messages to the remote.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let verify = clone_bare(_bare_dir.path());
        let path = verify.path().join("channels/general.thread");
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if let Ok(file) = parse_thread(&content) {
                if file.messages().len() == 2
                    && file.messages()[0].author.as_str() == "bob"
                    && file.messages()[1].author.as_str() == "alice"
                {
                    assert_eq!(file.messages()[0].body, "rival msg");
                    assert_eq!(file.messages()[1].body, "alice after conflict");
                    return;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!("sync_loop did not converge within 15s after conflict");
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

// ---------------------------------------------------------------------------
// Test 5: send must NOT block on push completion. With an unreachable remote
// (bare repo deleted), push will fail forever. send still returns
// `committed` + commit_id in well under one sync cycle.
//
// This is the regression test for the timeout-retry duplicate-message bug:
// previously send awaited push_rx, so an unreachable / slow remote forced
// the client into Timeout, the agent layer retried, and the same message
// landed twice in the thread.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_does_not_block_when_push_fails() {
    let (bare_dir, _clone_dir, state) = setup_with_remote().await;

    // Simulate unreachable remote: blow away the bare repo entirely.
    std::fs::remove_dir_all(bare_dir.path()).unwrap();

    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let start = std::time::Instant::now();
    let resp = handle_request(
        send_request("general", "hello unreachable remote"),
        state.clone(),
    )
    .await;
    let elapsed = start.elapsed();

    assert!(
        resp.ok,
        "send should succeed even when push will fail: {:?}",
        resp.error
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "send should return immediately (~commit-time, sub-second), not wait for push to fail: took {:?}",
        elapsed
    );

    let data = resp.data.unwrap();
    assert_eq!(
        data["status"], "committed",
        "status should be committed regardless of push outcome"
    );
    assert!(
        data["commit_id"].as_str().is_some_and(|s| !s.is_empty()),
        "commit_id should be the local HEAD hash"
    );
}
