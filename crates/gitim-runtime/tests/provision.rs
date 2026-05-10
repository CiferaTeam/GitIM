mod common;

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, RuntimeError};

use common::{ensure_daemon_in_path, setup_bare_remote, short_tempdir, stop_daemon};

#[tokio::test]
async fn test_provision_fresh() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "test-agent".into(),
        display_name: "Test Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };

    let handle = provision_agent(&agents_dir, &config, true).await.unwrap();

    assert_eq!(handle.handler, "test-agent");
    assert!(handle.repo_root.exists());
    assert!(handle.repo_root.join(".gitim").exists());

    let client = GitimClient::new(&handle.repo_root);
    let status = client.status().await.unwrap();
    assert!(status.ok);

    stop_daemon(&handle.repo_root).await;
}

#[tokio::test]
async fn test_provision_idempotent() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "idem-agent".into(),
        display_name: "Idempotent Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };

    let handle1 = provision_agent(&agents_dir, &config, true).await.unwrap();
    stop_daemon(&handle1.repo_root).await;

    let handle2 = provision_agent(&agents_dir, &config, true).await.unwrap();
    assert_eq!(handle1.repo_root, handle2.repo_root);

    let client = GitimClient::new(&handle2.repo_root);
    let status = client.status().await.unwrap();
    assert!(status.ok);

    stop_daemon(&handle2.repo_root).await;
}

#[tokio::test]
async fn test_provision_invalid_remote() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "bad-agent".into(),
        display_name: "Bad Agent".into(),
        remote_url: "/nonexistent/repo.git".into(),
        github_email: None,
    };

    let result = provision_agent(&agents_dir, &config, true).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        RuntimeError::GitCloneFailed(_)
    ));
}

/// Reverse coverage at the runtime layer for the `join_general` toggle.
/// Provision bot-a normally so general exists with bot-a in members, then
/// provision bot-b with `join_general=false` and assert bot-b is NOT a
/// member of `channels/general.meta.yaml` in bot-b's clone.
#[tokio::test]
async fn test_provision_skip_general_when_join_general_false() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config_a = AgentConfig {
        handler: "bot-a".into(),
        display_name: "Bot A".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };
    let handle_a = provision_agent(&agents_dir, &config_a, true).await.unwrap();

    let config_b = AgentConfig {
        handler: "bot-b".into(),
        display_name: "Bot B".into(),
        remote_url: remote.to_str().unwrap().into(),
        github_email: None,
    };
    let handle_b = provision_agent(&agents_dir, &config_b, false)
        .await
        .unwrap();

    let meta_path = handle_b.repo_root.join("channels/general.meta.yaml");
    let meta_content = std::fs::read_to_string(&meta_path)
        .expect("bot-b should have general.meta.yaml from bot-a's earlier push");
    let meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(&meta_content).unwrap();

    assert!(
        meta.members.contains(&"bot-a".to_string()),
        "bot-a should be a member (join_general=true)"
    );
    assert!(
        !meta.members.contains(&"bot-b".to_string()),
        "bot-b must NOT be a member when join_general=false, got: {:?}",
        meta.members
    );

    stop_daemon(&handle_a.repo_root).await;
    stop_daemon(&handle_b.repo_root).await;
}
