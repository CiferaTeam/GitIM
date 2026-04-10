use std::path::PathBuf;
use std::process::Command;

use gitim_client::GitimClient;
use tempfile::{Builder, TempDir};

/// Ensure `gitim-daemon` binary is findable by adding target/debug to PATH.
pub fn ensure_daemon_in_path() {
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

/// Create a temp dir under /tmp to keep Unix socket paths under SUN_LEN (104 bytes on macOS).
pub fn short_tempdir() -> TempDir {
    Builder::new().prefix("gim").tempdir_in("/tmp").unwrap()
}

/// Create a bare git repo with an initial commit so clone works.
pub fn setup_bare_remote(tmp: &TempDir) -> PathBuf {
    let bare_path = tmp.path().join("remote.git");

    Command::new("git")
        .args(["init", "--bare", bare_path.to_str().unwrap()])
        .output()
        .unwrap();

    let init_clone = tmp.path().join("init-clone");
    Command::new("git")
        .args(["clone", bare_path.to_str().unwrap(), "init-clone"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

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

/// Stop daemon for a repo directory (best-effort cleanup).
pub async fn stop_daemon(repo_root: &std::path::Path) {
    let client = GitimClient::new(repo_root);
    let _ = client.stop().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}
