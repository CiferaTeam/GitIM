//! Cover the "clone empty GitHub repo" onboarding path.
//!
//! Scenario: a brand-new remote has no HEAD yet. A clone lands with an unborn
//! branch; the first local commit creates the branch. Prior to the upstream
//! fix, the first `push` succeeded but never linked `origin/main` as upstream,
//! so the next `has_unpushed_commits` call errored on `@{upstream}`, which the
//! sync loop swallowed as a warn — messages then silently stopped syncing.
//!
//! These tests lock in that `GitStorage::push` establishes upstream on the
//! first push and that subsequent `has_unpushed_commits` / `push` operate
//! normally.

use std::path::Path;
use std::process::Command;

use gitim_sync::git::GitStorage;
use tempfile::TempDir;

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run git");
    if !output.status.success() {
        panic!(
            "git {:?} failed in {}: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn setup_git_config(dir: &Path, name: &str, email: &str) {
    run_git(dir, &["config", "user.email", email]);
    run_git(dir, &["config", "user.name", name]);
    // Neutralize dev-box git config that could mask the bug: recent git
    // versions / local overrides may auto-wire upstream on plain `git push`,
    // which hides the empty-remote regression. Force the strict path.
    run_git(dir, &["config", "push.default", "nothing"]);
    run_git(dir, &["config", "push.autoSetupRemote", "false"]);
}

fn clone_empty_remote(bare: &Path, into: &Path) {
    let output = Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            into.to_str().unwrap(),
        ])
        .current_dir(into.parent().unwrap())
        .output()
        .expect("clone failed to spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("empty repository") || output.status.success(),
        "clone of empty bare repo should succeed (stderr: {})",
        stderr
    );
}

fn write_initial_layout(root: &Path) -> Vec<String> {
    std::fs::write(root.join(".gitignore"), ".gitim/\n").unwrap();
    let channels = root.join("channels");
    std::fs::create_dir_all(&channels).unwrap();
    std::fs::write(
        channels.join("general.meta.yaml"),
        "display_name: General\ncreated_by: alice\ncreated_at: 20260417T000000Z\nintroduction: 默认频道\n",
    )
    .unwrap();
    std::fs::write(channels.join("general.thread"), "").unwrap();
    vec![
        ".gitignore".into(),
        "channels/general.meta.yaml".into(),
        "channels/general.thread".into(),
    ]
}

fn fresh_clone(bare: &Path) -> TempDir {
    let verify = TempDir::new().unwrap();
    run_git(
        verify.path().parent().unwrap(),
        &[
            "clone",
            bare.to_str().unwrap(),
            verify.path().to_str().unwrap(),
        ],
    );
    verify
}

/// Core regression: onboard → first push creates the remote branch + upstream,
/// a follow-up commit's unpushed check succeeds, and its push lands cleanly.
/// Before the fix, step 4 here returned `Err` and sync_loop would stall.
#[test]
fn empty_remote_first_push_wires_upstream_and_unblocks_followups() {
    let bare = TempDir::new().unwrap();
    run_git(bare.path(), &["init", "--bare"]);

    let clone_dir = TempDir::new().unwrap();
    clone_empty_remote(bare.path(), clone_dir.path());
    setup_git_config(clone_dir.path(), "Alice", "alice@test.com");

    let repo = GitStorage::new(clone_dir.path());

    let initial_paths = write_initial_layout(clone_dir.path());
    let initial_refs: Vec<&str> = initial_paths.iter().map(String::as_str).collect();
    repo.add_and_commit(&initial_refs, "init: repo structure").unwrap();

    repo.push().expect("first push should seed upstream + remote branch");

    let upstream = Command::new("git")
        .args(["rev-parse", "--symbolic-full-name", "@{upstream}"])
        .current_dir(clone_dir.path())
        .output()
        .unwrap();
    assert!(
        upstream.status.success(),
        "@{{upstream}} must resolve after first push: {}",
        String::from_utf8_lossy(&upstream.stderr)
    );

    assert!(
        !repo.has_unpushed_commits().unwrap(),
        "no unpushed commits after the seeding push"
    );

    let thread = clone_dir.path().join("channels/general.thread");
    std::fs::write(&thread, "[L1][P0][@alice][20260417T000100Z] hello\n").unwrap();
    repo.add_and_commit(&["channels/general.thread"], "msg: @alice L1").unwrap();

    assert!(
        repo.has_unpushed_commits().unwrap(),
        "second commit must be detected as unpushed (this is what regressed)"
    );

    repo.push().expect("second push should succeed via existing upstream");

    let verify = fresh_clone(bare.path());
    let remote_thread =
        std::fs::read_to_string(verify.path().join("channels/general.thread")).unwrap();
    assert!(
        remote_thread.contains("@alice") && remote_thread.contains("hello"),
        "remote must contain the second message, got: {:?}",
        remote_thread
    );
}

/// Idempotence check: `-u` on push must not break repos that already have an
/// upstream configured (normal steady-state case). Pushes an empty-delta
/// commit to confirm `push` still behaves when upstream is already wired.
#[test]
fn push_with_upstream_already_set_is_idempotent() {
    let bare = TempDir::new().unwrap();
    run_git(bare.path(), &["init", "--bare"]);

    let clone_dir = TempDir::new().unwrap();
    clone_empty_remote(bare.path(), clone_dir.path());
    setup_git_config(clone_dir.path(), "Alice", "alice@test.com");

    let repo = GitStorage::new(clone_dir.path());
    let paths = write_initial_layout(clone_dir.path());
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    repo.add_and_commit(&refs, "init").unwrap();
    repo.push().unwrap();

    std::fs::write(
        clone_dir.path().join("channels/general.thread"),
        "[L1][P0][@alice][20260417T000000Z] first\n",
    )
    .unwrap();
    repo.add_and_commit(&["channels/general.thread"], "msg 1").unwrap();
    repo.push().unwrap();

    std::fs::write(
        clone_dir.path().join("channels/general.thread"),
        "[L1][P0][@alice][20260417T000000Z] first\n[L2][P0][@alice][20260417T000100Z] second\n",
    )
    .unwrap();
    repo.add_and_commit(&["channels/general.thread"], "msg 2").unwrap();
    repo.push().expect("repeated push with existing upstream should succeed");

    assert!(!repo.has_unpushed_commits().unwrap());
}
