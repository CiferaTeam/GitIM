#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for sync_loop auth circuit breaker.
//!
//! Strategy: feed the `AuthCircuit` the same `Result<(), GitError>` values
//! the real push/fetch would produce, and verify the state machine latches
//! the shared `Arc<AtomicBool>` at exactly the threshold. This tests the
//! circuit itself end-to-end (no mocking of the circuit's internals) while
//! avoiding the fragility of staging a live GitHub auth failure in CI.
//!
//! Also includes one real-git integration test that runs `run_sync_cycle`
//! against a real repo with an unreachable remote — validating that
//! non-auth failures do NOT trip the circuit (guard against false positives
//! from network errors / missing remotes).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gitim_sync::git::{GitError, GitStorage};
use gitim_sync::sync_loop::{
    run_sync_cycle, AuthCircuit, SyncOutcome, AUTH_FAILURE_TRIP_THRESHOLD, AUTH_PROBE_INTERVAL,
};
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

fn auth_err() -> GitError {
    GitError::AuthFailed("fatal: Authentication failed for 'https://github.com/x/y.git/'".into())
}

// ── AuthCircuit state machine ────────────────────────────────────

#[test]
fn circuit_trips_after_threshold_consecutive_auth_failures() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());

    assert_eq!(AUTH_FAILURE_TRIP_THRESHOLD, 3, "test assumes threshold 3");

    // First 2 failures: counter grows, flag stays down.
    assert!(!circuit.record(&Err(auth_err())));
    assert!(!flag.load(Ordering::SeqCst));
    assert!(!circuit.record(&Err(auth_err())));
    assert!(!flag.load(Ordering::SeqCst));

    // Third failure: transition to tripped, returns true exactly once.
    let tripped_now = circuit.record(&Err(auth_err()));
    assert!(tripped_now, "third auth failure should transition circuit");
    assert!(flag.load(Ordering::SeqCst));

    // Subsequent auth failures: flag stays latched, but record returns false
    // (no new transition) so caller doesn't log repeatedly.
    assert!(!circuit.record(&Err(auth_err())));
    assert!(flag.load(Ordering::SeqCst));
}

#[test]
fn circuit_resets_counter_on_success() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());

    circuit.record(&Err(auth_err()));
    circuit.record(&Err(auth_err()));
    // Two auth failures accumulated but below threshold.
    assert!(!flag.load(Ordering::SeqCst));

    // Success wipes the slate.
    circuit.record(&Ok(()));
    assert!(!flag.load(Ordering::SeqCst));

    // Need THREE more failures (not one) to trip — proves counter reset.
    assert!(!circuit.record(&Err(auth_err())));
    assert!(!circuit.record(&Err(auth_err())));
    assert!(!flag.load(Ordering::SeqCst), "should not trip yet");
    assert!(circuit.record(&Err(auth_err())));
    assert!(flag.load(Ordering::SeqCst));
}

#[test]
fn circuit_ignores_non_auth_errors() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());

    // Rate-limit, push-conflict, command-failed: none count toward auth budget.
    // Even 10 of these don't trip — only AuthFailed does.
    for _ in 0..10 {
        circuit.record(&Err(GitError::RateLimited));
        circuit.record(&Err(GitError::PushConflict));
        circuit.record(&Err(GitError::CommandFailed("fatal: something".into())));
    }
    assert!(!flag.load(Ordering::SeqCst));

    // An auth-then-non-auth-then-auth pair still accumulates toward threshold —
    // non-auth errors are neutral, not a reset. This is intentional: a single
    // transient network blip shouldn't mask credential decay.
    circuit.record(&Err(auth_err()));
    circuit.record(&Err(GitError::RateLimited));
    circuit.record(&Err(auth_err()));
    assert!(
        !flag.load(Ordering::SeqCst),
        "two auth failures still below threshold"
    );

    circuit.record(&Err(auth_err()));
    assert!(flag.load(Ordering::SeqCst));
}

