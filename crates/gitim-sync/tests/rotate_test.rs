#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_sync::git::GitStorage;
use gitim_sync::rotate::{
    check_push_fence, follow_redirect, try_fire_rotation as try_fire_rotation_impl, RotationError,
    RotationOutcome,
};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod support;
use support::TestWorkingBranchPush;

fn try_fire_rotation(
    storage: &GitStorage,
    current_branch: &str,
    threshold: u64,
    archive_dir: &std::path::Path,
    author: (&str, &str),
    created_at: &str,
) -> Result<RotationOutcome, gitim_sync::rotate::RotationError> {
    try_fire_rotation_impl(
        storage,
        &std::sync::Mutex::new(()),
        current_branch,
        threshold,
        archive_dir,
        author,
        created_at,
    )
}

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
    git(&clone, &["config", "commit.gpgsign", "false"]);
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
    git(&c, &["config", "commit.gpgsign", "false"]);
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
/// Configured upstream of `branch` ("" when none) — sync_loop's cycle top
/// probes `@{upstream}` and bails the whole cycle when it doesn't resolve,
/// so every epoch-branch switch must leave upstream set to stay publishable.
fn upstream_of(dir: &tempfile::TempDir, branch: &str) -> String {
    let spec = format!("{branch}@{{upstream}}");
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", &spec])
        .current_dir(dir.path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn install_receive_pack_race(clone: &tempfile::TempDir, race_command: &str) -> std::path::PathBuf {
    let script = clone.path().join(".git/test-receive-pack-race");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nset -eu\n{race_command}\nexec git-receive-pack \"$@\"\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(
        clone,
        &[
            "config",
            "remote.origin.receivepack",
            script.to_str().unwrap(),
        ],
    );
    script
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
fn epoch_publication_rejects_a_remote_rewind_after_validation() {
    let (bare, clone) = setup_bare_and_clone(5);
    let storage = GitStorage::new(clone.path());
    let accepted = storage.rev_parse("origin/main").unwrap();
    let rewound = storage.rev_parse("origin/main^").unwrap();
    install_receive_pack_race(
        &clone,
        &format!(
            "git --git-dir='{}' update-ref refs/heads/main '{}' '{}'",
            bare.path().display(),
            rewound,
            accepted
        ),
    );
    let archive = tempfile::TempDir::new().unwrap();

    let outcome = try_fire_rotation(
        &storage,
        "main",
        3,
        archive.path(),
        ("d", "d@g"),
        "2026-07-31T04:00:00Z",
    );

    assert!(
        matches!(outcome, Ok(RotationOutcome::Lost) | Err(_)),
        "epoch publication unexpectedly won: {outcome:?}"
    );
    assert_eq!(
        GitStorage::new(bare.path()).rev_parse("main").unwrap(),
        rewound
    );
    assert!(GitStorage::new(bare.path())
        .rev_parse("main-epoch-2")
        .is_err());
}

#[test]
fn fire_rejects_a_handler_commit_after_its_network_snapshot() {
    let (bare, clone) = setup_bare_and_clone(5);
    let storage = GitStorage::new(clone.path());
    let captured_origin = storage.rev_parse("origin/main").unwrap();
    let commit_lock = Arc::new(Mutex::new(()));
    let held = commit_lock.lock().unwrap();
    let worker_lock = commit_lock.clone();
    let worker_root = clone.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let archive = tempfile::TempDir::new().unwrap();
        try_fire_rotation_impl(
            &GitStorage::new(&worker_root),
            &worker_lock,
            "main",
            3,
            archive.path(),
            ("d", "d@g"),
            "2026-06-10T00:00:00Z",
        )
    });

    let checkpoint = clone.path().join(".gitim/skill-validation.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::fs::read_to_string(&checkpoint)
            .is_ok_and(|contents| contents.contains(&captured_origin))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        std::fs::read_to_string(&checkpoint)
            .unwrap_or_default()
            .contains(&captured_origin),
        "rotation must complete network validation before the handler race"
    );

    commit_file(
        &clone,
        "handler.thread",
        "handler commit after rotation capture",
    );
    let handler_head = storage.rev_parse("HEAD").unwrap();
    drop(held);

    let error = worker.join().unwrap().unwrap_err();
    assert!(matches!(
        error,
        RotationError::Git(gitim_sync::git::GitError::PushConflict)
    ));
    assert_eq!(head_branch(&clone), "main");
    assert_eq!(storage.rev_parse("HEAD").unwrap(), handler_head);
    assert!(!clone.path().join("gitim.epoch.yaml").exists());
    assert!(
        Command::new("git")
            .args(["show-ref", "--verify", "--quiet", "refs/heads/main-epoch-2"])
            .current_dir(clone.path())
            .status()
            .is_ok_and(|status| !status.success()),
        "CAS rejection must not leave a local orphan branch"
    );
    assert!(
        Command::new("git")
            .args([
                "--git-dir",
                bare.path().to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/main-epoch-2",
            ])
            .status()
            .is_ok_and(|status| !status.success()),
        "CAS rejection must not publish a partial epoch"
    );
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
    assert_eq!(
        upstream_of(&clone, "main-epoch-2"),
        "origin/main-epoch-2",
        "won fire must leave the new branch publishable"
    );
    let yaml = std::fs::read_to_string(clone.path().join("gitim.epoch.yaml")).unwrap();
    assert!(yaml.contains("status: active") && yaml.contains("epoch: 2"));
    assert!(arch.path().join("epoch-1.bundle").exists());
}

#[test]
fn won_fire_preserves_a_branch_switched_by_the_atomic_push_hook() {
    let (_bare, clone) = setup_bare_and_clone(5);
    git(&clone, &["checkout", "-b", "unrelated"]);
    commit_file(&clone, "unrelated.txt", "keep unrelated bytes");
    let unrelated_head = GitStorage::new(clone.path()).rev_parse("HEAD").unwrap();
    let unrelated_bytes = std::fs::read(clone.path().join("unrelated.txt")).unwrap();
    git(&clone, &["checkout", "main"]);

    let marker = clone.path().join(".git/post-atomic-switch");
    let hook = clone.path().join(".git/hooks/pre-push");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nif [ ! -f '{}' ]; then\n  touch '{}'\n  git -C '{}' checkout -f unrelated\nfi\n",
            marker.display(),
            marker.display(),
            clone.path().display(),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let outcome = try_fire_rotation(
        &storage,
        "main",
        3,
        arch.path(),
        ("d", "d@g"),
        "2026-07-31T01:50:00Z",
    )
    .unwrap();

    assert!(matches!(outcome, RotationOutcome::Won { .. }));
    assert_eq!(head_branch(&clone), "unrelated");
    assert_eq!(storage.rev_parse("HEAD").unwrap(), unrelated_head);
    assert_eq!(
        std::fs::read(clone.path().join("unrelated.txt")).unwrap(),
        unrelated_bytes
    );
    assert_eq!(
        storage.rev_parse("main").unwrap(),
        storage.rev_parse("origin/main").unwrap()
    );
    assert_eq!(
        storage.rev_parse("main-epoch-2").unwrap(),
        storage.rev_parse("origin/main-epoch-2").unwrap()
    );

    git(&clone, &["checkout", "main"]);
    assert!(follow_redirect(&storage, "main").unwrap());
    assert_eq!(head_branch(&clone), "main-epoch-2");
}

