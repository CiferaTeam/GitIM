#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::token_propagation::propagate_token;
use tempfile::TempDir;

fn write_config(workspace: &Path, cfg: WorkspaceConfig) {
    std::fs::create_dir_all(workspace.join(".gitim-runtime")).unwrap();
    cfg.write(workspace).unwrap();
}

// Fake git clone: `git init`, then stamp the remote URL in `.git/config` so
// `git config remote.origin.url` can read it back. Real pushes aren't needed
// by propagation itself — only `git config` runs against `.git/config`.
fn fake_clone(dir: &Path, remote_url: &str) {
    std::fs::create_dir_all(dir).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["remote", "add", "origin", remote_url])
        .current_dir(dir)
        .status()
        .unwrap()
        .success());
}

fn read_remote_url(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git config --get failed: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn github_config(token: &str) -> WorkspaceConfig {
    WorkspaceConfig {
        workspace: ".".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some("https://github.com/owner/repo".to_string()),
            token: Some(token.to_string()),
            github_email: None,
        },
    }
}

#[test]
fn propagate_token_updates_all_clones() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    write_config(workspace, github_config("new_token"));

    let old_url = "https://x-access-token:old_token@github.com/owner/repo.git";
    let human = workspace.join(".gitim-runtime").join("human");
    let agent_a = workspace.join("agent-a");
    let agent_b = workspace.join("agent-b");

    fake_clone(&human, old_url);
    fake_clone(&agent_a, old_url);
    fake_clone(&agent_b, old_url);

    propagate_token(workspace).unwrap();

    let want = "https://x-access-token:new_token@github.com/owner/repo.git";
    assert_eq!(read_remote_url(&human), want);
    assert_eq!(read_remote_url(&agent_a), want);
    assert_eq!(read_remote_url(&agent_b), want);
}

#[test]
fn propagate_token_skips_local_mode() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    let cfg = WorkspaceConfig {
        workspace: ".".to_string(),
        created_at: "2026-04-17T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    };
    write_config(workspace, cfg);

    let existing = "file:///tmp/some-local.git";
    let human = workspace.join(".gitim-runtime").join("human");
    let agent = workspace.join("agent-a");
    fake_clone(&human, existing);
    fake_clone(&agent, existing);

    propagate_token(workspace).unwrap();

    // Local mode: no-op — URLs untouched.
    assert_eq!(read_remote_url(&human), existing);
    assert_eq!(read_remote_url(&agent), existing);
}

#[test]
fn propagate_token_skips_missing_clone_directory() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    write_config(workspace, github_config("new_token"));

    let old_url = "https://x-access-token:old_token@github.com/owner/repo.git";
    let human = workspace.join(".gitim-runtime").join("human");
    let agent_a = workspace.join("agent-a");
    // agent-c path is never created; propagation must not crash on its absence.

    fake_clone(&human, old_url);
    fake_clone(&agent_a, old_url);

    propagate_token(workspace).unwrap();

    let want = "https://x-access-token:new_token@github.com/owner/repo.git";
    assert_eq!(read_remote_url(&human), want);
    assert_eq!(read_remote_url(&agent_a), want);
}

#[test]
fn propagate_token_skips_directories_without_git() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    write_config(workspace, github_config("new_token"));

    let old_url = "https://x-access-token:old_token@github.com/owner/repo.git";
    let human = workspace.join(".gitim-runtime").join("human");
    let agent_a = workspace.join("agent-a");
    let random = workspace.join("random-dir");

    fake_clone(&human, old_url);
    fake_clone(&agent_a, old_url);
    std::fs::create_dir_all(&random).unwrap();
    std::fs::write(random.join("note.txt"), "not a git dir").unwrap();

    propagate_token(workspace).unwrap();

    let want = "https://x-access-token:new_token@github.com/owner/repo.git";
    assert_eq!(read_remote_url(&human), want);
    assert_eq!(read_remote_url(&agent_a), want);
    assert!(random.join("note.txt").exists());
}
