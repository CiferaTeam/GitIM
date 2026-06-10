#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_sync::git::GitStorage;
use gitim_sync::rotate::{check_push_fence, follow_redirect, try_fire_rotation, RotationOutcome};
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

    gitim_sync::rotate::cleanup_failed_fire(&storage, "main", "main-epoch-2").unwrap();

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
        &|| {},
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

    gitim_sync::rotate::cleanup_failed_fire(&storage, "main", "main-epoch-2").unwrap();

    let dirty = std::fs::read_to_string(clone.path().join("f0.txt")).unwrap();
    assert_eq!(dirty, "deferred message content", "dirty file must survive");
    // Residue R' still present (cleanup refused) — fence keeps it unpublished.
    assert!(clone.path().join("gitim.epoch.yaml").exists());
}