#[test]
fn won_fire_preserves_dirty_tracked_bytes_written_during_atomic_push() {
    let (_bare, clone) = setup_bare_and_clone(5);
    let marker = clone.path().join(".git/post-atomic-dirty");
    let hook = clone.path().join(".git/hooks/pre-push");
    let deferred = "deferred handler bytes during atomic push\n";
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nif [ ! -f '{}' ]; then\n  touch '{}'\n  printf '%s' '{}' > '{}'\nfi\n",
            marker.display(),
            marker.display(),
            deferred,
            clone.path().join("f0.txt").display(),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let outcome = try_fire_rotation(
        &storage,
        "main",
        3,
        arch.path(),
        ("d", "d@g"),
        "2026-07-31T02:20:00Z",
    )
    .unwrap();

    assert!(matches!(outcome, RotationOutcome::Won { .. }));
    assert_eq!(head_branch(&clone), "main");
    assert_eq!(
        storage.rev_parse("HEAD").unwrap(),
        storage.rev_parse("origin/main").unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(clone.path().join("f0.txt")).unwrap(),
        deferred
    );
    assert!(storage.has_dirty_tracked_files().unwrap());
    assert_eq!(
        storage.rev_parse("main-epoch-2").unwrap(),
        storage.rev_parse("origin/main-epoch-2").unwrap()
    );

    git(&clone, &["restore", "--", "f0.txt"]);
    assert!(follow_redirect(&storage, "main").unwrap());
    assert_eq!(head_branch(&clone), "main-epoch-2");
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
fn lost_fire_replays_a_handler_tail_committed_after_the_local_seal() {
    let (bare, clone) = setup_bare_and_clone(3);
    let writer = clone_from(&bare);
    commit_file(&writer, "remote.txt", "remote advance");

    let marker = clone.path().join(".git/rotation-tail-fired");
    let hook = clone.path().join(".git/hooks/pre-push");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nif [ ! -f '{}' ]; then\n  touch '{}'\n  cat > '{}' <<'EOF'\n[L000001][P000000][@handler][20260730T230000Z] preserved after seal\nEOF\n  git -C '{}' add general.thread\n  git -C '{}' commit -m 'handler message after local seal'\n  git -C '{}' push origin main\nfi\n",
            marker.display(),
            marker.display(),
            clone.path().join("general.thread").display(),
            clone.path().display(),
            clone.path().display(),
            writer.path().display(),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
    }

    let storage = GitStorage::new(clone.path());
    let archive = tempfile::tempdir().unwrap();
    let outcome = try_fire_rotation(
        &storage,
        "main",
        3,
        archive.path(),
        ("d", "d@g"),
        "2026-07-30T23:00:00Z",
    )
    .unwrap();

    assert!(matches!(outcome, RotationOutcome::Lost), "got {outcome:?}");
    assert_eq!(head_branch(&clone), "main");
    assert!(!clone.path().join("gitim.epoch.yaml").exists());
    assert_eq!(
        std::fs::read_to_string(clone.path().join("general.thread")).unwrap(),
        "[L000001][P000000][@handler][20260730T230000Z] preserved after seal\n"
    );
    assert!(
        Command::new("git")
            .args(["branch", "--list", "main-epoch-2"])
            .current_dir(clone.path())
            .output()
            .is_ok_and(|output| output.status.success() && output.stdout.is_empty()),
        "losing orphan ref must be removed"
    );
    assert!(
        !clone.path().join(".gitim/rotation-recovery.json").exists(),
        "completed recovery must clear its journal"
    );
    let tail_refs = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname)",
            "refs/gitim/rotation-tail/",
        ])
        .current_dir(clone.path())
        .output()
        .unwrap();
    assert!(tail_refs.status.success());
    assert!(
        tail_refs.stdout.is_empty(),
        "completed recovery must clear its tail ref"
    );
    assert!(
        Command::new("git")
            .args([
                "--git-dir",
                bare.path().to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/main-epoch-2",
            ])
            .status()
            .is_ok_and(|status| !status.success()),
        "atomic loss must not publish a partial epoch"
    );

    storage.push().unwrap();
    let published = Command::new("git")
        .args([
            "--git-dir",
            bare.path().to_str().unwrap(),
            "show",
            "main:general.thread",
        ])
        .output()
        .unwrap();
    assert!(published.status.success());
    let published = String::from_utf8(published.stdout).unwrap();
    assert_eq!(published.matches("preserved after seal").count(), 1);
}

#[test]
fn failed_rotation_recovery_resumes_from_a_prepared_tail_journal() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let writer = clone_from(&_bare);
    commit_file(&writer, "remote.txt", "remote advance");
    git(&writer, &["push", "origin", "main"]);

    let storage = GitStorage::new(clone.path());
    storage.fetch().unwrap();
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();
    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect prepared rotation recovery"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "general.thread",
        "[L000001][P000000][@handler][20260730T231500Z] crash-safe tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "prepared"
        }))
        .unwrap(),
    )
    .unwrap();

    gitim_sync::rotate::cleanup_failed_fire(
        &storage,
        &std::sync::Mutex::new(()),
        "main",
        "main-epoch-2",
    )
    .unwrap();

    assert_eq!(head_branch(&clone), "main");
    assert!(!clone.path().join("gitim.epoch.yaml").exists());
    assert_eq!(
        std::fs::read_to_string(clone.path().join("general.thread")).unwrap(),
        "[L000001][P000000][@handler][20260730T231500Z] crash-safe tail\n"
    );
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
    assert!(Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &tail_ref])
        .current_dir(clone.path())
        .status()
        .is_ok_and(|status| !status.success()));
}

