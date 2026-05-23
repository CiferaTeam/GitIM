#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Verify how sync_loop handles non-thread, non-meta files (e.g. cron
//! `crons/<name>/spec.yaml`) on the rebase-conflict path.
//!
//! Context: `sync_loop::sync_with_push` captures `local_additions` via
//! `diff_unpushed("*.thread")` and `local_metas` via
//! `changed_files_unpushed("*.meta.yaml")`. If the rebase fails AND both
//! collections are empty, it currently calls `discard_unpushed()` on the
//! local clone — which `git reset --hard @{upstream}` blows away the
//! local commit. For files outside those globs (cron specs, future
//! protocol additions) this is silent data loss.
//!
//! These tests run against the real `run_sync_cycle` entry point — same
//! code path the daemon hits in production — using a bare repo + two
//! clones, so the bug shows up empirically rather than by code-reading.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use gitim_sync::git::GitStorage;
use gitim_sync::sync_loop::{run_sync_cycle, AuthCircuit};
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
    // Disable GPG signing locally: devs and CI with global `commit.gpgsign=true`
    // would otherwise silently fail every commit in this test (no key in the
    // ephemeral tempdir), the repo would never advance past `git init`, and the
    // assertions below would fail in confusing ways. Repo-local override beats
    // global config.
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

/// Bare repo + two clones with one shared initial commit. Returns
/// `(bare, clone_a, clone_b)`.
fn setup_two_clones() -> (TempDir, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let clone_a = TempDir::new().unwrap();
    let clone_b = TempDir::new().unwrap();

    run_git(bare.path(), &["init", "--bare"]);

    run_git(
        clone_a.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            clone_a.path().to_str().unwrap(),
        ],
    );
    setup_git_config(clone_a.path(), "Alice", "alice@test.com");

    // Initial commit so `@{upstream}` resolves on both clones.
    std::fs::write(clone_a.path().join("README.md"), "init\n").unwrap();
    run_git(clone_a.path(), &["add", "."]);
    run_git(clone_a.path(), &["commit", "-m", "initial"]);
    run_git(clone_a.path(), &["push", "-u", "origin", "HEAD"]);

    run_git(
        clone_b.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            clone_b.path().to_str().unwrap(),
        ],
    );
    setup_git_config(clone_b.path(), "Bob", "bob@test.com");

    (bare, clone_a, clone_b)
}

/// Drive one cycle of the sync loop. Returns whether `on_pushed` fired
/// — proxy for "the cycle succeeded in landing local work on the remote".
fn drive_one_cycle(repo: &GitStorage) -> bool {
    let pushed = Arc::new(AtomicBool::new(false));
    let pushed_clone = pushed.clone();
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag);
    let commit_lock = Mutex::new(());

    let _ = run_sync_cycle(
        repo,
        &mut circuit,
        &commit_lock,
        &move || {
            pushed_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        },
        &|_, _, _| {},
        &|_| {},
        &|| {},
        None,
    );

    pushed.load(std::sync::atomic::Ordering::SeqCst)
}

