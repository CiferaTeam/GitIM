#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, AgentState, Poller};

use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};

#[tokio::test]
async fn test_state_save_and_load() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "state-agent".into(),
        display_name: "State Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };
    let handle = provision_agent(&agents_dir, &config, true).await.unwrap();

    // Initialize poller, get a cursor
    let mut poller = Poller::new(GitimClient::new(&handle.repo_root));
    poller.poll().await.unwrap();
    let cursor = poller.cursor().unwrap().to_string();

    // Save state
    let state = AgentState {
        cursor: Some(cursor.clone()),
        session_token: Some("test-session-123".into()),
        ..Default::default()
    };
    state.save(&handle.repo_root).unwrap();

    // Load state in a fresh context
    let loaded = AgentState::load(&handle.repo_root).unwrap();
    assert_eq!(loaded.cursor.as_deref(), Some(cursor.as_str()));
    assert_eq!(loaded.session_token.as_deref(), Some("test-session-123"));

    stop_daemon(&handle.repo_root).await;
}

#[tokio::test]
async fn test_cursor_restore_skips_old_messages() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "restore-agent".into(),
        display_name: "Restore Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };
    let handle = provision_agent(&agents_dir, &config, true).await.unwrap();
    let client = GitimClient::new(&handle.repo_root);

    // Phase 1: initialize poller, send a message, poll to detect it
    let mut poller1 = Poller::new(GitimClient::new(&handle.repo_root));
    poller1.poll().await.unwrap(); // init cursor

    client
        .send("general", "message before save", None, None)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let result = poller1.poll().await.unwrap();
    assert!(!result.changes.is_empty(), "should detect the message");

    // Save cursor
    let saved_cursor = poller1.cursor().unwrap().to_string();
    let state = AgentState {
        cursor: Some(saved_cursor.clone()),
        session_token: None,
        ..Default::default()
    };
    state.save(&handle.repo_root).unwrap();

    // Phase 2: create NEW poller with restored cursor — should NOT see old message
    let mut poller2 = Poller::with_cursor(GitimClient::new(&handle.repo_root), saved_cursor);
    let result = poller2.poll().await.unwrap();
    assert!(
        result.changes.is_empty(),
        "restored poller should not re-detect old messages"
    );

    stop_daemon(&handle.repo_root).await;
}

#[tokio::test]
async fn test_load_missing_state_returns_default() {
    let tmp = short_tempdir();
    std::fs::create_dir_all(tmp.path().join(".gitim")).unwrap();

    let state = AgentState::load(tmp.path()).unwrap();
    assert!(state.cursor.is_none());
    assert!(state.session_token.is_none());
}