#[test]
fn prepared_rotation_recovery_resumes_onto_a_remote_winners_active_branch() {
    let (bare, clone) = setup_bare_and_clone(3);
    let writer = clone_from(&bare);
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let storage = GitStorage::new(clone.path());
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    let writer_storage = GitStorage::new(writer.path());
    let archive = tempfile::tempdir().unwrap();
    assert!(matches!(
        try_fire_rotation(
            &writer_storage,
            "main",
            3,
            archive.path(),
            ("winner", "winner@g"),
            "2026-07-30T23:20:00Z",
        )
        .unwrap(),
        RotationOutcome::Won { .. }
    ));
    storage.fetch().unwrap();
    let upstream = storage.rev_parse("origin/main").unwrap();
    let active = storage.rev_parse("origin/main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect losing local rotation"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "general.thread",
        "[L000001][P000000][@handler][20260730T232000Z] migrate to winner\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main-epoch-2",
            "active_oid": active,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "prepared"
        }))
        .unwrap(),
    )
    .unwrap();

    gitim_sync::rotate::cleanup_failed_fire(
        &storage,
        &std::sync::Mutex::new(()),
        "main",
        "main-epoch-2",
    )
    .unwrap();

    assert_eq!(head_branch(&clone), "main-epoch-2");
    assert_eq!(upstream_of(&clone, "main-epoch-2"), "origin/main-epoch-2");
    assert_eq!(
        std::fs::read_to_string(clone.path().join("general.thread")).unwrap(),
        "[L000001][P000000][@handler][20260730T232000Z] migrate to winner\n"
    );
    assert_eq!(storage.rev_parse("main").unwrap(), upstream);
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
}

fn assert_rotation_recovery_resumes_after_phase_crash(
    phase: &str,
    delete_orphan_before_resume: bool,
    prune_tail_before_resume: bool,
) {
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect crash phase fixture"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "phase-crash.thread",
        "[L000001][P000000][@handler][20260730T233000Z] publish after restart\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);

    git(&clone, &["reset", "--hard", &upstream]);
    git(&clone, &["cherry-pick", &tail_head]);
    let repaired_head = storage.rev_parse("HEAD").unwrap();
    if delete_orphan_before_resume {
        git(&clone, &["branch", "-D", "main-epoch-2"]);
    }

    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": phase,
            "repaired_head": repaired_head
        }))
        .unwrap(),
    )
    .unwrap();
    if prune_tail_before_resume {
        git(&clone, &["update-ref", "-d", &tail_ref, &tail_head]);
        git(&clone, &["reflog", "expire", "--expire=now", "--all"]);
        git(&clone, &["gc", "--prune=now"]);
        for oid in [&seal_oid, &tail_head] {
            let status = Command::new("git")
                .args(["cat-file", "-e", &format!("{oid}^{{commit}}")])
                .current_dir(clone.path())
                .status()
                .unwrap();
            assert!(!status.success(), "{oid} must be pruned before restart");
        }
    }

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert_eq!(head_branch(&clone), "main");
    assert_eq!(storage.rev_parse("HEAD").unwrap(), repaired_head);
    assert_eq!(upstream_of(&clone, "main"), "origin/main");
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
    assert!(Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &tail_ref])
        .current_dir(clone.path())
        .status()
        .is_ok_and(|status| !status.success()));

    storage.push().unwrap();
    let published = Command::new("git")
        .args([
            "--git-dir",
            _bare.path().to_str().unwrap(),
            "show",
            "main:phase-crash.thread",
        ])
        .output()
        .unwrap();
    assert!(published.status.success());
    assert_eq!(
        String::from_utf8(published.stdout)
            .unwrap()
            .matches("publish after restart")
            .count(),
        1
    );
}

#[test]
fn rotation_recovery_advances_prepared_after_branch_move_before_phase_save() {
    assert_rotation_recovery_resumes_after_phase_crash("prepared", false, false);
}

#[test]
fn rotation_recovery_resumes_after_moved_phase_save() {
    assert_rotation_recovery_resumes_after_phase_crash("moved", false, false);
}

#[test]
fn rotation_recovery_resumes_after_completed_phase_save() {
    assert_rotation_recovery_resumes_after_phase_crash("completed", true, false);
}

#[test]
fn completed_rotation_cleanup_resumes_after_tail_objects_are_pruned() {
    assert_rotation_recovery_resumes_after_phase_crash("completed", true, true);
}

fn assert_rotation_cleanup_preserves_a_switched_branch(phase: &str) {
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect cleanup branch fixture"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "cleanup-branch.thread",
        "[L000001][P000000][@handler][20260731T015000Z] durable tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &upstream]);
    git(&clone, &["cherry-pick", &tail_head]);
    let repaired_head = storage.rev_parse("HEAD").unwrap();

    git(&clone, &["checkout", "-b", "unrelated", &upstream]);
    commit_file(&clone, "unrelated.txt", "keep unrelated bytes");
    let unrelated_head = storage.rev_parse("HEAD").unwrap();
    let unrelated_bytes = std::fs::read(clone.path().join("unrelated.txt")).unwrap();

    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    let journal_path = clone.path().join(".gitim/rotation-recovery.json");
    std::fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": phase,
            "repaired_head": repaired_head
        }))
        .unwrap(),
    )
    .unwrap();
    let journal_bytes = std::fs::read(&journal_path).unwrap();

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    let result = guard.resume_pending_recoveries(&storage, &std::sync::Mutex::new(()));

    assert!(matches!(
        result,
        Err(gitim_sync::skill::checkpoint::SkillSyncError::Git(
            gitim_sync::git::GitError::PushConflict
        ))
    ));
    assert_eq!(head_branch(&clone), "unrelated");
    assert_eq!(storage.rev_parse("HEAD").unwrap(), unrelated_head);
    assert_eq!(
        std::fs::read(clone.path().join("unrelated.txt")).unwrap(),
        unrelated_bytes
    );
    assert_eq!(storage.rev_parse(&tail_ref).unwrap(), tail_head);
    assert_eq!(storage.rev_parse("main-epoch-2").unwrap(), orphan_oid);
    assert_eq!(std::fs::read(&journal_path).unwrap(), journal_bytes);
}

#[test]
fn moved_rotation_cleanup_preserves_a_switched_symbolic_branch() {
    assert_rotation_cleanup_preserves_a_switched_branch("moved");
}

#[test]
fn completed_rotation_cleanup_preserves_a_switched_symbolic_branch() {
    assert_rotation_cleanup_preserves_a_switched_branch("completed");
}