/// Snapshot the file contents in a clone — used to compare working tree
/// state before and after a sync cycle.
fn read_or_missing(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

// ── Baseline: different non-thread files, no conflict ────────────────
//
// Sanity check: cron specs at different paths must round-trip cleanly.
// If this fails, sync_loop can't even handle the easy non-thread case
// and the bug investigation has bigger scope than just rebase conflicts.

#[test]
fn non_thread_no_conflict_different_files_both_survive() {
    let (bare, clone_a, clone_b) = setup_two_clones();

    let repo_a = GitStorage::new(clone_a.path());
    let repo_b = GitStorage::new(clone_b.path());

    // A creates crons/job-a/spec.yaml + pushes
    std::fs::create_dir_all(clone_a.path().join("crons/job-a")).unwrap();
    std::fs::write(
        clone_a.path().join("crons/job-a/spec.yaml"),
        "schedule: '@daily'\ntarget: alice\n",
    )
    .unwrap();
    repo_a
        .add_and_commit(&["crons/job-a/spec.yaml"], "cron: create job-a")
        .unwrap();
    repo_a.push().unwrap();

    // B creates crons/job-b/spec.yaml locally — no remote conflict
    std::fs::create_dir_all(clone_b.path().join("crons/job-b")).unwrap();
    std::fs::write(
        clone_b.path().join("crons/job-b/spec.yaml"),
        "schedule: '@hourly'\ntarget: bob\n",
    )
    .unwrap();
    repo_b
        .add_and_commit(&["crons/job-b/spec.yaml"], "cron: create job-b")
        .unwrap();

    // Drive the real sync_loop cycle.
    let pushed = drive_one_cycle(&repo_b);
    assert!(pushed, "B should have pushed cleanly when no conflict");

    // B's working tree should now have BOTH specs (A pulled via rebase,
    // B's local commit then pushed).
    assert_eq!(
        read_or_missing(&clone_b.path().join("crons/job-a/spec.yaml")).as_deref(),
        Some("schedule: '@daily'\ntarget: alice\n"),
        "A's spec must be present on B after pull-rebase",
    );
    assert_eq!(
        read_or_missing(&clone_b.path().join("crons/job-b/spec.yaml")).as_deref(),
        Some("schedule: '@hourly'\ntarget: bob\n"),
        "B's spec must remain on B after sync",
    );

    // Remote (verify by re-cloning bare) should have both as well.
    let verify = TempDir::new().unwrap();
    run_git(
        verify.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            verify.path().to_str().unwrap(),
        ],
    );
    assert!(verify.path().join("crons/job-a/spec.yaml").exists());
    assert!(verify.path().join("crons/job-b/spec.yaml").exists());
}

// ── Bug exposure: same non-thread file conflict ──────────────────────
//
// Both clones modify the same `crons/foo/spec.yaml`. Clone A pushes
// first; Clone B's sync cycle then runs against an ahead remote.
//
// Acceptance per Task 0.1:
//   (a) conflict resolves cleanly (one side wins, that content lives in
//       both B's working tree and the remote), OR
//   (b) sync_loop returns/logs a clear error AND B's local commit
//       survives so the user can do something about it.
//
// Anything else — particularly silently dropping B's local commit so
// only A's content remains and the user has no idea — is THE BUG. The
// test FAILS in that case.

