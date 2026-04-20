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

    let handle = provision_agent(&agents_dir, &config).await.unwrap();

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

    let handle1 = provision_agent(&agents_dir, &config).await.unwrap();
    stop_daemon(&handle1.repo_root).await;

    let handle2 = provision_agent(&agents_dir, &config).await.unwrap();
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

    let result = provision_agent(&agents_dir, &config).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), RuntimeError::GitCloneFailed(_)));
}