#[test]
fn prepared_rotation_recovery_replays_again_after_the_active_remote_advances() {
    let (bare, clone) = setup_bare_and_clone(3);
    let writer = clone_from(&bare);
    let storage = GitStorage::new(clone.path());
    let active_oid = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect remote advance fixture"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "local-tail.thread",
        "[L000001][P000000][@handler][20260730T234000Z] durable local tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &active_oid]);
    git(&clone, &["cherry-pick", &tail_head]);
    let prior_repaired_head = storage.rev_parse("HEAD").unwrap();
    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": active_oid,
            "active_branch": "main",
            "active_oid": active_oid,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "prepared",
            "repaired_head": prior_repaired_head
        }))
        .unwrap(),
    )
    .unwrap();

    commit_file(
        &writer,
        "remote-advance.thread",
        "ordinary remote advance\n",
    );
    git(&writer, &["push", "origin", "main"]);

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert_eq!(head_branch(&clone), "main");
    assert_eq!(
        std::fs::read_to_string(clone.path().join("remote-advance.thread")).unwrap(),
        "ordinary remote advance\n"
    );
    assert!(clone.path().join("local-tail.thread").exists());
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());

    storage.push().unwrap();
    git(&clone, &["fetch", "origin"]);
    for (path, expected) in [
        ("remote-advance.thread", "ordinary remote advance"),
        ("local-tail.thread", "durable local tail"),
    ] {
        let content = Command::new("git")
            .args(["show", &format!("origin/main:{path}")])
            .current_dir(clone.path())
            .output()
            .unwrap();
        assert!(content.status.success(), "{path} must publish");
        assert_eq!(
            String::from_utf8(content.stdout)
                .unwrap()
                .matches(expected)
                .count(),
            1
        );
    }
}

#[test]
fn replayed_rotation_recovery_replays_again_after_the_active_remote_advances() {
    let (bare, clone) = setup_bare_and_clone(3);
    let writer = clone_from(&bare);
    let storage = GitStorage::new(clone.path());
    let active_oid = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect replayed remote advance"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "replayed-local-tail.thread",
        "[L000001][P000000][@handler][20260731T031500Z] replayed durable local tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &active_oid]);
    git(&clone, &["cherry-pick", &tail_head]);
    let stale_repaired_head = storage.rev_parse("HEAD").unwrap();

    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": active_oid,
            "active_branch": "main",
            "active_oid": active_oid,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "replayed",
            "repaired_head": stale_repaired_head
        }))
        .unwrap(),
    )
    .unwrap();

    commit_file(
        &writer,
        "replayed-remote-advance.thread",
        "remote advance after replay\n",
    );
    git(&writer, &["push", "origin", "main"]);

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert_eq!(head_branch(&clone), "main");
    assert_ne!(storage.rev_parse("HEAD").unwrap(), stale_repaired_head);
    assert_eq!(
        std::fs::read_to_string(clone.path().join("replayed-remote-advance.thread")).unwrap(),
        "remote advance after replay\n"
    );
    assert!(clone.path().join("replayed-local-tail.thread").exists());
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());

    storage.push().unwrap();
    git(&clone, &["fetch", "origin"]);
    for path in [
        "replayed-remote-advance.thread",
        "replayed-local-tail.thread",
    ] {
        let status = Command::new("git")
            .args(["cat-file", "-e", &format!("origin/main:{path}")])
            .current_dir(clone.path())
            .status()
            .unwrap();
        assert!(status.success(), "{path} must publish");
    }
}

#[test]
fn replayed_rotation_recovery_replays_onto_a_remote_epoch_winner() {
    let (bare, clone) = setup_bare_and_clone(3);
    let writer = clone_from(&bare);
    let storage = GitStorage::new(clone.path());
    let active_oid = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect replayed epoch winner"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "replayed-epoch-tail.thread",
        "[L000001][P000000][@handler][20260731T031600Z] replay onto epoch winner\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &active_oid]);
    git(&clone, &["cherry-pick", &tail_head]);
    let stale_repaired_head = storage.rev_parse("HEAD").unwrap();

    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": active_oid,
            "active_branch": "main",
            "active_oid": active_oid,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "replayed",
            "repaired_head": stale_repaired_head
        }))
        .unwrap(),
    )
    .unwrap();

    let writer_storage = GitStorage::new(writer.path());
    let archive = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(
            &writer_storage,
            "main",
            1,
            archive.path(),
            ("winner", "winner@example.com"),
            "2026-07-31T03:16:30Z",
        )
        .unwrap(),
        RotationOutcome::Won { .. }
    ));

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert_eq!(head_branch(&clone), "main-epoch-2");
    assert_ne!(storage.rev_parse("HEAD").unwrap(), stale_repaired_head);
    assert!(clone.path().join("replayed-epoch-tail.thread").exists());
    assert_eq!(
        storage
            .show_file_at_ref("HEAD", "gitim.epoch.yaml")
            .unwrap(),
        storage
            .show_file_at_ref("origin/main-epoch-2", "gitim.epoch.yaml")
            .unwrap()
    );
    assert_eq!(
        storage.rev_parse("main").unwrap(),
        storage.rev_parse("origin/main").unwrap()
    );
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());

    storage.push().unwrap();
    git(&clone, &["fetch", "origin"]);
    let status = Command::new("git")
        .args([
            "cat-file",
            "-e",
            "origin/main-epoch-2:replayed-epoch-tail.thread",
        ])
        .current_dir(clone.path())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn prepared_rotation_recovery_captures_a_handler_commit_after_transient_boot_failure() {
    let (bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect transient boot fixture"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "before-boot.thread",
        "[L000001][P000000][@handler][20260731T001000Z] durable before boot\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "prepared"
        }))
        .unwrap(),
    )
    .unwrap();

    let remote_url = bare.path().to_str().unwrap();
    git(
        &clone,
        &["remote", "set-url", "origin", "/missing/gitim-origin"],
    );
    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    assert!(
        guard
            .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
            .is_err(),
        "boot recovery must surface the transient fetch failure"
    );

    commit_file(
        &clone,
        "after-boot.thread",
        "[L000001][P000000][@handler][20260731T001100Z] committed while boot retry waits\n",
    );
    git(&clone, &["remote", "set-url", "origin", remote_url]);

    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
    let tail_refs = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/gitim/rotation-tail/{seal_oid}"),
        ])
        .current_dir(clone.path())
        .output()
        .unwrap();
    assert!(tail_refs.status.success());
    assert!(
        tail_refs.stdout.is_empty(),
        "completed recovery must clear every operation tail ref"
    );
    assert!(clone.path().join("before-boot.thread").exists());
    assert!(clone.path().join("after-boot.thread").exists());
    storage.push().unwrap();
    git(&clone, &["fetch", "origin"]);
    for (path, expected) in [
        ("before-boot.thread", "durable before boot"),
        ("after-boot.thread", "committed while boot retry waits"),
    ] {
        let content = Command::new("git")
            .args(["show", &format!("origin/main:{path}")])
            .current_dir(clone.path())
            .output()
            .unwrap();
        assert!(content.status.success(), "{path} must publish");
        assert_eq!(
            String::from_utf8(content.stdout)
                .unwrap()
                .matches(expected)
                .count(),
            1
        );
    }
}