#[test]
fn non_thread_conflict_same_file_does_not_silently_drop_local() {
    let (bare, clone_a, clone_b) = setup_two_clones();

    let repo_a = GitStorage::new(clone_a.path());
    let repo_b = GitStorage::new(clone_b.path());

    // Step 1: create the spec on both clones from a shared base. We
    // commit + push it from A so B starts with the same file present.
    std::fs::create_dir_all(clone_a.path().join("crons/foo")).unwrap();
    std::fs::write(
        clone_a.path().join("crons/foo/spec.yaml"),
        "schedule: '@daily'\ntarget: alice\nprompt: base\n",
    )
    .unwrap();
    repo_a
        .add_and_commit(&["crons/foo/spec.yaml"], "cron: create foo")
        .unwrap();
    repo_a.push().unwrap();
    repo_b.fetch().unwrap();
    repo_b.rebase_onto_origin().unwrap();
    assert!(clone_b.path().join("crons/foo/spec.yaml").exists());

    // Step 2: A modifies the spec — different prompt — and pushes.
    let a_content = "schedule: '@daily'\ntarget: alice\nprompt: ALICE WINS\n";
    std::fs::write(clone_a.path().join("crons/foo/spec.yaml"), a_content).unwrap();
    repo_a
        .add_and_commit(&["crons/foo/spec.yaml"], "cron: edit foo from A")
        .unwrap();
    repo_a.push().unwrap();

    // Step 3: B modifies the SAME spec with different content — but
    // does NOT pull first. Local commit lands; B is now divergent.
    let b_content = "schedule: '@daily'\ntarget: alice\nprompt: BOB WINS\n";
    std::fs::write(clone_b.path().join("crons/foo/spec.yaml"), b_content).unwrap();
    repo_b
        .add_and_commit(&["crons/foo/spec.yaml"], "cron: edit foo from B")
        .unwrap();
    let b_head_before_sync = repo_b.rev_parse("HEAD").unwrap();

    // Step 4: Drive the actual sync_loop cycle — same code path the
    // daemon runs in production. We don't expect this to push (rebase
    // will fail), so `on_pushed` will not fire.
    let pushed = drive_one_cycle(&repo_b);

    // Step 5: Inspect what survived. The bug is: B's local commit got
    // silently dropped. We accept three good outcomes; reject the bad.

    let working_tree_after = read_or_missing(&clone_b.path().join("crons/foo/spec.yaml"));
    let head_after_sync = repo_b.rev_parse("HEAD").unwrap();
    let still_unpushed = repo_b.has_unpushed_commits().unwrap_or(false);

    // Sanity: file still exists at all (catastrophic loss check).
    let after = working_tree_after.expect("spec.yaml should not have been deleted entirely");

    let b_wins_in_tree = after == b_content;
    let a_wins_in_tree = after == a_content;
    let b_local_commit_preserved = head_after_sync == b_head_before_sync && still_unpushed;

    // Three acceptable outcomes. Each is a way the system can be safe:
    //  (i)  conflict resolved with a winning side — that side present
    //       in working tree, no further user action required.
    //  (ii) cycle bailed out and left B's local commit intact, so the
    //       next cycle (or the user) can deal with it.
    //  (iii) push happened (rebase + resolution succeeded) — handled
    //       implicitly by (i) since the push wouldn't change the file.

    let safe_outcome = if pushed {
        // The cycle reported success. Then either A's or B's content
        // must be on the remote AND on B's disk, and both clones must
        // agree once we re-clone the remote.
        let verify = TempDir::new().unwrap();
        run_git(
            verify.path().parent().unwrap(),
            &[
                "clone",
                bare.path().to_str().unwrap(),
                verify.path().to_str().unwrap(),
            ],
        );
        let remote_after =
            std::fs::read_to_string(verify.path().join("crons/foo/spec.yaml")).unwrap();
        // Tree and remote must agree on the same winning content.
        remote_after == after && (b_wins_in_tree || a_wins_in_tree)
    } else {
        // No push happened. We require that B's local commit was NOT
        // silently destroyed: either it's still HEAD (cycle bailed
        // out), or the cycle resolved the conflict locally and left a
        // new commit ready for next cycle to push.
        b_local_commit_preserved
            || (!still_unpushed && b_wins_in_tree)
            || head_after_sync != b_head_before_sync
    };

    // The smoking-gun bug is: pushed=false, B's commit gone (HEAD
    // matches A's HEAD), and only A's content is in the working tree.
    // That's silent data loss with no signal to the caller.
    let silent_drop = !pushed
        && !still_unpushed
        && head_after_sync != b_head_before_sync
        && a_wins_in_tree
        && !b_wins_in_tree;

    assert!(
        !silent_drop,
        "BUG: sync silently dropped B's local commit — \
         HEAD before={b_head_before_sync}, HEAD after={head_after_sync}, \
         working tree={after:?}, pushed={pushed}, still_unpushed={still_unpushed}",
    );

    assert!(
        safe_outcome,
        "Unexpected sync outcome — expected either (a) successful push with \
         a clear winner mirrored on remote, or (b) bailed-out cycle that \
         preserved B's local commit. Got: pushed={pushed}, \
         head_changed={}, still_unpushed={still_unpushed}, \
         a_wins_in_tree={a_wins_in_tree}, b_wins_in_tree={b_wins_in_tree}, \
         working tree={after:?}",
        head_after_sync != b_head_before_sync,
    );
}

// ── Mixed commit: thread file + non-thread file in ONE commit ────────
//
// Single commit on B touches both a `.thread` file (resolvable) AND a
// `crons/<name>/spec.yaml` (non-thread, non-meta — UNresolvable). A
// pushes a conflicting append on the same thread first.
//
// Pre-fix behaviour (the bug): the rebase fails on the thread side; the
// guard runs `changed_files_unpushed_all()` AFTER the rebase has already
// failed, which on a partial-rebase HEAD can return wrong/empty results;
// `local_additions` from before the rebase is non-empty (the thread side)
// so the resolvable path runs; `discard_unpushed` does
// `git reset --hard @{upstream}` and silently destroys the spec.yaml side
// of the same commit.
//
// Post-fix: the unpushed file list is captured BEFORE the rebase. After
// the rebase fails, the cached list still contains the cron spec; the
// presence of any non-thread non-meta file forces the bail path
// (abort_rebase + warn), and B's local commit is preserved intact.

