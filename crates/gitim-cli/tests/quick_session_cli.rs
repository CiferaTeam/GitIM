#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const ATTEMPT_ID: &str = "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ";

fn fake_clone() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let run_dir = tmp.path().join(".gitim/run");
    fs::create_dir_all(&run_dir).expect("mkdir run dir");
    fs::write(tmp.path().join(".gitim/me.json"), r#"{"handler":"bob"}"#).expect("write me.json");
    fs::write(run_dir.join("gitim.pid"), std::process::id().to_string()).expect("write pid");
    tmp
}

fn gitim() -> Command {
    Command::cargo_bin("gitim").expect("gitim binary")
}

fn serve_once(
    clone: &tempfile::TempDir,
    data: Value,
) -> (thread::JoinHandle<()>, mpsc::Receiver<Value>) {
    let listener =
        UnixListener::bind(clone.path().join(".gitim/run/gitim.sock")).expect("bind daemon socket");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (probe, _) = listener.accept().expect("readiness probe");
        drop(probe);
        let (mut stream, _) = listener.accept().expect("request connection");
        let mut line = String::new();
        BufReader::new(stream.try_clone().expect("clone stream"))
            .read_line(&mut line)
            .expect("read request");
        let _ = sender.send(serde_json::from_str(&line).expect("request json"));
        writeln!(stream, "{}", json!({"ok":true,"data":data})).expect("write response");
    });
    (handle, receiver)
}

#[test]
fn list_accepts_filters_and_prints_human_rows() {
    let clone = fake_clone();
    let (server, request) = serve_once(
        &clone,
        json!({"sessions":[{
            "id":SESSION_ID,
            "title":"Investigate auth",
            "agent_id":"bob",
            "created_by":"alice",
            "status":"active",
            "updated_at":"2026-07-11T00:00:00Z",
            "last_message_preview":"Found the issue",
            "revision":2,
            "archived":false,
            "ref":format!("session:{SESSION_ID}")
        }]}),
    );
    gitim()
        .current_dir(clone.path())
        .args(["session", "list", "--agent", "bob", "--actionable"])
        .assert()
        .success()
        .stdout(predicate::str::contains(SESSION_ID))
        .stdout(predicate::str::contains("Investigate auth"))
        .stdout(predicate::str::contains("@bob"));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"list_quick_sessions","archived":false,"agent_id":"bob","actionable":true})
    );
}

#[test]
fn read_accepts_session_id_and_prints_transcript() {
    let clone = fake_clone();
    let (server, _) = serve_once(
        &clone,
        json!({"session":{
            "meta":{
                "id":SESSION_ID,"title":"Investigate auth",
                "agent_id":"bob","created_by":"alice","status":"active",
                "created_at":"2026-07-11T00:00:00Z","updated_at":"2026-07-11T00:01:00Z",
                "last_message_preview":"reply","revision":2
            },
            "entries":[{"type":"message","line_number":1,"point_to":0,"author":"alice","timestamp":"2026-07-11T00:00:00Z","body":"hello","mentions":[],"links":[]}],
            "archived":false
        }}),
    );
    gitim()
        .current_dir(clone.path())
        .args(["session", "read", SESSION_ID])
        .assert()
        .success()
        .stdout(predicate::str::contains("Investigate auth"))
        .stdout(predicate::str::contains("L000001 @alice: hello"));
    server.join().unwrap();
}

#[test]
fn title_accepts_attempt_and_prints_revision() {
    let clone = fake_clone();
    let (server, request) = serve_once(
        &clone,
        json!({"session_id":SESSION_ID,"title":"Investigate auth","status":"running","revision":2}),
    );
    gitim()
        .current_dir(clone.path())
        .args([
            "session",
            "title",
            SESSION_ID,
            "Investigate auth",
            "--attempt-id",
            ATTEMPT_ID,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("revision=2"));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"set_quick_session_title","session_id":SESSION_ID,"title":"Investigate auth","attempt_id":ATTEMPT_ID})
    );
}

#[test]
fn send_reads_stdin_and_prints_stable_fields() {
    let clone = fake_clone();
    let line_ref = format!("session:{SESSION_ID}:L000002");
    let (server, request) = serve_once(
        &clone,
        json!({"session_id":SESSION_ID,"line_number":2,"status":"active","revision":3,"ref":line_ref}),
    );
    gitim()
        .current_dir(clone.path())
        .write_stdin("agent reply\n")
        .args([
            "session",
            "send",
            SESSION_ID,
            "--stdin",
            "--reply-to",
            "1",
            "--attempt-id",
            ATTEMPT_ID,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(SESSION_ID))
        .stdout(predicate::str::contains("L000002"))
        .stdout(predicate::str::contains("revision=3"))
        .stdout(predicate::str::contains(&line_ref));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"send_quick_session_message","session_id":SESSION_ID,"body":"agent reply\n","reply_to":1,"attempt_id":ATTEMPT_ID})
    );
}

#[test]
fn send_json_outputs_only_response_data() {
    let clone = fake_clone();
    let data = json!({"session_id":SESSION_ID,"line_number":2,"status":"active","revision":3,"ref":format!("session:{SESSION_ID}:L000002")});
    let (server, _) = serve_once(&clone, data.clone());
    let output = gitim()
        .current_dir(clone.path())
        .args([
            "--json",
            "session",
            "send",
            SESSION_ID,
            "reply",
            "--reply-to",
            "1",
            "--attempt-id",
            ATTEMPT_ID,
        ])
        .output()
        .expect("run gitim");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        data
    );
    server.join().unwrap();
}

#[test]
fn summarize_requires_stdin_and_passes_attempt() {
    let clone = fake_clone();
    let (server, request) = serve_once(
        &clone,
        json!({"session_id":SESSION_ID,"summary":"Auth findings\n","status":"running","revision":3}),
    );
    gitim()
        .current_dir(clone.path())
        .write_stdin("Auth findings\n")
        .args([
            "session",
            "summarize",
            SESSION_ID,
            "--stdin",
            "--attempt-id",
            ATTEMPT_ID,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("revision=3"));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"set_quick_session_summary","session_id":SESSION_ID,"summary":"Auth findings\n","attempt_id":ATTEMPT_ID})
    );
}

#[test]
fn send_rejects_missing_turn_coordinates() {
    gitim()
        .args(["session", "send", SESSION_ID, "reply"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--reply-to"));
}

#[test]
fn list_help_describes_archived_only_filter() {
    gitim()
        .args(["session", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "List archived Quick Sessions only",
        ));
}

#[test]
fn list_rejects_archived_with_actionable() {
    gitim()
        .args(["session", "list", "--archived", "--actionable"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--archived"))
        .stderr(predicate::str::contains("--actionable"));
}