#[test]
fn prepared_rotation_recovery_replays_a_handler_commit_above_its_repaired_head() {
    let (bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect repaired boot fixture"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "repaired-base.thread",
        "[L000001][P000000][@handler][20260731T002000Z] original durable tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &upstream]);
    git(&clone, &["cherry-pick", &tail_head]);
    let repaired_head = storage.rev_parse("HEAD").unwrap();
    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "prepared",
            "repaired_head": repaired_head
        }))
        .unwrap(),
    )
    .unwrap();

    let remote_url = bare.path().to_str().unwrap();
    git(
        &clone,
        &["remote", "set-url", "origin", "/missing/gitim-origin"],
    );
    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    assert!(
        guard
            .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
            .is_err(),
        "boot recovery must surface the transient fetch failure"
    );
    commit_file(
        &clone,
        "repaired-descendant.thread",
        "[L000001][P000000][@handler][20260731T002100Z] appended above repaired\n",
    );
    git(&clone, &["remote", "set-url", "origin", remote_url]);

    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
    assert!(clone.path().join("repaired-base.thread").exists());
    assert!(clone.path().join("repaired-descendant.thread").exists());
    storage.push().unwrap();
    git(&clone, &["fetch", "origin"]);
    for (path, expected) in [
        ("repaired-base.thread", "original durable tail"),
        ("repaired-descendant.thread", "appended above repaired"),
    ] {
        let content = Command::new("git")
            .args(["show", &format!("origin/main:{path}")])
            .current_dir(clone.path())
            .output()
            .unwrap();
        assert!(content.status.success(), "{path} must publish");
        assert_eq!(
            String::from_utf8(content.stdout)
                .unwrap()
                .matches(expected)
                .count(),
            1
        );
    }
}

#[test]
fn captured_repaired_descendant_survives_journal_reload() {
    use std::os::unix::fs::PermissionsExt;

    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(&clone, &["commit", "-m", "seal: redirect reload fixture"]);
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "reload-base.thread",
        "[L000001][P000000][@handler][20260731T011000Z] durable tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &upstream]);
    git(&clone, &["cherry-pick", &tail_head]);
    let repaired_head = storage.rev_parse("HEAD").unwrap();
    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "prepared",
            "repaired_head": repaired_head
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        clone.path().join("reload-descendant.thread"),
        "[L000001][P000000][@handler][20260731T011100Z] appended above repaired\n",
    )
    .unwrap();
    git(&clone, &["add", "reload-descendant.thread"]);
    git(&clone, &["commit", "-m", "reload-descendant.thread"]);
    let expected_head = storage.rev_parse("HEAD").unwrap();

    let hook = clone.path().join(".git/hooks/post-commit");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nprintf 'internal replay pause\\n' > '{}'\n",
            clone.path().join("f0.txt").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    assert!(
        guard
            .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
            .is_err(),
        "the injected tracked change must stop recovery after capture"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(clone.path().join(".gitim/rotation-recovery.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["expected_head"], expected_head);
    assert_eq!(persisted["prior_repaired_head"], repaired_head);
    assert_ne!(persisted["tail_head"], tail_head);

    std::fs::remove_file(hook).unwrap();
    git(&clone, &["restore", "--", "f0.txt"]);
    let restarted_guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    restarted_guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
    let tail_refs = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/gitim/rotation-tail/{seal_oid}"),
        ])
        .current_dir(clone.path())
        .output()
        .unwrap();
    assert!(tail_refs.status.success());
    assert!(tail_refs.stdout.is_empty());
    assert!(clone.path().join("reload-base.thread").exists());
    assert!(clone.path().join("reload-descendant.thread").exists());
    storage.push().unwrap();
    git(&clone, &["fetch", "origin"]);
    for (path, expected) in [
        ("reload-base.thread", "durable tail"),
        ("reload-descendant.thread", "appended above repaired"),
    ] {
        let content = Command::new("git")
            .args(["show", &format!("origin/main:{path}")])
            .current_dir(clone.path())
            .output()
            .unwrap();
        assert!(content.status.success(), "{path} must publish after reload");
        assert_eq!(
            String::from_utf8(content.stdout)
                .unwrap()
                .matches(expected)
                .count(),
            1
        );
    }
}

#[test]
fn replayed_rotation_reconciles_an_update_ref_only_crash_residue() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let upstream = storage.rev_parse("origin/main").unwrap();
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    let orphan_oid = storage.rev_parse("main-epoch-2").unwrap();

    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(
        &clone,
        &["commit", "-m", "seal: redirect update-ref residue fixture"],
    );
    let seal_oid = storage.rev_parse("HEAD").unwrap();
    commit_file(
        &clone,
        "update-ref-residue.thread",
        "[L000001][P000000][@handler][20260731T012000Z] durable tail\n",
    );
    let tail_head = storage.rev_parse("HEAD").unwrap();
    let tail_ref = format!("refs/gitim/rotation-tail/{seal_oid}");
    git(&clone, &["update-ref", &tail_ref, &tail_head]);
    git(&clone, &["reset", "--hard", &upstream]);
    git(&clone, &["cherry-pick", &tail_head]);
    let repaired_head = storage.rev_parse("HEAD").unwrap();
    git(&clone, &["reset", "--hard", &tail_head]);
    git(
        &clone,
        &["update-ref", "refs/heads/main", &repaired_head, &tail_head],
    );

    std::fs::create_dir_all(clone.path().join(".gitim")).unwrap();
    std::fs::write(
        clone.path().join(".gitim/rotation-recovery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "operation_id": seal_oid,
            "branch": "main",
            "upstream_oid": upstream,
            "active_branch": "main",
            "active_oid": upstream,
            "seal_oid": seal_oid,
            "tail_ref": tail_ref,
            "tail_head": tail_head,
            "orphan_branch": "main-epoch-2",
            "orphan_oid": orphan_oid,
            "phase": "replayed",
            "repaired_head": repaired_head
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(storage.has_dirty_tracked_files().unwrap());

    let guard = gitim_sync::skill::guard::SkillSyncGuard::new(clone.path()).unwrap();
    std::fs::write(clone.path().join("f0.txt"), "deferred send\n").unwrap();
    assert!(
        guard
            .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
            .is_err(),
        "real tracked work must remain protected"
    );
    assert_eq!(
        std::fs::read_to_string(clone.path().join("f0.txt")).unwrap(),
        "deferred send\n"
    );
    assert!(clone.path().join(".gitim/rotation-recovery.json").exists());
    git(&clone, &["restore", "--", "f0.txt"]);

    guard
        .resume_pending_recoveries(&storage, &std::sync::Mutex::new(()))
        .unwrap();

    assert_eq!(storage.rev_parse("HEAD").unwrap(), repaired_head);
    assert!(!storage.has_dirty_tracked_files().unwrap());
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
    assert!(clone.path().join("update-ref-residue.thread").exists());
}

#[test]
fn cleanup_refuses_when_foreign_commits_ahead() {
    // Zero-loss guard I3: foreign commits ahead of origin → no reset.
    let (_bare, clone) = setup_bare_and_clone(3);
    commit_file(&clone, "user-msg.thread", "[L1][@x][t] precious");
    let storage = GitStorage::new(clone.path());

    let error = gitim_sync::rotate::cleanup_failed_fire(
        &storage,
        &std::sync::Mutex::new(()),
        "main",
        "main-epoch-2",
    )
    .unwrap_err();
    assert!(error.to_string().contains("exactly one local seal"));
    assert!(
        clone.path().join("user-msg.thread").exists(),
        "foreign commit must not be reset away"
    );
}

#[test]
fn follow_noop_when_origin_active() {
    let (_bare, clone) = setup_bare_and_clone(2);
    let storage = GitStorage::new(clone.path());
    let acted = follow_redirect(&storage, "main").unwrap();
    assert!(!acted);
    assert_eq!(head_branch(&clone), "main");
}

#[test]
fn follow_switches_and_migrates_unpushed() {
    // A fires; B has one unpushed message → follow must carry it to the new branch.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);

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
    assert!(matches!(o, RotationOutcome::Won { .. }));

    commit_file(
        &clone_b,
        "general.thread",
        "[L1][@b][2026-06-10T00:00:01Z] hello",
    );

    let storage_b = GitStorage::new(clone_b.path());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-2");
    assert_eq!(
        upstream_of(&clone_b, "main-epoch-2"),
        "origin/main-epoch-2",
        "follow must leave the target branch publishable"
    );
    assert!(clone_b.path().join("general.thread").exists());
    let yaml = std::fs::read_to_string(clone_b.path().join("gitim.epoch.yaml")).unwrap();
    assert!(yaml.contains("status: active"));
}

#[test]
fn follow_resolves_across_two_epochs() {
    // Two consecutive rotations; a sleeping B follows once → lands on epoch 3.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t1").unwrap(),
        RotationOutcome::Won { .. }
    ));
    for i in 0..3 {
        commit_file(&clone_a, &format!("e2-{i}.txt"), "x");
    }
    git(&clone_a, &["push", "origin", "main-epoch-2"]);
    assert!(matches!(
        try_fire_rotation(
            &storage_a,
            "main-epoch-2",
            3,
            arch.path(),
            ("a", "a@g"),
            "t2"
        )
        .unwrap(),
        RotationOutcome::Won { .. }
    ));

    let storage_b = GitStorage::new(clone_b.path());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-3");
    assert_eq!(
        upstream_of(&clone_b, "main-epoch-3"),
        "origin/main-epoch-3",
        "multi-hop follow must leave the final branch publishable"
    );
}

