use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{Config, Handler};
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

/// Set up a local-only git repo (no remote) with GitIM structure.
/// Returns (TempDir, AppState).
async fn setup_no_remote() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    run_git(&root, &["init"]);
    run_git(&root, &["commit", "--allow-empty", "-m", "init"]);

    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/alice.meta.json"),
        r#"{"display_name":"Alice","role":"dev","introduction":"hi"}"#,
    )
    .unwrap();
    // Create "general" channel meta.json (required by handle_send)
    std::fs::write(
        root.join("channels/general.meta.json"),
        r#"{"display_name":"general","created_by":"alice","created_at":"20260323T000000Z","introduction":"general channel","members":[]}"#,
    )
    .unwrap();

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "add initial structure"]);

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        event_tx,
        Some("alice".to_string()),
    ));

    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

    (tmp, state)
}

/// Set up a bare repo + clone with GitIM structure, suitable for push tests.
/// Returns (bare_dir, clone_dir, AppState).
async fn setup_with_remote() -> (TempDir, TempDir, Arc<AppState>) {
    let bare_dir = TempDir::new().unwrap();
    let clone_dir = TempDir::new().unwrap();

    // Init bare repo
    run_git(bare_dir.path(), &["init", "--bare"]);

    // Clone
    run_git(
        clone_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_dir.path().to_str().unwrap(),
        ],
    );
    run_git(clone_dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(clone_dir.path(), &["config", "user.name", "test"]);

    let root = clone_dir.path().to_path_buf();

    // Create GitIM structure
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join(".gitim")).unwrap();
    std::fs::write(root.join(".gitim/config.yaml"), "version: 1").unwrap();
    std::fs::write(
        root.join("users/alice.meta.json"),
        r#"{"display_name":"Alice","role":"dev","introduction":"hi"}"#,
    )
    .unwrap();
    // Create "general" channel meta.json (required by handle_send)
    std::fs::write(
        root.join("channels/general.meta.json"),
        r#"{"display_name":"general","created_by":"alice","created_at":"20260323T000000Z","introduction":"general channel","members":[]}"#,
    )
    .unwrap();
    // Create an empty thread file so the channel exists
    std::fs::write(root.join("channels/general.thread"), "").unwrap();

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "initial structure"]);
    run_git(&root, &["push", "-u", "origin", "main"]);

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        event_tx,
        Some("alice".to_string()),
    ));

    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

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
    run_git(
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
// Test 2: With-remote — send returns "pushed" with commit_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_with_remote_returns_pushed() {
    let (_bare_dir, _clone_dir, state) = setup_with_remote().await;

    assert!(state.has_remote, "cloned repo should have remote");

    // Spawn the sync loop
    AppState::spawn_sync_loop(state.clone());
    // Give sync loop a moment to initialise
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let resp = handle_request(send_request("general", "hello push"), state.clone()).await;
    assert!(resp.ok, "send should succeed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(
        data["status"], "pushed",
        "with remote + sync, status should be pushed"
    );
    assert!(
        data["commit_id"].as_str().map_or(false, |s| !s.is_empty()),
        "commit_id should be non-empty"
    );
    assert_eq!(data["line_number"], 1);
    assert_eq!(data["channel"], "general");

    // Verify the message actually reached the remote
    let verify = clone_bare(_bare_dir.path());
    let remote_content =
        std::fs::read_to_string(verify.path().join("channels/general.thread")).unwrap();
    let file = parse_thread(&remote_content).unwrap();
    assert_eq!(file.messages().len(), 1);
    assert_eq!(file.messages()[0].body, "hello push");
}

// ---------------------------------------------------------------------------
// Test 3: Sequential sends — both return "pushed" with correct line_numbers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sequential_sends_return_pushed() {
    let (_bare_dir, _clone_dir, state) = setup_with_remote().await;

    AppState::spawn_sync_loop(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // First message
    let resp1 = handle_request(send_request("general", "msg one"), state.clone()).await;
    assert!(resp1.ok, "first send should succeed: {:?}", resp1.error);
    let data1 = resp1.data.unwrap();
    assert_eq!(data1["status"], "pushed");
    assert_eq!(data1["line_number"], 1);

    // Second message
    let resp2 = handle_request(send_request("general", "msg two"), state.clone()).await;
    assert!(resp2.ok, "second send should succeed: {:?}", resp2.error);
    let data2 = resp2.data.unwrap();
    assert_eq!(data2["status"], "pushed");
    assert_eq!(data2["line_number"], 2);

    // Verify both messages in remote
    let verify = clone_bare(_bare_dir.path());
    let remote_content =
        std::fs::read_to_string(verify.path().join("channels/general.thread")).unwrap();
    let file = parse_thread(&remote_content).unwrap();
    assert_eq!(file.messages().len(), 2);
    assert_eq!(file.messages()[0].body, "msg one");
    assert_eq!(file.messages()[1].body, "msg two");
}

// ---------------------------------------------------------------------------
// Test 4: Push conflict — daemon rebases and still returns "pushed"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_conflict_still_succeeds() {
    let (_bare_dir, _clone_dir, state) = setup_with_remote().await;

    // Create a second clone that will push a conflicting commit
    let rival = clone_bare(_bare_dir.path());
    run_git(rival.path(), &["config", "user.email", "rival@test.com"]);
    run_git(rival.path(), &["config", "user.name", "rival"]);

    // Rival writes a message directly to the thread and pushes
    let bob = Handler::new("bob").unwrap();
    let rival_msg = format_message(1, 0, &bob, "20260325T120000Z", "rival msg");
    std::fs::write(rival.path().join("channels/general.thread"), &rival_msg).unwrap();
    // Also create bob's user file so the thread content is valid
    std::fs::create_dir_all(rival.path().join("users")).ok();
    std::fs::write(
        rival.path().join("users/bob.meta.json"),
        r#"{"display_name":"Bob","role":"dev","introduction":"hi"}"#,
    )
    .unwrap();
    run_git(rival.path(), &["add", "."]);
    run_git(rival.path(), &["commit", "-m", "rival: bob msg"]);
    run_git(rival.path(), &["push"]);

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
        data["status"], "pushed",
        "should still push successfully after conflict resolution"
    );
    assert!(
        data["commit_id"].as_str().map_or(false, |s| !s.is_empty()),
        "commit_id should be non-empty"
    );

    // Verify remote has both messages (bob's original + alice's appended)
    let verify = clone_bare(_bare_dir.path());
    let remote_content =
        std::fs::read_to_string(verify.path().join("channels/general.thread")).unwrap();
    let file = parse_thread(&remote_content).unwrap();
    assert_eq!(
        file.messages().len(),
        2,
        "remote should have 2 messages (rival + alice)"
    );
    // Bob's message should be first (lower line_number)
    assert_eq!(file.messages()[0].author.as_str(), "bob");
    assert_eq!(file.messages()[0].body, "rival msg");
    // Alice's message should follow
    assert_eq!(file.messages()[1].author.as_str(), "alice");
    assert_eq!(file.messages()[1].body, "alice after conflict");
}
