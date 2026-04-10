use std::path::PathBuf;
use std::process::Command;

use gitim_client::GitimClient;
use gitim_runtime::{provision_agent, AgentConfig, RuntimeError};
use tempfile::{Builder, TempDir};

/// Ensure `gitim-daemon` binary is findable by adding target/debug to PATH.
fn ensure_daemon_in_path() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = PathBuf::from(manifest_dir).join("../..");
    let target_debug = workspace_root.join("target/debug").canonicalize().unwrap();

    let current_path = std::env::var("PATH").unwrap_or_default();
    if !current_path.contains(target_debug.to_str().unwrap()) {
        std::env::set_var(
            "PATH",
            format!("{}:{}", target_debug.display(), current_path),
        );
    }
}

/// Create a bare git repo with an initial commit so clone works.
fn setup_bare_remote(tmp: &TempDir) -> PathBuf {
    let bare_path = tmp.path().join("remote.git");

    // Init bare repo
    Command::new("git")
        .args(["init", "--bare", bare_path.to_str().unwrap()])
        .output()
        .unwrap();

    // Create a temporary clone to make the initial commit
    let init_clone = tmp.path().join("init-clone");
    Command::new("git")
        .args(["clone", bare_path.to_str().unwrap(), "init-clone"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    // Configure git user for the init clone
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&init_clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&init_clone)
        .output()
        .unwrap();

    // Create initial commit
    std::fs::write(init_clone.join(".gitkeep"), "").unwrap();
    Command::new("git")
        .args(["add", ".gitkeep"])
        .current_dir(&init_clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(&init_clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["push"])
        .current_dir(&init_clone)
        .output()
        .unwrap();

    bare_path
}

/// Stop daemon for an agent directory (best-effort cleanup).
async fn stop_daemon(repo_root: &std::path::Path) {
    let client = GitimClient::new(repo_root);
    let _ = client.stop().await;
    // Give daemon time to shut down
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

#[tokio::test]
async fn test_provision_fresh() {
    ensure_daemon_in_path();
    // Use /tmp to keep Unix socket paths under SUN_LEN (104 bytes on macOS)
    let tmp = Builder::new().prefix("gim").tempdir_in("/tmp").unwrap();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "test-agent".into(),
        display_name: "Test Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
    };

    let handle = provision_agent(&agents_dir, &config).await.unwrap();

    assert_eq!(handle.handler, "test-agent");
    assert!(handle.repo_root.exists());
    assert!(handle.repo_root.join(".gitim").exists());

    // Verify daemon is responsive
    let client = GitimClient::new(&handle.repo_root);
    let status = client.status().await.unwrap();
    assert!(status.ok);

    stop_daemon(&handle.repo_root).await;
}

#[tokio::test]
async fn test_provision_idempotent() {
    ensure_daemon_in_path();
    // Use /tmp to keep Unix socket paths under SUN_LEN (104 bytes on macOS)
    let tmp = Builder::new().prefix("gim").tempdir_in("/tmp").unwrap();
    let remote = setup_bare_remote(&tmp);
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "idem-agent".into(),
        display_name: "Idempotent Agent".into(),
        remote_url: remote.to_str().unwrap().into(),
    };

    // First provision
    let handle1 = provision_agent(&agents_dir, &config).await.unwrap();
    stop_daemon(&handle1.repo_root).await;

    // Second provision — should succeed without cloning again
    let handle2 = provision_agent(&agents_dir, &config).await.unwrap();
    assert_eq!(handle1.repo_root, handle2.repo_root);

    // Verify daemon is responsive after re-provision
    let client = GitimClient::new(&handle2.repo_root);
    let status = client.status().await.unwrap();
    assert!(status.ok);

    stop_daemon(&handle2.repo_root).await;
}

#[tokio::test]
async fn test_provision_invalid_remote() {
    ensure_daemon_in_path();
    // Use /tmp to keep Unix socket paths under SUN_LEN (104 bytes on macOS)
    let tmp = Builder::new().prefix("gim").tempdir_in("/tmp").unwrap();
    let agents_dir = tmp.path().join("agents");
    std::fs::create_dir(&agents_dir).unwrap();

    let config = AgentConfig {
        handler: "bad-agent".into(),
        display_name: "Bad Agent".into(),
        remote_url: "/nonexistent/repo.git".into(),
    };

    let result = provision_agent(&agents_dir, &config).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), RuntimeError::GitCloneFailed(_)));
}