#[test]
fn fence_blocks_push_when_head_redirected() {
    // B pulled R (HEAD tree's epoch.yaml = redirected) → fence must report true.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));
    git(&clone_b, &["fetch", "origin"]);
    git(&clone_b, &["reset", "--hard", "origin/main"]); // simulate pulling R
    commit_file(&clone_b, "late.thread", "[L1][@b][t] late msg"); // scenario 4

    let storage_b = GitStorage::new(clone_b.path());
    assert!(
        check_push_fence(&storage_b).unwrap(),
        "HEAD carries redirected epoch.yaml"
    );
    assert!(
        !check_push_fence(&storage_a).unwrap(),
        "active branch must pass the fence"
    );
}

#[test]
fn fire_with_dirty_tracked_file_returns_not_ready() {
    // Zero-loss (review R-I2): send.rs defers a failed `git commit` by
    // leaving the message on disk for sync_loop to commit later. That
    // content exists nowhere but this working tree — Won's `checkout -f` /
    // Lost's `reset --hard` would destroy it permanently, so fire must
    // refuse to rotate over a dirty tracked file.
    let (_bare, clone) = setup_bare_and_clone(5);
    std::fs::write(
        clone.path().join("f0.txt"),
        "c0\n[L1][@x][t] deferred, uncommitted",
    )
    .unwrap();

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
    let content = std::fs::read_to_string(clone.path().join("f0.txt")).unwrap();
    assert!(
        content.contains("deferred, uncommitted"),
        "dirty tracked content must survive"
    );
    assert_eq!(head_branch(&clone), "main");
}

#[test]
fn follow_migrates_message_committed_on_top_of_pulled_redirect() {
    // Design scenario 4, Shape B (review R-I4): B pulled R, then a handler
    // committed a message on top of it. origin/main..HEAD = [msg] only —
    // R is reachable from origin/main, so migrate transplants exactly the
    // message and never replays the seal commit onto the new epoch.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));
    git(&clone_b, &["fetch", "origin"]);
    git(&clone_b, &["reset", "--hard", "origin/main"]); // R now in local chain
    commit_file(&clone_b, "late.thread", "[L1][@b][t] late msg");

    let storage_b = GitStorage::new(clone_b.path());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-2");
    assert!(clone_b.path().join("late.thread").exists());
    let out = Command::new("git")
        .args(["log", "--format=%s", "main-epoch-2"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    let subjects = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        subjects.contains("late.thread"),
        "message must ride the new epoch: {subjects}"
    );
    assert!(
        !subjects.contains("seal: redirect"),
        "R must not be transplanted onto the new epoch: {subjects}"
    );
    let local = storage_b.rev_parse("main").unwrap();
    let remote = storage_b.rev_parse("origin/main").unwrap();
    assert_eq!(
        local, remote,
        "old branch must align to origin after follow"
    );
}

#[test]
fn follow_migrate_conflict_aborts_cleanly() {
    // Review R-I3: a conflicted migrate rebase must not strand the clone
    // mid-rebase (.git/rebase-merge + detached HEAD). Err contract: the
    // switch did not happen — HEAD back on the old branch, message intact.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);

    // A rewrites f0.txt and pushes, then fires: the snapshot tree carries
    // "A version".
    commit_file(&clone_a, "f0.txt", "A version");
    git(&clone_a, &["push", "origin", "main"]);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));

    // B (stale base "c0") rewrites the same file differently → the migrate
    // rebase onto the snapshot must conflict.
    commit_file(&clone_b, "f0.txt", "B version");

    let storage_b = GitStorage::new(clone_b.path());
    let result = follow_redirect(&storage_b, "main");
    assert!(result.is_err(), "conflicted migrate must surface as Err");

    assert!(
        !clone_b.path().join(".git/rebase-merge").exists()
            && !clone_b.path().join(".git/rebase-apply").exists(),
        "no mid-rebase state may persist after a failed follow"
    );
    assert_eq!(
        head_branch(&clone_b),
        "main",
        "HEAD must be back on the old branch"
    );
    let content = std::fs::read_to_string(clone_b.path().join("f0.txt")).unwrap();
    assert_eq!(content, "B version", "local message commit must be intact");
}

