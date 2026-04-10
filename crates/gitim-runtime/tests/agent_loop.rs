mod common;

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, AgentLoop, Poller};

use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};

/// End-to-end test: send message → agent detects → claude processes → agent replies.
/// Requires `claude` CLI and `gitim` CLI in PATH.
/// Run with: cargo test -p gitim-runtime --test agent_loop -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_agent_loop_end_to_end() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    // Provision agent
    let config = AgentConfig {
        handler: "loop-agent".into(),
        display_name: "Loop Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
    };
    let handle = provision_agent(&agents_dir, &config).await.unwrap();
    let client = GitimClient::new(&handle.repo_root);
    eprintln!("[setup] agent provisioned at {}", handle.repo_root.display());

    // Create agent loop
    let poller = Poller::new(GitimClient::new(&handle.repo_root));
    let mut agent_loop = AgentLoop::with_defaults(poller, &handle.repo_root);

    // Initialize cursor (first poll)
    let processed = agent_loop.run_once().await.unwrap();
    assert!(!processed, "first run should have no messages");
    eprintln!("[setup] cursor initialized");

    // Send a trigger message (as the agent itself — just validating the pipeline)
    let send_resp = client
        .send("general", "This is a test. Please reply with: test-reply-ok", None, None)
        .await
        .unwrap();
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);
    eprintln!("[trigger] sent message to general");

    // Wait for sync loop to push
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Agent loop processes the message
    let processed = agent_loop.run_once().await.unwrap();
    assert!(processed, "should have detected and processed the message");
    eprintln!("[agent] processed message via claude");

    // Wait for agent's gitim send to commit + sync
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Read general channel to verify agent replied
    let read_resp = client.read("general", Some(20), None).await.unwrap();
    assert!(read_resp.ok, "read failed: {:?}", read_resp.error);

    let entries = read_resp.data.unwrap();
    let messages = entries["entries"].as_array().unwrap();
    eprintln!("[verify] {} messages in general:", messages.len());
    for msg in messages {
        let author = msg["author"].as_str().unwrap_or("?");
        let body = msg["body"].as_str().unwrap_or("?");
        eprintln!("  @{}: {}", author, body);
    }

    // Should have more messages than just the trigger (claude added a reply via gitim send)
    assert!(
        messages.len() >= 2,
        "expected at least 2 messages (trigger + agent reply), got {}",
        messages.len()
    );
    eprintln!("[verify] pipeline validated: {} messages total", messages.len());

    stop_daemon(&handle.repo_root).await;
}