#[test]
fn mixed_commit_with_thread_conflict_preserves_non_thread_change() {
    let (_bare, clone_a, clone_b) = setup_two_clones();

    let repo_a = GitStorage::new(clone_a.path());
    let repo_b = GitStorage::new(clone_b.path());

    let thread_rel = "channels/general.thread";
    let cron_spec_rel = "crons/weekly/spec.yaml";

    // Step 1: A creates the thread with one message + pushes; B catches up.
    let base_thread = "[L000001][P000000][@alice][20260317T100000Z] base from alice\n";
    std::fs::create_dir_all(clone_a.path().join("channels")).unwrap();
    std::fs::write(clone_a.path().join(thread_rel), base_thread).unwrap();
    repo_a
        .add_and_commit(&[thread_rel], "thread: create with base")
        .unwrap();
    repo_a.push().unwrap();
    repo_b.fetch().unwrap();
    repo_b.rebase_onto_origin().unwrap();
    assert!(clone_b.path().join(thread_rel).exists());

    // Step 2: A appends a message to the thread and pushes. From B's
    // perspective the remote is now ahead by one commit on this file.
    let a_thread = "[L000001][P000000][@alice][20260317T100000Z] base from alice\n\
                    [L000002][P000001][@alice][20260317T100100Z] alice follow-up\n";
    std::fs::write(clone_a.path().join(thread_rel), a_thread).unwrap();
    repo_a
        .add_and_commit(&[thread_rel], "thread: alice follow-up")
        .unwrap();
    repo_a.push().unwrap();

    // Step 3: B makes a SINGLE commit that touches both the thread (L2
    // collides with A's append, so rebase will conflict) AND a brand-new
    // crons/weekly/spec.yaml (non-thread non-meta — what the fix is for).
    let b_thread = "[L000001][P000000][@alice][20260317T100000Z] base from alice\n\
                    [L000002][P000001][@bob][20260317T100200Z] bob append at same line\n";
    std::fs::write(clone_b.path().join(thread_rel), b_thread).unwrap();

    let bob_spec = "version: 1\n\
                    schedule: '@weekly'\n\
                    target: bob\n\
                    prompt: weekly digest\n\
                    created_by: bob\n\
                    created_at: '2026-05-09T12:00:00Z'\n";
    std::fs::create_dir_all(clone_b.path().join("crons/weekly")).unwrap();
    std::fs::write(clone_b.path().join(cron_spec_rel), bob_spec).unwrap();

    repo_b
        .add_and_commit(
            &[thread_rel, cron_spec_rel],
            "mixed: thread append + new cron spec",
        )
        .unwrap();
    let b_head_before_sync = repo_b.rev_parse("HEAD").unwrap();

    // Step 4: drive sync. Rebase will fail on the thread; the fix's
    // capture-before-rebase guard must see the cron spec in the unpushed
    // set and bail (preserving B's local commit) instead of falling
    // through to discard_unpushed.
    let _pushed = drive_one_cycle(&repo_b);

    // Step 5: assert B's spec.yaml is still on disk with B's content. Two
    // safe shapes:
    //   (a) Cycle bailed: B's commit still HEAD, both files intact.
    //   (b) Cycle resolved + pushed: spec.yaml content present on disk
    //       and on remote unchanged (only the thread side could differ).
    let spec_after = read_or_missing(&clone_b.path().join(cron_spec_rel));
    assert_eq!(
        spec_after.as_deref(),
        Some(bob_spec),
        "BUG: B's cron spec.yaml was silently dropped during mixed-commit \
         rebase conflict (this is the data-loss scenario the pre-rebase \
         capture exists to prevent)",
    );

    // The local commit must survive in some shape. We accept either:
    // - HEAD unchanged (bail path), still_unpushed=true
    // - HEAD changed but spec preserved (fix kept the non-thread side
    //   even after re-applying)
    // The smoking-gun bug is HEAD reset to upstream AND no unpushed
    // commits AND spec absent — assert against that explicitly.
    let head_after = repo_b.rev_parse("HEAD").unwrap();
    let still_unpushed = repo_b.has_unpushed_commits().unwrap_or(false);
    let smoking_gun = head_after != b_head_before_sync && !still_unpushed && spec_after.is_none();
    assert!(
        !smoking_gun,
        "BUG: B's mixed commit was silently dropped — HEAD before={}, \
         HEAD after={}, still_unpushed={}, spec present={}",
        b_head_before_sync,
        head_after,
        still_unpushed,
        spec_after.is_some(),
    );
}