#[test]
fn race_two_daemons_only_one_wins_other_follows() {
    // Design scenario 1: two daemons cross the threshold and fire over the
    // same sealed tip — exactly one Won, the other Lost; the loser follows
    // and both converge with zero residue.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let storage_b = GitStorage::new(clone_b.path());
    let arch_a = tempfile::TempDir::new().unwrap();
    let arch_b = tempfile::TempDir::new().unwrap();

    let oa = try_fire_rotation(&storage_a, "main", 3, arch_a.path(), ("a", "a@g"), "t").unwrap();
    let ob = try_fire_rotation(&storage_b, "main", 3, arch_b.path(), ("b", "b@g"), "t").unwrap();
    assert!(matches!(oa, RotationOutcome::Won { .. }), "got {oa:?}");
    assert!(matches!(ob, RotationOutcome::Lost), "got {ob:?}");

    // Loser follows; both converge on the same branch; loser has no residue.
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    for cl in [&clone_a, &clone_b] {
        assert_eq!(head_branch(cl), "main-epoch-2");
    }
    let out = Command::new("git")
        .args(["log", "--oneline", "main", "-1"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    let local_main_tip = String::from_utf8_lossy(&out.stdout);
    assert!(
        local_main_tip.contains("seal: redirect"),
        "loser's local main must equal origin (winner's R), got: {local_main_tip}"
    );
}

#[test]
fn normal_push_loses_to_fire_message_migrates() {
    // Design scenario 3 end-to-end: B writes a message but fire already
    // happened → B's push rejects, the message reaches the new branch via
    // follow's migrate, AND the sealed branch tip remains R (invariant 1).
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));

    commit_file(
        &clone_b,
        "ch.thread",
        "[L1][@b][t] msg born on sealed branch",
    );
    let storage_b = GitStorage::new(clone_b.path());
    // B's push must reject (origin/main already carries R) — sync_loop would
    // then run fence + follow.
    assert!(storage_b.push().is_err());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-2");
    assert!(
        clone_b.path().join("ch.thread").exists(),
        "message survived migration"
    );

    // Publish from the new branch, then verify invariant 1 on origin.
    git(&clone_b, &["push", "origin", "main-epoch-2"]);
    git(&clone_b, &["fetch", "origin"]);
    let tip_msg = Command::new("git")
        .args(["log", "-1", "--format=%s", "origin/main"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&tip_msg.stdout).starts_with("seal: redirect"),
        "sealed branch tip must remain the redirect commit"
    );
}

#[test]
fn boot_cleanup_resets_partial_fire_residue() {
    // Design scenario 7: a fire that died after its local commits but before
    // the atomic push leaves R' on local main + a stale orphan branch, while
    // origin stays clean. Boot cleanup must reset both away.
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());

    // Manufacture the residue in fire's real order: orphan first, then the
    // redirect commit on main. Subject MUST start with "seal: redirect" —
    // cleanup's self-produced-commit verification gates on that prefix.
    storage
        .create_orphan_commit(
            "main-epoch-2",
            "gitim.epoch.yaml",
            "status: active\n",
            "snapshot: partial",
            ("d", "d@g"),
        )
        .unwrap();
    let redirect = gitim_core::epoch::EpochFile::new_redirect(
        1,
        "main".into(),
        2,
        "main-epoch-2".into(),
        "deadbeef".into(),
        "deadbeef".into(),
        "t".into(),
        None,
    );
    let yaml = serde_yaml::to_string(&redirect).unwrap();
    storage
        .write_redirect_commit(
            "gitim.epoch.yaml",
            &yaml,
            "seal: redirect epoch 1 -> main-epoch-2 (partial fire)",
            ("d", "d@g"),
        )
        .unwrap();

    gitim_sync::rotate::cleanup_failed_fire(
        &storage,
        &std::sync::Mutex::new(()),
        "main",
        "main-epoch-2",
    )
    .unwrap();

    assert_eq!(head_branch(&clone), "main");
    assert!(!clone.path().join("gitim.epoch.yaml").exists());
    assert_eq!(
        storage.rev_parse("main").unwrap(),
        storage.rev_parse("origin/main").unwrap(),
        "local main must be back on origin"
    );
    let out = Command::new("git")
        .args(["branch", "-l", "main-epoch-2"])
        .current_dir(clone.path())
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stale orphan branch must be deleted");
}

// === Task 7: sync_loop fence integration ===

/// One full sync cycle with no-op callbacks — exercises the real
/// `run_sync_cycle` path (fence checkpoints included).
fn run_one_sync_cycle(storage: &GitStorage, lock: &std::sync::Mutex<()>) {
    let mut circuit = gitim_sync::sync_loop::AuthCircuit::new(std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(false),
    ));
    gitim_sync::sync_loop::run_sync_cycle(
        storage,
        &mut circuit,
        lock,
        &|_, _| {},
        &|_, _, _| {},
        &|_| {},
        &|| {},
        None,
    );
}

#[test]
fn sync_cycle_routes_message_to_new_epoch_after_rotation() {
    // B has an unpushed message; origin already rotated. Two sync cycles must
    // land the message on origin/main-epoch-2 and never publish anything
    // after R on origin/main (invariant 1).
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));
    commit_file(
        &clone_b,
        "late.thread",
        "[L1][@b][t] written before B knows",
    );

    let storage_b = GitStorage::new(clone_b.path());
    let lock = std::sync::Mutex::new(());
    // Cycle 1: push rejects -> fetch -> fence (i) -> follow + migrate.
    // Cycle 2: pushes from the new branch.
    run_one_sync_cycle(&storage_b, &lock);
    run_one_sync_cycle(&storage_b, &lock);

    git(&clone_b, &["fetch", "origin"]);
    let out = Command::new("git")
        .args(["show", "origin/main-epoch-2:late.thread"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "message must land on origin/main-epoch-2"
    );
    let tip = Command::new("git")
        .args(["log", "-1", "--format=%s", "origin/main"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&tip.stdout).starts_with("seal: redirect"),
        "sealed branch tip must remain the redirect commit"
    );
}

#[test]
fn fence_self_heals_stranded_redirect_residue() {
    // R' stranded locally (a lost fire whose cleanup failed) while origin is
    // active -> one sync cycle retries the cleanup and unbricks the node.
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let redirect = gitim_core::epoch::EpochFile::new_redirect(
        1,
        "main".into(),
        2,
        "main-epoch-2".into(),
        "deadbeef".into(),
        "deadbeef".into(),
        "t".into(),
        None,
    );
    let yaml = serde_yaml::to_string(&redirect).unwrap();
    storage
        .write_redirect_commit(
            "gitim.epoch.yaml",
            &yaml,
            "seal: redirect epoch 1 -> main-epoch-2 (lost, cleanup failed)",
            ("d", "d@g"),
        )
        .unwrap();

    let lock = std::sync::Mutex::new(());
    run_one_sync_cycle(&storage, &lock);

    assert_eq!(
        storage.rev_parse("main").unwrap(),
        storage.rev_parse("origin/main").unwrap(),
        "stranded R' must be cleaned up"
    );
    assert!(!clone.path().join("gitim.epoch.yaml").exists());
}

