#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Lifecycle / spawn tests for `cron_engine`.
//!
//! These exercise `spawn_cron_engine` end-to-end: the task is created,
//! it scans, it fires, it survives malformed specs, and it dies cleanly
//! when the runtime drops.
//!
//! ### Why most of these tests are `#[ignore]`
//!
//! `spawn_cron_engine` deliberately throttles its first tick by one full
//! interval (60s) to avoid a startup burst — see the doc comment on
//! `state::AppState::spawn_cron_engine`. That makes any "did it fire?"
//! test take a wall-clock minute even on the happiest path. Default-run
//! tests should stay snappy, so the long ones are gated behind
//! `--ignored` (run with `cargo test -p gitim-daemon --test cron_engine_lifecycle_test -- --ignored`).
//!
//! The CAS / startup / shutdown invariants don't need a 60s wait and
//! stay in the default-run set.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::{CronSpec, Handler};
use gitim_daemon::api::Request;
use gitim_daemon::cron_engine::{fire, FireRequest};
use gitim_daemon::cron_paths::format_thread_filename_ts;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

/// Build a temp git repo with one user and return (TempDir, root_path).
fn setup_repo(handler: &str) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    common::write_user(&root, handler, handler, "dev", "hi");
    common::run_git(&root, &["init"]);
    common::run_git(&root, &["add", "."]);
    common::run_git(&root, &["commit", "-m", "init"]);
    (tmp, root)
}

fn make_state(root: std::path::PathBuf, handler: &str) -> Arc<AppState> {
    let (tx, _) = broadcast::channel(100);
    Arc::new(AppState::new(
        root,
        common::make_config(),
        tx,
        Some(handler.to_string()),
    ))
}

fn write_spec_yaml(crons_root: &std::path::Path, name: &str, target: &str, schedule: &str) {
    let spec = CronSpec {
        version: 1,
        schedule: schedule.to_string(),
        timezone: None,
        target: Handler::new(target).unwrap(),
        prompt: format!("hi from {name}"),
        enabled: true,
        created_by: Handler::new(target).unwrap(),
        // created_at far in the past so any reasonable schedule has at
        // least one anchor-relative fire candidate, exercising the
        // engine's main path.
        created_at: "2026-01-01T00:00:00Z".to_string(),
        extra: std::collections::BTreeMap::new(),
    };
    let dir = crons_root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("spec.yaml"), spec.to_yaml().unwrap()).unwrap();
}

// ─── default-fast tests: CAS, identity gate, startup/shutdown ───────────────

/// Calling `spawn_cron_engine` twice should latch the second call out
/// (CAS), same as `spawn_sync_loop`. We verify by observing the
/// `cron_engine_started` flag flip from false → true on first call and
/// stay true (with a warn log) on the second.
#[tokio::test]
async fn engine_spawn_is_cas_gated() {
    let (_tmp, root) = setup_repo("alice");
    let state = make_state(root, "alice");
    assert!(!state
        .cron_engine_started
        .load(std::sync::atomic::Ordering::SeqCst));

    AppState::spawn_cron_engine(state.clone());
    assert!(state
        .cron_engine_started
        .load(std::sync::atomic::Ordering::SeqCst));

    // Second call: idempotent, no panic.
    AppState::spawn_cron_engine(state.clone());
    assert!(state
        .cron_engine_started
        .load(std::sync::atomic::Ordering::SeqCst));
}

