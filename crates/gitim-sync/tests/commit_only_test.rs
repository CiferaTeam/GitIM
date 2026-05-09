use std::fs;
use std::path::Path;
use std::process::Command;

use gitim_sync::git::GitStorage;
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git command failed");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn add_and_commit_only_as_ignores_unrelated_staged_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init"]);
    git(root, &["config", "user.name", "test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    fs::write(root.join("board.md"), "v1\n").unwrap();
    fs::write(root.join("other.txt"), "v1\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);

    fs::write(root.join("board.md"), "v2\n").unwrap();
    fs::write(root.join("other.txt"), "v2\n").unwrap();
    git(root, &["add", "other.txt"]);

    let storage = GitStorage::new(root);
    let head = storage
        .add_and_commit_only_as(
            "board.md",
            "board: update @alice",
            Some(("alice", "alice@gitim")),
        )
        .unwrap();

    assert_eq!(head, git(root, &["rev-parse", "HEAD"]).trim());

    let committed_files = git(root, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(committed_files.trim(), "board.md");

    let staged_files = git(root, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged_files.trim(), "other.txt");
}