#[test]
fn cleanup_refuses_when_tracked_files_dirty() {
    // A deferred-send dirty file (commit failed, left on disk for sync to
    // pick up) must never be eaten by cleanup's reset --hard.
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let redirect = gitim_core::epoch::EpochFile::new_redirect(
        1,
        "main".into(),
        2,
        "main-epoch-2".into(),
        "deadbeef".into(),
        "deadbeef".into(),
        "t".into(),
        None,
    );
    let yaml = serde_yaml::to_string(&redirect).unwrap();
    storage
        .write_redirect_commit(
            "gitim.epoch.yaml",
            &yaml,
            "seal: redirect epoch 1 -> main-epoch-2 (partial fire)",
            ("d", "d@g"),
        )
        .unwrap();
    // Dirty a TRACKED file after the residue commit (f0.txt exists from setup).
    std::fs::write(clone.path().join("f0.txt"), "deferred message content").unwrap();

    let error = gitim_sync::rotate::cleanup_failed_fire(
        &storage,
        &std::sync::Mutex::new(()),
        "main",
        "main-epoch-2",
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("tracked working-tree changes"),
        "unexpected cleanup error: {error}"
    );

    let dirty = std::fs::read_to_string(clone.path().join("f0.txt")).unwrap();
    assert_eq!(dirty, "deferred message content", "dirty file must survive");
    // Residue R' still present (cleanup refused) — fence keeps it unpublished.
    assert!(clone.path().join("gitim.epoch.yaml").exists());
}

#[test]
fn cleanup_preserves_a_rotation_tail_that_mutates_epoch_metadata() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    git(&clone, &["branch", "main-epoch-2", "HEAD"]);
    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 1\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(&clone, &["commit", "-m", "seal: redirect unsafe tail"]);
    let seal = storage.rev_parse("HEAD").unwrap();
    std::fs::write(
        clone.path().join("gitim.epoch.yaml"),
        "epoch: 99\nbranch: main\nstatus: redirected\n",
    )
    .unwrap();
    git(&clone, &["add", "gitim.epoch.yaml"]);
    git(&clone, &["commit", "-m", "unsafe epoch tail"]);
    let tail = storage.rev_parse("HEAD").unwrap();
    let orphan = storage.rev_parse("main-epoch-2").unwrap();

    let error = gitim_sync::rotate::cleanup_failed_fire(
        &storage,
        &std::sync::Mutex::new(()),
        "main",
        "main-epoch-2",
    )
    .unwrap_err();

    assert!(error.to_string().contains("unsafe or unrelated"));
    assert_eq!(storage.rev_parse("HEAD").unwrap(), tail);
    assert_eq!(storage.rev_parse("main-epoch-2").unwrap(), orphan);
    assert!(storage
        .rev_parse(&format!("refs/gitim/rotation-tail/{seal}"))
        .is_err());
    assert!(!clone.path().join(".gitim/rotation-recovery.json").exists());
}

#[test]
fn migrate_conflict_falls_back_to_renumber() {
    // Design scenario 8: B's unpushed message collides with a message already
    // published on the new epoch (same thread file, same line number). The
    // rebase --onto conflict must degrade to content-aware renumber — B's
    // message lands on the new branch with a shifted line number, nothing is
    // lost, and the sync loop converges instead of fence-looping forever.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));
    // A publishes L1 in general.thread on the new epoch.
    commit_file(
        &clone_a,
        "general.thread",
        "[L000001][P000000][@a][20260610T000000Z] first on epoch 2\n",
    );
    git(&clone_a, &["push", "origin", "main-epoch-2"]);

    // B, unaware, writes its own L1 in the same file on sealed main.
    commit_file(
        &clone_b,
        "general.thread",
        "[L000001][P000000][@b][20260610T000001Z] conflicting line\n",
    );

    let storage_b = GitStorage::new(clone_b.path());
    let lock = std::sync::Mutex::new(());
    // Cycle 1: push rejects -> fence (i) -> follow -> migrate CONFLICT ->
    //          renumber fallback commits on the new branch.
    // Cycle 2: pushes from the new branch.
    run_one_sync_cycle(&storage_b, &lock);
    run_one_sync_cycle(&storage_b, &lock);

    assert_eq!(
        head_branch(&clone_b),
        "main-epoch-2",
        "B must have switched"
    );
    git(&clone_b, &["fetch", "origin"]);
    let out = Command::new("git")
        .args(["show", "origin/main-epoch-2:general.thread"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "thread must exist on origin epoch-2");
    let content = String::from_utf8_lossy(&out.stdout);
    assert!(
        content.contains("first on epoch 2"),
        "A's message intact: {content}"
    );
    assert!(
        content.contains("conflicting line"),
        "B's message survived migration: {content}"
    );
    assert!(
        content.contains("[L000002]"),
        "B's message renumbered to L000002: {content}"
    );
}

#[test]
fn fence_fails_closed_on_corrupt_epoch_yaml_without_losing_messages() {
    // Review I1: an unreadable origin epoch.yaml must close the fence
    // (no push), and the degradation path must NOT eat local messages —
    // the discard-then-network-roundtrip window restores on failure.
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);

    // A pushes a CORRUPT epoch.yaml (says redirected, but garbage shape).
    commit_file(
        &clone_a,
        "gitim.epoch.yaml",
        "status: redirected\n:::garbage:::\n",
    );
    git(&clone_a, &["push", "origin", "main"]);

    // B, unaware, writes a message.
    commit_file(
        &clone_b,
        "precious.thread",
        "[L000001][P000000][@b][20260610T000002Z] keep me",
    );

    let storage_b = GitStorage::new(clone_b.path());
    let lock = std::sync::Mutex::new(());
    run_one_sync_cycle(&storage_b, &lock);
    run_one_sync_cycle(&storage_b, &lock);

    // Fail-closed: nothing reached origin/main beyond A's corrupt commit.
    git(&clone_b, &["fetch", "origin"]);
    let remote = Command::new("git")
        .args(["show", "origin/main:precious.thread"])
        .current_dir(clone_b.path())
        .output()
        .unwrap();
    assert!(
        !remote.status.success(),
        "message must NOT publish through a closed fence"
    );

    // Zero-loss: the message commit survives locally (restored after the
    // failed degradation roundtrip), still on main.
    assert_eq!(head_branch(&clone_b), "main");
    assert!(
        clone_b.path().join("precious.thread").exists(),
        "local message must survive the fence cycles"
    );
}
