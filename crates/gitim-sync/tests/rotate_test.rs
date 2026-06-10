#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_sync::git::GitStorage;
use gitim_sync::rotate::{try_fire_rotation, RotationOutcome};
use std::process::Command;

// === helpers (shared by later tasks in this file) ===
fn git(dir: &tempfile::TempDir, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(dir.path())
        .status()
        .unwrap()
        .success());
}
fn commit_file(dir: &tempfile::TempDir, name: &str, content: &str) {
    std::fs::write(dir.path().join(name), content).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", name]);
}
fn setup_bare_and_clone(n_commits: usize) -> (tempfile::TempDir, tempfile::TempDir) {
    let bare = tempfile::TempDir::new().unwrap();
    let clone = tempfile::TempDir::new().unwrap();
    git(&bare, &["init", "--bare", "-b", "main"]);
    git(&clone, &["clone", bare.path().to_str().unwrap(), "."]);
    git(&clone, &["config", "user.email", "t@t"]);
    git(&clone, &["config", "user.name", "t"]);
    for i in 0..n_commits {
        commit_file(&clone, &format!("f{i}.txt"), &format!("c{i}"));
    }
    git(&clone, &["push", "-u", "origin", "main"]);
    (bare, clone)
}
fn clone_from(bare: &tempfile::TempDir) -> tempfile::TempDir {
    let c = tempfile::TempDir::new().unwrap();
    git(&c, &["clone", bare.path().to_str().unwrap(), "."]);
    git(&c, &["config", "user.email", "t@t"]);
    git(&c, &["config", "user.name", "t"]);
    c
}
fn head_branch(dir: &tempfile::TempDir) -> String {
    let out = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn under_threshold_returns_not_ready() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(
        &storage,
        "main",
        100,
        arch.path(),
        ("d", "d@g"),
        "2026-06-10T00:00:00Z",
    )
    .unwrap();
    assert!(matches!(o, RotationOutcome::NotReady));
}

#[test]
fn solo_fire_wins_switches_branch_tags_and_bundles() {
    let (_bare, clone) = setup_bare_and_clone(5);
    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(
        &storage,
        "main",
        3,
        arch.path(),
        ("d", "d@g"),
        "2026-06-10T00:00:00Z",
    )
    .unwrap();
    let RotationOutcome::Won {
        new_branch,
        new_epoch,
        sealed_branch,
        ..
    } = o
    else {
        panic!("expected Won, got {o:?}");
    };
    assert_eq!(
        (sealed_branch.as_str(), new_branch.as_str(), new_epoch),
        ("main", "main-epoch-2", 2)
    );
    assert_eq!(head_branch(&clone), "main-epoch-2");
    let yaml = std::fs::read_to_string(clone.path().join("gitim.epoch.yaml")).unwrap();
    assert!(yaml.contains("status: active") && yaml.contains("epoch: 2"));
    assert!(arch.path().join("epoch-1.bundle").exists());
}

#[test]
fn fire_with_unpushed_backlog_returns_not_ready() {
    // Zero-loss guard I3: messages committed between push-success and lock
    // acquisition must defer rotation — a Lost reset would destroy them.
    let (_bare, clone) = setup_bare_and_clone(5);
    commit_file(
        &clone,
        "inflight.thread",
        "[L1][@x][t] committed but not pushed",
    );

    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(
        &storage,
        "main",
        3,
        arch.path(),
        ("d", "d@g"),
        "2026-06-10T00:00:00Z",
    )
    .unwrap();
    assert!(matches!(o, RotationOutcome::NotReady), "got {o:?}");
    assert!(
        clone.path().join("inflight.thread").exists(),
        "backlog must survive"
    );
    assert_eq!(head_branch(&clone), "main");
}

#[test]
fn fire_loses_to_normal_push_cleans_up_and_self_heals() {
    // Design scenario 2: someone pushes a plain message while we fire →
    // atomic reject → local cleanup leaves no trace, origin has no rotation.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);

    commit_file(&clone_b, "msg.txt", "normal write wins");
    git(&clone_b, &["push", "origin", "main"]);

    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(
        &storage_a,
        "main",
        3,
        arch.path(),
        ("a", "a@g"),
        "2026-06-10T00:00:00Z",
    )
    .unwrap();
    assert!(matches!(o, RotationOutcome::Lost), "got {o:?}");

    assert_eq!(head_branch(&clone_a), "main");
    assert!(!clone_a.path().join("gitim.epoch.yaml").exists());
    let out = Command::new("git")
        .args(["branch", "-l", "main-epoch-2"])
        .current_dir(clone_a.path())
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stale orphan branch must be deleted");
    let local = storage_a.rev_parse("main").unwrap();
    let remote = storage_a.rev_parse("origin/main").unwrap();
    assert_eq!(local, remote, "local main must be reset to origin");
}

#[test]
fn cleanup_refuses_when_foreign_commits_ahead() {
    // Zero-loss guard I3: foreign commits ahead of origin → no reset.
    let (_bare, clone) = setup_bare_and_clone(3);
    commit_file(&clone, "user-msg.thread", "[L1][@x][t] precious");
    let storage = GitStorage::new(clone.path());

    gitim_sync::rotate::cleanup_failed_fire(&storage, "main", "main-epoch-2").unwrap();
    assert!(
        clone.path().join("user-msg.thread").exists(),
        "foreign commit must not be reset away"
    );
}