#[test]
fn circuit_flag_shared_across_clones() {
    // daemon AppState clones the Arc<AtomicBool> to give readers (status API)
    // a handle. Verify a trip in one clone is visible from another.
    let flag_a = Arc::new(AtomicBool::new(false));
    let flag_b = flag_a.clone();
    let mut circuit = AuthCircuit::new(flag_a);

    for _ in 0..AUTH_FAILURE_TRIP_THRESHOLD {
        circuit.record(&Err(auth_err()));
    }

    assert!(flag_b.load(Ordering::SeqCst), "daemon-side clone sees trip");
}

// ── AuthCircuit half-open recovery ───────────────────────────────

#[test]
fn circuit_does_not_probe_immediately_after_trip() {
    // A freshly-tripped circuit must NOT probe right away — probing instantly
    // would hammer a remote that just rejected us. The half-open window only
    // opens after AUTH_PROBE_INTERVAL.
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());
    for _ in 0..AUTH_FAILURE_TRIP_THRESHOLD {
        circuit.record(&Err(auth_err()));
    }
    assert!(circuit.is_tripped());
    assert!(
        !circuit.should_attempt_probe(Instant::now()),
        "must not probe in the same instant it tripped"
    );
}

#[test]
fn circuit_allows_probe_after_interval() {
    // Once AUTH_PROBE_INTERVAL has elapsed since the trip, the circuit goes
    // half-open: one cycle is allowed to attempt git again.
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());
    for _ in 0..AUTH_FAILURE_TRIP_THRESHOLD {
        circuit.record(&Err(auth_err()));
    }
    let after = Instant::now() + AUTH_PROBE_INTERVAL + Duration::from_secs(1);
    assert!(
        circuit.should_attempt_probe(after),
        "should allow a probe once the interval elapsed"
    );
}

#[test]
fn circuit_recovers_latch_on_successful_probe() {
    // The whole point of half-open: a successful op (the probe) clears the
    // latch so sync resumes WITHOUT a daemon restart. This is the behaviour
    // the old pure-latch design lacked.
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());
    for _ in 0..AUTH_FAILURE_TRIP_THRESHOLD {
        circuit.record(&Err(auth_err()));
    }
    assert!(flag.load(Ordering::SeqCst), "precondition: tripped");

    circuit.record(&Ok(()));

    assert!(
        !flag.load(Ordering::SeqCst),
        "a successful probe must clear the shared latch"
    );
    assert!(!circuit.is_tripped());
}

#[test]
fn circuit_failed_probe_keeps_latch_and_backs_off() {
    // A failed probe must re-arm the latch and reset the timer so the next
    // probe is another full interval away — not every cycle.
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());
    for _ in 0..AUTH_FAILURE_TRIP_THRESHOLD {
        circuit.record(&Err(auth_err()));
    }

    let t1 = Instant::now() + AUTH_PROBE_INTERVAL + Duration::from_secs(1);
    assert!(circuit.should_attempt_probe(t1));

    // Probe fires at t1 but fails.
    circuit.mark_probe(t1);
    circuit.record(&Err(auth_err()));

    assert!(circuit.is_tripped(), "failed probe keeps the latch set");
    assert!(
        !circuit.should_attempt_probe(t1),
        "must not re-probe immediately after a failed probe"
    );
    assert!(
        circuit.should_attempt_probe(t1 + AUTH_PROBE_INTERVAL + Duration::from_secs(1)),
        "next probe only after another full interval"
    );
}

// ── run_sync_cycle integration ───────────────────────────────────

/// Create a clone whose remote points at a non-existent local path.
/// Push and fetch will fail with CommandFailed (not AuthFailed) — critical
/// for asserting the circuit doesn't false-trip on plain network/config errors.
fn setup_clone_with_dead_remote() -> (TempDir, TempDir, GitStorage) {
    let bare_dir = TempDir::new().unwrap();
    let clone_dir = TempDir::new().unwrap();

    run_git(bare_dir.path(), &["init", "--bare"]);
    run_git(
        clone_dir.path().parent().unwrap(),
        &[
            "clone",
            bare_dir.path().to_str().unwrap(),
            clone_dir.path().to_str().unwrap(),
        ],
    );
    run_git(clone_dir.path(), &["config", "user.email", "t@t.com"]);
    run_git(clone_dir.path(), &["config", "user.name", "T"]);
    std::fs::write(clone_dir.path().join("init.txt"), "init").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "initial"]);
    run_git(clone_dir.path(), &["push", "-u", "origin", "HEAD"]);

    // Now point origin at a path that doesn't exist. Real push → CommandFailed.
    let dead_path = format!(
        "file://{}/nonexistent-bare.git",
        bare_dir.path().parent().unwrap().display()
    );
    run_git(
        clone_dir.path(),
        &["remote", "set-url", "origin", &dead_path],
    );

    // Create an unpushed commit so sync_with_push gets exercised.
    std::fs::write(clone_dir.path().join("local.txt"), "data").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "local"]);

    let repo = GitStorage::new(clone_dir.path());
    (bare_dir, clone_dir, repo)
}

