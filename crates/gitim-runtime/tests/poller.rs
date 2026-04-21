mod common;

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, Poller};
use serial_test::serial;

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
        github_email: None,
    };

    let handle = provision_agent(&agents_dir, &config).await.unwrap();
    let client = GitimClient::new(&handle.repo_root);
    (handle.repo_root, client)
}

#[tokio::test]
#[serial]
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
        // Look for a real message change (kind=channel + non-empty entries),
        // not just onboard channel_meta diffs.
        let msg_change = result.changes.iter().find(|c| {
            c.channel == "general" && c.kind == "channel" && !c.entries.is_empty()
        });
        if msg_change.is_some() {
            detected = true;
            break;
        }
    }
    assert!(detected, "should detect new message after send within 10 retries");

    stop_daemon(&repo_root).await;
}

#[tokio::test]
#[serial]
async fn test_poll_no_duplicates() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // Init cursor
    poller.poll().await.unwrap();

    // Send a message
    client
        .send("general", "message one", None, None)
        .await
        .unwrap();

    // Poll with retries until the message is detected
    let mut detected = false;
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let result = poller.poll().await.unwrap();
        let msg_change = result.changes.iter().find(|c| {
            c.channel == "general" && c.kind == "channel" && !c.entries.is_empty()
        });
        if msg_change.is_some() {
            detected = true;
            break;
        }
    }
    assert!(detected, "should detect the message");

    // Second poll: no new changes
    let result = poller.poll().await.unwrap();
    assert!(
        result.changes.is_empty(),
        "should not re-detect the same message"
    );

    stop_daemon(&repo_root).await;
}

#[tokio::test]
#[serial]
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

#[tokio::test]
async fn test_peek_does_not_advance_cursor() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // Init cursor
    poller.poll().await.unwrap();
    let cursor_before = poller.cursor().unwrap().to_string();

    // Send a message
    client
        .send("general", "peek test message", None, None)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Peek: should see the message
    let peek_result = poller.peek().await.unwrap();
    assert!(!peek_result.changes.is_empty(), "peek should detect new message");

    // Cursor should NOT have advanced
    let cursor_after = poller.cursor().unwrap().to_string();
    assert_eq!(cursor_before, cursor_after, "peek must not advance cursor");

    // Poll: should also see the same message (cursor didn't move)
    let poll_result = poller.poll().await.unwrap();
    assert!(!poll_result.changes.is_empty(), "poll should still get the message");

    // Now cursor has advanced
    let cursor_final = poller.cursor().unwrap().to_string();
    assert_ne!(cursor_before, cursor_final, "poll should advance cursor");

    stop_daemon(&repo_root).await;
}