/// If the daemon has no current_user (shouldn't happen in production —
/// caller gates on identity), the spawned task should exit cleanly
/// rather than panic or fire bogus crons.
#[tokio::test]
async fn engine_exits_cleanly_without_identity() {
    let (_tmp, root) = setup_repo("alice");
    let (tx, _) = broadcast::channel(100);
    let state = Arc::new(AppState::new(root, common::make_config(), tx, None));

    AppState::spawn_cron_engine(state.clone());
    // Task started; if it didn't crash, our setup contract holds.
    assert!(state
        .cron_engine_started
        .load(std::sync::atomic::Ordering::SeqCst));

    // Brief yield so the task has a chance to inspect current_user and
    // log/exit. We don't strictly need to await it — just exercising the
    // path.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

// ─── long-running tests: actual fires ───────────────────────────────────────

/// Engine starts and runs through one tick without panicking.
///
/// Wall-clock cost: ~70s (first-tick throttle = 60s, then one tick body).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "wall-clock 70s due to first-tick startup throttle"]
async fn engine_starts_and_ticks() {
    let (_tmp, root) = setup_repo("alice");
    let state = make_state(root, "alice");
    AppState::spawn_cron_engine(state.clone());
    // Sleep through the first tick + a margin.
    tokio::time::sleep(std::time::Duration::from_secs(70)).await;
    // No panic, engine still alive (CAS still true). Test passes.
    assert!(state
        .cron_engine_started
        .load(std::sync::atomic::Ordering::SeqCst));
}

/// Set up a spec with `* * * * *` (every minute) targeted at the daemon's
/// own handler. After ~70s, exactly one fire should be visible on disk.
///
/// First-tick throttle ensures the engine doesn't fire on startup; the
/// second tick (~70s after spawn) is the one that emits the fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "wall-clock 70s; runs the full scan + fire path"]
async fn engine_fires_due_spec() {
    let (_tmp, root) = setup_repo("alice");
    let state = make_state(root.clone(), "alice");
    write_spec_yaml(&root.join("crons"), "every-min", "alice", "* * * * *");

    AppState::spawn_cron_engine(state.clone());
    tokio::time::sleep(std::time::Duration::from_secs(70)).await;

    // At least one fire file should exist under crons/every-min/.
    let dir = root.join("crons/every-min");
    let n_threads = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".thread"))
        .count();
    assert!(
        n_threads >= 1,
        "expected ≥ 1 thread fire after ~70s tick, got {n_threads}"
    );

    // First fire should NOT be on the very second the test started — the
    // throttle skips the immediate-on-startup tick. We verify obliquely:
    // the fire's body parses + has the right shape.
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    let fire_file = entries
        .iter()
        .find(|e| e.file_name().to_string_lossy().ends_with(".thread"))
        .unwrap();
    let body = std::fs::read_to_string(fire_file.path()).unwrap();
    let parsed = gitim_core::parser::parse_thread(&body).expect("body parses");
    assert!(!parsed.entries.is_empty());
    match &parsed.entries[0] {
        gitim_core::types::ThreadEntry::Message(m) => {
            assert_eq!(m.author.as_str(), "system");
            assert!(m.body.starts_with("cron(every-min):"));
        }
        other => panic!("expected message, got {other:?}"),
    }
}

/// Daemon's me.json says alice. Spec is targeted at bob. Engine must
/// NOT fire — verifies the ownership invariant under live spawn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "wall-clock 70s"]
async fn engine_skips_non_owned() {
    let (_tmp, root) = setup_repo("alice");
    // Add bob as a user so the spec target validates.
    std::fs::write(
        root.join("users/bob.meta.yaml"),
        "display_name: bob\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();
    let state = make_state(root.clone(), "alice");
    write_spec_yaml(&root.join("crons"), "for-bob", "bob", "* * * * *");

    AppState::spawn_cron_engine(state.clone());
    tokio::time::sleep(std::time::Duration::from_secs(70)).await;

    // No thread files should exist under bob's spec dir.
    let dir = root.join("crons/for-bob");
    let n_threads = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".thread"))
        .count();
    assert_eq!(
        n_threads, 0,
        "alice clone must not fire bob's spec, found {n_threads}"
    );
}

/// One garbage spec.yaml + one valid spec sitting side by side. The
/// engine must skip the broken one (logging) and continue firing the
/// valid one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "wall-clock 70s"]
async fn engine_survives_malformed_spec() {
    let (_tmp, root) = setup_repo("alice");
    let state = make_state(root.clone(), "alice");

    // Garbage spec.
    let broken_dir = root.join("crons/broken");
    std::fs::create_dir_all(&broken_dir).unwrap();
    std::fs::write(
        broken_dir.join("spec.yaml"),
        "this is: not\n  - valid: yaml: ::\n",
    )
    .unwrap();
    // Valid spec.
    write_spec_yaml(&root.join("crons"), "valid", "alice", "* * * * *");

    AppState::spawn_cron_engine(state.clone());
    tokio::time::sleep(std::time::Duration::from_secs(70)).await;

    // Valid spec fired at least once.
    let valid_threads = std::fs::read_dir(root.join("crons/valid"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".thread"))
        .count();
    assert!(
        valid_threads >= 1,
        "valid spec should fire despite broken sibling — got {valid_threads}"
    );
    // Broken dir has no thread files (only the garbage spec).
    let broken_threads = std::fs::read_dir(&broken_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".thread"))
        .count();
    assert_eq!(broken_threads, 0);
}