#[test]
fn run_sync_cycle_does_not_trip_circuit_on_non_auth_errors() {
    let (_bare, _clone, repo) = setup_clone_with_dead_remote();
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());

    // 10 cycles of real push failures (all CommandFailed, not auth).
    // The circuit MUST NOT trip — this is the guard against false positives.
    let commit_lock = Mutex::new(());
    for _ in 0..10 {
        let _outcome = run_sync_cycle(
            &repo,
            &mut circuit,
            &commit_lock,
            &|| {},
            &|_, _, _| {},
            &|_| {},
            &|| {},
            None,
        );
    }

    assert!(
        !flag.load(Ordering::SeqCst),
        "non-auth errors must not trip circuit"
    );
    let _ = circuit; // suppress unused warning (state inspected only via flag)
}

#[test]
fn run_sync_cycle_probes_when_tripped_flag_lacks_trip_time() {
    // A tripped flag with no recorded trip time (set externally, or restored
    // from a prior state) is probe-eligible immediately — the half-open circuit
    // attempts git rather than idling forever on an unexplained latch. Here the
    // dead remote makes the probe fail with a NON-auth error, so the cycle does
    // not short-circuit. ("Just tripped → short-circuit" is covered by
    // end_to_end_trip_then_skip_git, where tripped_at is recent.)
    let (_bare, _clone, repo) = setup_clone_with_dead_remote();
    let flag = Arc::new(AtomicBool::new(true));
    let mut circuit = AuthCircuit::new(flag);

    let commit_lock = Mutex::new(());
    let outcome = run_sync_cycle(
        &repo,
        &mut circuit,
        &commit_lock,
        &|| {},
        &|_, _, _| {},
        &|_| {},
        &|| {},
        None,
    );

    assert!(
        !matches!(outcome, SyncOutcome::AuthCircuitOpen),
        "an unexplained latch should go half-open and attempt a probe, not idle"
    );
}

#[test]
fn end_to_end_trip_then_skip_git() {
    // Glue the state machine and cycle together:
    // (1) drive 3 auth failures into the circuit → flag latches
    // (2) run_sync_cycle against a real repo sees the flag → short-circuits
    // This is the intent of the feature: PAT revoke → sync stops hitting remote.
    let (_bare, _clone, repo) = setup_clone_with_dead_remote();
    let flag = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(flag.clone());

    for _ in 0..AUTH_FAILURE_TRIP_THRESHOLD {
        circuit.record(&Err(auth_err()));
    }
    assert!(flag.load(Ordering::SeqCst), "circuit should be tripped");

    // Track whether on_cycle_done ran (it should, even in short-circuit path —
    // pending_push waiters still need to be notified).
    let cycle_done = Arc::new(AtomicBool::new(false));
    let cycle_done_clone = cycle_done.clone();

    let commit_lock = Mutex::new(());
    let outcome = run_sync_cycle(
        &repo,
        &mut circuit,
        &commit_lock,
        &|| panic!("on_pushed must not fire when circuit is open"),
        &|_, _, _| panic!("on_renumbered must not fire when circuit is open"),
        &|_| panic!("on_synced must not fire when circuit is open"),
        &move || {
            cycle_done_clone.store(true, Ordering::SeqCst);
        },
        None,
    );

    assert!(matches!(outcome, SyncOutcome::AuthCircuitOpen));
    assert!(
        cycle_done.load(Ordering::SeqCst),
        "on_cycle_done must still fire so pending_push waiters are notified"
    );
}
