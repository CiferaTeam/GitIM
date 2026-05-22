//! Shared `#[cfg(test)]` git fixtures. Provisions a bare origin + seeded
//! clone in a handful of lines so callers can race a remote forward,
//! diverge HEAD, or stage rebase-conflict scenarios without re-inlining
//! the same dozen `git` shell-outs in every test.

use std::path::Path;
use std::process::Command;

/// Bare origin + one clone with a seed commit pushed. The clone has
/// `user.name` / `user.email` configured to `user` / `email` so commits
/// from this clone are attributable in `git log` when a test inspects it.
pub fn seed_bare_with_clone(user: &str, email: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let bare = tempfile::TempDir::new().unwrap();
    let clone = tempfile::TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "--bare"])
        .current_dir(bare.path())
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "clone",
            bare.path().to_str().unwrap(),
            clone.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    configure_git_identity(clone.path(), user, email);
    std::fs::write(clone.path().join("seed.txt"), "seed").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(clone.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "seed"])
        .current_dir(clone.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["push", "-u", "origin", "HEAD"])
        .current_dir(clone.path())
        .output()
        .unwrap();
    (bare, clone)
}

/// Set `user.name` / `user.email` on a clone. Required because git refuses
/// to commit without identity and TempDir clones don't inherit the host's.
pub fn configure_git_identity(clone: &Path, user: &str, email: &str) {
    Command::new("git")
        .args(["config", "user.email", email])
        .current_dir(clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", user])
        .current_dir(clone)
        .output()
        .unwrap();
}

/// Write `content` to `clone/name`, stage everything, commit with `msg`.
pub fn commit_file(clone: &Path, name: &str, content: &str, msg: &str) {
    std::fs::write(clone.join(name), content).unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(clone)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", msg])
        .current_dir(clone)
        .output()
        .unwrap();
}

/// Spin up a throwaway helper clone of `bare`, add `count` commits, push.
/// Used to simulate a second clone racing the receiving clone forward
/// without polluting the receiver's working tree.
pub fn push_n_commits_to_bare(bare: &Path, count: u64) {
    let helper = tempfile::TempDir::new().unwrap();
    Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            helper.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    configure_git_identity(helper.path(), "H", "h@test.com");
    for i in 1..=count {
        commit_file(helper.path(), &format!("h-{i}.txt"), "x", &format!("H {i}"));
    }
    Command::new("git")
        .args(["push"])
        .current_dir(helper.path())
        .output()
        .unwrap();
}
