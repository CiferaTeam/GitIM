mod common;

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, Poller};

use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};

/// Provision an agent and return (repo_root, client) for test use.
async fn setup_agent(
    tmp: &tempfile::TempDir,
) -> (std::path::PathBuf, GitimClient) {
    let remote = setup_bare_remote(tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "poll-agent".into(),
        display_name: "Poll Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
    };

    let handle = provision_agent(&agents_dir, &config).await.unwrap();
    let client = GitimClient::new(&handle.repo_root);
    (handle.repo_root, client)
}

#[tokio::test]
async fn test_poll_init_and_detect() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // First poll: initialize cursor
    let _result = poller.poll().await.unwrap();
    assert!(poller.cursor().is_some(), "cursor should be initialized");
    // Note: first poll may return onboard-related channel_meta changes — that's OK.

    // Send a message
    let send_resp = client
        .send("general", "hello from test", None, None)
        .await
        .unwrap();
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);

    // Poll with retries — the sync loop may need a moment to push
    let mut detected = false;
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let result = poller.poll().await.unwrap();
        if !result.changes.is_empty() {
            let general_change = result.changes.iter().find(|c| c.channel == "general");
            assert!(
                general_change.is_some(),
                "should have a change for 'general' channel"
            );
            detected = true;
            break;
        }
    }
    assert!(detected, "should detect new message after send within 10 retries");

    stop_daemon(&repo_root).await;
}

#[tokio::test]
async fn test_poll_no_duplicates() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // Init cursor
    poller.poll().await.unwrap();

    // Send + wait for sync
    client
        .send("general", "message one", None, None)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // First poll after send: detect change
    let result = poller.poll().await.unwrap();
    assert!(!result.changes.is_empty(), "should detect the message");

    // Second poll: no new changes
    let result = poller.poll().await.unwrap();
    assert!(
        result.changes.is_empty(),
        "should not re-detect the same message"
    );

    stop_daemon(&repo_root).await;
}

#[tokio::test]
async fn test_poll_cursor_survives_empty() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, _client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // Init
    poller.poll().await.unwrap();
    let cursor1 = poller.cursor().unwrap().to_string();

    // Poll with no new messages
    let result = poller.poll().await.unwrap();
    assert!(result.changes.is_empty());
    let cursor2 = poller.cursor().unwrap().to_string();

    // Cursor should stay the same
    assert_eq!(cursor1, cursor2, "cursor should not change when no new messages");

    stop_daemon(&repo_root).await;
}
