#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Verifies that timer file content surfaces in the helper layer that
//! agent_loop uses to build the LLM prompt. Goes through the same
//! gitim-core APIs the production agent_loop uses — no provider mock,
//! no daemon. Confirms the file → prompt pipeline contract.

use gitim_core::timer::{
    self, format_fired_for_prompt, pop_fired_timers, register_timer, TimersFile,
};
use std::fs;
use std::time::Duration as StdDuration;

fn fake_clone() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    fs::create_dir_all(tmp.path().join(".gitim")).expect("mkdir .gitim");
    tmp
}

#[test]
fn full_round_trip_set_then_pop_renders_prompt() {
    let clone = fake_clone();
    // Set a timer that's already past (10 seconds is min, but we'll
    // fast-forward `now` past fire_at in pop_fired_timers).
    let t = register_timer(
        clone.path(),
        chrono::Duration::seconds(timer::MIN_DURATION_SECS),
        "<#deploys:L000128>".into(),
        Some("check prod".into()),
    )
    .expect("register");
    // Advance `now` past fire_at.
    let now = t.fire_at + chrono::Duration::seconds(1);
    let fired = pop_fired_timers(clone.path(), now).expect("pop");
    assert_eq!(fired.len(), 1);
    let prompt = format_fired_for_prompt(&fired, now);
    assert!(prompt.contains("⏰ Timer reminder(s) fired"));
    assert!(prompt.contains("<#deploys:L000128>"));
    assert!(prompt.contains("check prod"));

    // Second pop with same now → no more fired.
    let again = pop_fired_timers(clone.path(), now).expect("pop2");
    assert!(again.is_empty());
}

#[test]
fn future_timer_not_yet_fired() {
    let clone = fake_clone();
    let _t = register_timer(
        clone.path(),
        chrono::Duration::seconds(60 * 60),
        "<#x>".into(),
        None,
    )
    .expect("register");
    let fired = pop_fired_timers(clone.path(), chrono::Utc::now()).expect("pop");
    assert!(fired.is_empty());
    let remaining = timer::read_timers(clone.path()).expect("read");
    assert_eq!(remaining.timers.len(), 1);
}

#[test]
fn corrupt_file_returns_no_fired_and_preserves_file() {
    let clone = fake_clone();
    fs::write(
        clone.path().join(".gitim").join("timers.json"),
        "{not json{{",
    )
    .expect("write corrupt");
    let fired = pop_fired_timers(clone.path(), chrono::Utc::now()).expect("pop ok");
    assert!(fired.is_empty());
    let raw = fs::read_to_string(clone.path().join(".gitim").join("timers.json"))
        .expect("file still there");
    assert_eq!(raw, "{not json{{", "corrupt file must be preserved");
}

#[test]
fn cross_restart_backlog_fires_all_at_once() {
    let clone = fake_clone();
    // Manually craft a file with 3 long-past timers (simulating runtime
    // was offline for hours).
    let past = chrono::Utc::now() - chrono::Duration::hours(2);
    let file = TimersFile {
        version: 1,
        timers: (0..3)
            .map(|i| timer::Timer {
                id: format!("20260520T100000-aaaaa{i}"),
                fire_at: past + chrono::Duration::minutes(i as i64),
                created_at: past - chrono::Duration::hours(1),
                anchor: format!("<#x{i}>"),
                note: None,
            })
            .collect(),
    };
    timer::write_timers(clone.path(), &file).expect("write");
    let fired = pop_fired_timers(clone.path(), chrono::Utc::now()).expect("pop");
    assert_eq!(fired.len(), 3, "all backlog timers fire on next pop");
    let prompt = format_fired_for_prompt(&fired, chrono::Utc::now());
    assert!(prompt.contains("1."));
    assert!(prompt.contains("3."));
}

#[test]
fn concurrent_writers_no_lost_update() {
    use std::sync::Arc;
    use std::thread;
    let clone = fake_clone();
    let clone_path: Arc<std::path::PathBuf> = Arc::new(clone.path().to_path_buf());

    // 3 threads, each registers 1 timer (cap is 3, so all should fit
    // if they serialize).
    let mut handles = vec![];
    for i in 0..3 {
        let p = clone_path.clone();
        handles.push(thread::spawn(move || {
            register_timer(
                &p,
                chrono::Duration::seconds(60 * 60),
                format!("<#c{i}>"),
                None,
            )
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        3,
        "all 3 should succeed: {results:?}"
    );

    let final_file = timer::read_timers(&clone_path).expect("read");
    assert_eq!(final_file.timers.len(), 3, "no lost updates");

    // Now a 4th thread tries — should fail with CapReached.
    let p = clone_path.clone();
    let extra = thread::spawn(move || {
        register_timer(&p, chrono::Duration::seconds(60), "<#x>".into(), None)
    })
    .join()
    .unwrap();
    assert!(matches!(extra.unwrap_err(), timer::TimerError::CapReached));

    let _ = StdDuration::from_millis(1); // silence unused import warning
}

#[test]
fn write_failure_after_partial_does_not_corrupt() {
    // Validates that on a write error the timers file either contains the
    // old state or the new state — never a half-written state. We can't
    // induce a real ENOSPC from a unit test, so we approximate by writing
    // happy-path and checking that no `.tmp` siblings remain after.
    let clone = fake_clone();
    register_timer(
        clone.path(),
        chrono::Duration::seconds(60),
        "<#x>".into(),
        None,
    )
    .expect("register");
    let dir = clone.path().join(".gitim");
    let leftover: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let s = e.file_name().to_string_lossy().into_owned();
            // tempfile NamedTempFile default prefix is ".tmp" — match
            // anything between timers.json and not equal to it / lockfile.
            s.contains("timers.json") && s != "timers.json" && s != "timers.json.lock"
        })
        .collect();
    assert!(leftover.is_empty(), "stray temp files: {leftover:?}");
}