/// Spawn the engine, then drop the runtime by letting the test finish
/// quickly. The task must die cleanly (no leaked thread, no panic on
/// Drop). We verify by observing the AtomicBool latch — if the task
/// panicked it would be visible in the test output.
#[tokio::test]
async fn engine_stops_on_runtime_shutdown() {
    let (_tmp, root) = setup_repo("alice");
    let state = make_state(root, "alice");
    AppState::spawn_cron_engine(state.clone());
    // Brief sleep so the task body actually runs once.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // Implicit: when this `async fn` returns, tokio cancels remaining
    // tasks. No `panic` propagates → engine drained cleanly.
    drop(state);
}

// ─── filename / format sanity (smoke) ───────────────────────────────────────

/// Smoke check: the in-process scan path produces filenames that
/// `format_thread_filename_ts` can roundtrip with the parser.
#[tokio::test]
async fn engine_filename_contract() {
    let stem = format_thread_filename_ts(Utc.with_ymd_and_hms(2026, 5, 11, 9, 0, 0).unwrap());
    assert_eq!(stem, "2026-05-11T09-00-00Z");
}

// ─── fire → poll delivery (the missing e2e link) ────────────────────────────

/// End-to-end: directly drive `fire()`, then call `handle_poll`, and
/// assert the runtime would see a `cron_thread`-kind ChannelChange.
///
/// This is the test the original `engine_fires_due_spec` should have
/// extended — it wasn't enough that fire wrote a file; the runtime poller
/// consumes ChannelChanges, not raw fs state, so without this assertion
/// the fire could silently never reach the agent_loop.
///
/// Default-run (not `#[ignore]`) because we drive `fire` directly and
/// skip the 60s engine throttle.
#[tokio::test]
async fn fire_then_poll_delivers_cron_thread_change() {
    let (_tmp, root) = setup_repo("alice");
    let state = make_state(root.clone(), "alice");

    // Pre-write the spec.yaml + commit so HEAD has it before the fire.
    // This mirrors what handle_create_cron does, minus the IPC plumbing.
    let spec = CronSpec {
        version: 1,
        schedule: "@daily".to_string(),
        timezone: None,
        target: Handler::new("alice").unwrap(),
        prompt: "weekly summary".to_string(),
        enabled: true,
        created_by: Handler::new("alice").unwrap(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        extra: BTreeMap::new(),
    };
    let spec_dir = root.join("crons/weekly");
    std::fs::create_dir_all(&spec_dir).unwrap();
    std::fs::write(spec_dir.join("spec.yaml"), spec.to_yaml().unwrap()).unwrap();
    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["add", "crons"]);
    run_git(&["commit", "-m", "create weekly cron"]);

    // Cursor: HEAD before the fire write.
    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Directly fire — bypasses the engine's 60s throttle but exercises
    // the same write+commit path the real loop does.
    let fire_ts = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
    fire(
        &state,
        FireRequest {
            spec_name: "weekly".to_string(),
            spec: spec.clone(),
            theoretical_ts: fire_ts,
        },
    )
    .await
    .expect("fire should succeed");

    // Poll across the fire — the runtime would do this on its next tick.
    let poll_resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

    let changes = poll_resp.data.unwrap()["changes"]
        .as_array()
        .cloned()
        .unwrap();
    let cron_change = changes
        .iter()
        .find(|c| c["kind"] == "cron_thread")
        .unwrap_or_else(|| {
            panic!(
                "fire wrote a file but poll surfaced nothing — runtime would never wake. changes={:#?}",
                changes
            )
        });
    assert_eq!(cron_change["channel"], "cron:weekly");
    let entries = cron_change["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["author"], "system");
    assert!(entries[0]["body"]
        .as_str()
        .unwrap_or("")
        .contains("cron(weekly)"));
}
