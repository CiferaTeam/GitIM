#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// End-to-end tests for `gitim timer set | list | cancel`.
//
// Each test gets its own tempdir with a fake clone layout (a `.gitim/`
// directory + a stub `me.json`). The timer subcommand is pure-fs and
// never contacts the daemon, so no socket / process plumbing is needed
// — `assert_cmd` invokes the built `gitim` binary directly.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

fn fake_clone() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let gitim = tmp.path().join(".gitim");
    fs::create_dir_all(&gitim).expect("mkdir .gitim");
    fs::write(gitim.join("me.json"), r#"{"handler":"alice"}"#).expect("write me.json");
    tmp
}

fn gitim() -> Command {
    Command::cargo_bin("gitim").expect("gitim binary")
}

#[test]
fn set_then_list_shows_entry() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fires in"));

    gitim()
        .current_dir(clone.path())
        .args(["timer", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<#x>"));
}

#[test]
fn set_with_note() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>", "--note", "hello world"])
        .assert()
        .success();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));
}

#[test]
fn cap_enforced_at_4th_set() {
    let clone = fake_clone();
    for _ in 0..3 {
        gitim()
            .current_dir(clone.path())
            .args(["timer", "set", "30m", "<#x>"])
            .assert()
            .success();
    }
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cap"));
}

#[test]
fn cancel_by_full_id() {
    let clone = fake_clone();
    let out = gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .output()
        .expect("set");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let id = stdout.split_whitespace().next().expect("id").to_string();

    gitim()
        .current_dir(clone.path())
        .args(["timer", "cancel", &id])
        .assert()
        .success()
        .stdout(predicate::str::contains(&id));
}

#[test]
fn cancel_no_match_exits_2() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "cancel", "nonexistent-xyz"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("no timer matches"));
}

#[test]
fn cancel_ambiguous_prefix_exits_2() {
    let clone = fake_clone();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let out = gitim()
            .current_dir(clone.path())
            .args(["timer", "set", "30m", "<#x>"])
            .output()
            .expect("set");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let id = stdout.split_whitespace().next().expect("id").to_string();
        ids.push(id);
    }
    // Derive the longest common prefix from the actual ids — clock-stable
    // (no calendar dependency, no year-boundary brittleness).
    let prefix = longest_common_prefix(&ids[0], &ids[1]);
    assert!(
        prefix.len() >= 8,
        "expected ids registered in the same second to share ≥8 chars; got {prefix:?}"
    );
    gitim()
        .current_dir(clone.path())
        .args(["timer", "cancel", &prefix])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("matches 2 timers"));
}

fn longest_common_prefix(a: &str, b: &str) -> String {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
}

#[test]
fn duration_too_short_exits_2() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "5s", "<#x>"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid duration"));
}

#[test]
fn not_in_clone_exits_2() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    // No .gitim/ directory here.
    gitim()
        .current_dir(tmp.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("not in a gitim agent clone"));
}

#[test]
fn list_empty() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(no pending timers)"));
}

#[test]
fn list_json_outputs_array() {
    let clone = fake_clone();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "set", "30m", "<#x>"])
        .assert()
        .success();
    gitim()
        .current_dir(clone.path())
        .args(["timer", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("["));
}
