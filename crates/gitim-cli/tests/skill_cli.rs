#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

const REVISION: &str = "r-01K1D8QG2S8RX4T9M9BDKQ9Z7N";
const PROPOSAL: &str = "p-01K1D8QG2S8RX4T9M9BDKQ9Z7N";
const EVENT: &str = "e-01K1D8QG2S8RX4T9M9BDKQ9Z7N";

fn fake_clone() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let run_dir = tmp.path().join(".gitim/run");
    fs::create_dir_all(&run_dir).expect("run dir");
    fs::write(tmp.path().join(".gitim/me.json"), r#"{"handler":"alice"}"#).expect("me.json");
    fs::write(run_dir.join("gitim.pid"), std::process::id().to_string()).expect("pid");
    tmp
}

fn package(clone: &tempfile::TempDir) -> PathBuf {
    let path = clone.path().join("package");
    fs::create_dir_all(path.join("references")).unwrap();
    fs::write(
        path.join("SKILL.md"),
        "---\nname: release-check\ndescription: Verify releases.\n---\n\n# Instructions\n",
    )
    .unwrap();
    fs::write(path.join("references/checklist.md"), "check\n").unwrap();
    path
}

fn gitim() -> Command {
    Command::cargo_bin("gitim").expect("gitim binary")
}

fn serve_once(
    clone: &tempfile::TempDir,
    response: Value,
) -> (thread::JoinHandle<()>, mpsc::Receiver<Value>) {
    let socket = clone.path().join(".gitim/run/gitim.sock");
    if socket.exists() {
        fs::remove_file(&socket).expect("remove old daemon socket");
    }
    let listener = UnixListener::bind(socket).expect("daemon socket");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (probe, _) = listener.accept().expect("readiness probe");
        drop(probe);
        let (mut stream, _) = listener.accept().expect("request");
        let mut line = String::new();
        BufReader::new(stream.try_clone().expect("clone stream"))
            .read_line(&mut line)
            .expect("read request");
        let _ = sender.send(serde_json::from_str(&line).expect("request json"));
        writeln!(stream, "{response}").expect("response");
    });
    (handle, receiver)
}

#[test]
fn root_help_is_progressive_and_task_oriented() {
    gitim()
        .args(["skill", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("load"))
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("propose"))
        .stdout(predicate::str::contains("proposal"))
        .stdout(predicate::str::contains("role"))
        .stdout(predicate::str::contains("admin"))
        .stdout(predicate::str::contains("skill_proposal_publish").not());
}

#[test]
fn write_help_explains_retry_safe_event_ids() {
    gitim()
        .args(["skill", "create", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("idempotency key"))
        .stdout(predicate::str::contains("retry"));
}

#[test]
fn load_accepts_shorthand_and_prints_instructions_with_resource_index() {
    let clone = fake_clone();
    let canonical = format!("skill:release-check@{REVISION}");
    let (server, request) = serve_once(
        &clone,
        json!({"ok":true,"data":{
            "canonical_ref":canonical,
            "revision":{"id":REVISION},
            "skill_markdown":"# Instructions\nDo the checks.\n",
            "resources":[{"path":"references/checklist.md","byte_size":6,"media_type":"text/markdown","text":true}],
            "archived":false
        }}),
    );
    gitim()
        .current_dir(clone.path())
        .args(["skill", "load", &format!("release-check@{REVISION}")])
        .assert()
        .success()
        .stdout(predicate::str::contains(&canonical))
        .stdout(predicate::str::contains("Do the checks."))
        .stdout(predicate::str::contains("references/checklist.md"));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"skill_load","reference":format!("release-check@{REVISION}")})
    );
}

#[test]
fn create_canonicalizes_source_and_sends_optional_event_id() {
    let clone = fake_clone();
    let source = package(&clone);
    let (server, request) = serve_once(
        &clone,
        json!({"ok":true,"data":{
            "event_id":EVENT,
            "revision":REVISION,
            "proposal":null,
            "canonical_ref":format!("skill:release-check@{REVISION}"),
            "current_revision":REVISION,
            "archived":false,
            "commit_id":"abc123",
            "idempotent":false
        }}),
    );
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "create",
            "release-check",
            "--from",
            "package",
            "--display-name",
            "Release Check",
            "--description",
            "Verify releases.",
            "--event-id",
            EVENT,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "skill:release-check@{REVISION}"
        )))
        .stdout(predicate::str::contains(EVENT));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({
            "method":"skill_create",
            "slug":"release-check",
            "source_directory":source.canonicalize().unwrap(),
            "display_name":"Release Check",
            "description":"Verify releases.",
            "event_id":EVENT
        })
    );
}

