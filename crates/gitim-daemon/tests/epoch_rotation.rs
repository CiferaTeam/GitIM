#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Daemon-level epoch rotation wiring (Phase B v2 Task 8): the same code
//! path the `on_pushed` hook runs fires a rotation once the commit count
//! crosses the threshold, and the daemon-side state refresh follows.
//!
//! Threshold is passed explicitly through `attempt_rotation_for_test` —
//! no env vars (cargo test is multi-threaded; `set_var` races).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use gitim_core::types::Config;
use gitim_daemon::state::AppState;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn git(dir: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success(),
        "git {args:?} failed in {dir:?}"
    );
}

fn commit_file(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", name]);
}

fn setup_bare_and_clone(n_commits: usize) -> (TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let clone = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-b", "main"]);
    git(clone.path(), &["clone", bare.path().to_str().unwrap(), "."]);
    git(clone.path(), &["config", "user.email", "d@d"]);
    git(clone.path(), &["config", "user.name", "d"]);
    for i in 0..n_commits {
        commit_file(clone.path(), &format!("f{i}.txt"), &format!("c{i}"));
    }
    git(clone.path(), &["push", "-u", "origin", "main"]);
    (bare, clone)
}

fn make_state(repo_root: &Path) -> Arc<AppState> {
    let (event_tx, _) = broadcast::channel(16);
    Arc::new(AppState::new(
        repo_root.to_path_buf(),
        Config::default(),
        event_tx,
        None,
    ))
}

#[tokio::test]
async fn daemon_auto_rotates_when_threshold_crossed() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let state = make_state(clone.path());

    let st = state.clone();
    let fired = tokio::task::spawn_blocking(move || st.attempt_rotation_for_test(3))
        .await
        .expect("join blocking task")
        .expect("attempt_rotation_for_test");
    assert!(fired, "rotation should have fired");

    let head = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(clone.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main-epoch-2");

    // Daemon-visible epoch state refreshed: new branch is active.
    let snap = state
        .epoch_status_snapshot()
        .expect("epoch snapshot after rotation");
    assert_eq!(snap.epoch, 2);
    assert!(!state.is_redirected());
}

#[tokio::test]
async fn daemon_rotation_not_ready_under_threshold() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let state = make_state(clone.path());

    let st = state.clone();
    let fired = tokio::task::spawn_blocking(move || st.attempt_rotation_for_test(1000))
        .await
        .expect("join blocking task")
        .expect("attempt_rotation_for_test");
    assert!(!fired);

    let head = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(clone.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main");
}

#[tokio::test]
async fn rotation_check_throttle_dedupes_rapid_checks() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let state = make_state(clone.path());

    // First due-check passes, immediate second is throttled.
    assert!(state.rotation_check_due());
    assert!(!state.rotation_check_due());
}