#[test]
fn validate_infers_slug_from_skill_markdown() {
    let clone = fake_clone();
    let source = package(&clone);
    let (server, request) = serve_once(
        &clone,
        json!({"ok":true,"data":{
            "slug":"release-check","content_sha256":"abc","file_count":2,"resources":[]
        }}),
    );
    gitim()
        .current_dir(clone.path())
        .args(["skill", "validate", "--from", "package"])
        .assert()
        .success()
        .stdout(predicate::str::contains("release-check"))
        .stdout(predicate::str::contains("2 files"));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({
            "method":"skill_validate",
            "slug":"release-check",
            "source_directory":source.canonicalize().unwrap()
        })
    );
}

#[test]
fn binary_resource_requires_output_and_writes_exact_bytes() {
    let clone = fake_clone();
    let reference = format!("release-check@{REVISION}");
    let response = json!({"ok":true,"data":{
        "canonical_ref":format!("skill:release-check@{REVISION}"),
        "path":"assets/blob.bin","media_type":"application/octet-stream",
        "text":false,"content_base64":"AJ+Slg==","archived":false
    }});
    let (server, _) = serve_once(&clone, response.clone());
    gitim()
        .current_dir(clone.path())
        .args(["skill", "resource", &reference, "assets/blob.bin"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output"))
        .stderr(predicate::str::contains("skill_resource_binary"));
    server.join().unwrap();

    let output = clone.path().join("blob.bin");
    let (server, request) = serve_once(&clone, response);
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            &reference,
            "assets/blob.bin",
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("4 bytes"));
    server.join().unwrap();
    assert_eq!(fs::read(output).unwrap(), [0, 159, 146, 150]);
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"skill_resource","reference":reference,"path":"assets/blob.bin"})
    );
}

#[test]
fn json_mode_still_requires_output_for_binary_resources() {
    let clone = fake_clone();
    let reference = format!("release-check@{REVISION}");
    let (server, _) = serve_once(
        &clone,
        json!({"ok":true,"data":{
            "canonical_ref":format!("skill:release-check@{REVISION}"),
            "path":"assets/blob.bin","media_type":"application/octet-stream",
            "text":false,"content_base64":"AJ+Slg==","archived":false
        }}),
    );

    gitim()
        .current_dir(clone.path())
        .args(["--json", "skill", "resource", &reference, "assets/blob.bin"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output"))
        .stderr(predicate::str::contains("skill_resource_binary"));
    server.join().unwrap();
}

#[test]
fn resource_refuses_to_overwrite_without_force() {
    let clone = fake_clone();
    let output = clone.path().join("checklist.md");
    fs::write(&output, "keep").unwrap();
    let (server, _) = serve_once(
        &clone,
        json!({"ok":true,"data":{
            "canonical_ref":format!("skill:release-check@{REVISION}"),
            "path":"references/checklist.md","media_type":"text/markdown",
            "text":true,"content_base64":"bmV3","archived":false
        }}),
    );
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            &format!("release-check@{REVISION}"),
            "references/checklist.md",
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"))
        .stderr(predicate::str::contains("output_exists"));
    server.join().unwrap();
    assert_eq!(fs::read_to_string(output).unwrap(), "keep");
}

#[test]
fn proposal_publish_uses_nested_help_surface() {
    let clone = fake_clone();
    let (server, request) = serve_once(
        &clone,
        json!({"ok":true,"data":{
            "event_id":EVENT,"revision":null,"proposal":PROPOSAL,
            "canonical_ref":format!("skill:release-check@{REVISION}"),
            "current_revision":REVISION,"archived":false,
            "commit_id":"abc","idempotent":false
        }}),
    );
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "proposal",
            "publish",
            "release-check",
            PROPOSAL,
            "--event-id",
            EVENT,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(REVISION));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({
            "method":"skill_proposal_publish","slug":"release-check",
            "proposal":PROPOSAL,"event_id":EVENT
        })
    );
}

#[test]
fn list_json_outputs_only_bounded_response_data() {
    let clone = fake_clone();
    let data = json!({
        "skills":[{"slug":"release-check","display_name":"Release Check","description":"Verify releases.","current_revision":REVISION,"owners":["alice"],"maintainers":["alice"],"open_proposal_count":0,"archived":false,"last_event_id":EVENT}],
        "invalid":[],"next_after":null
    });
    let (server, request) = serve_once(&clone, json!({"ok":true,"data":data.clone()}));
    let output = gitim()
        .current_dir(clone.path())
        .args(["--json", "skill", "list", "--limit", "20"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        data
    );
    assert!(output.stderr.is_empty());
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({"method":"skill_list","archived":false,"limit":20,"after":null})
    );
}

#[test]
fn role_and_admin_commands_parse_without_contacting_a_daemon_for_help() {
    gitim()
        .args(["skill", "role", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("owner-add"))
        .stdout(predicate::str::contains("maintainer-remove"));
    gitim()
        .args(["skill", "admin", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("archive"))
        .stdout(predicate::str::contains("unarchive"));
}
