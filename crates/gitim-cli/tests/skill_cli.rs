#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

const REVISION: &str = "r-01K1D8QG2S8RX4T9M9BDKQ9Z7N";
const REQUEST: &str = "q-01K1D8QG2S8RX4T9M9BDKQ9Z7N";
const PROPOSAL: &str = "p-01K1D8QG2S8RX4T9M9BDKQ9Z7N";

fn gitim() -> Command {
    Command::cargo_bin("gitim").expect("gitim binary")
}

fn fake_clone() -> tempfile::TempDir {
    let clone = tempfile::TempDir::new().expect("tempdir");
    fs::create_dir_all(clone.path().join(".gitim/run")).expect("run directory");
    fs::write(
        clone.path().join(".gitim/me.json"),
        r#"{"handler":"alice"}"#,
    )
    .expect("me.json");
    clone
}

fn package(root: &Path) -> std::path::PathBuf {
    let source = root.join("package");
    fs::create_dir_all(&source).expect("package directory");
    fs::write(
        source.join("SKILL.md"),
        "---\nname: release-check\ndescription: Verify releases.\n---\n\n# Release check\n",
    )
    .expect("SKILL.md");
    source
}

fn serve_once(
    clone: &tempfile::TempDir,
    response: Value,
) -> (thread::JoinHandle<()>, mpsc::Receiver<Value>) {
    serve_once_raw(clone, format!("{}\n", json!({"ok":true,"data":response})))
}

fn serve_once_raw(
    clone: &tempfile::TempDir,
    response: String,
) -> (thread::JoinHandle<()>, mpsc::Receiver<Value>) {
    let socket = clone.path().join(".gitim/run/gitim.sock");
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(socket).expect("bind socket");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, line) = loop {
            let (stream, _) = listener.accept().expect("daemon connection");
            let mut line = String::new();
            BufReader::new(stream.try_clone().expect("clone request stream"))
                .read_line(&mut line)
                .expect("read request");
            if !line.is_empty() {
                break (stream, line);
            }
        };
        let _ = sender.send(serde_json::from_str(&line).expect("request json"));
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    (handle, receiver)
}

fn serve_once_without_response(
    clone: &tempfile::TempDir,
) -> (thread::JoinHandle<()>, mpsc::Receiver<Value>) {
    let socket = clone.path().join(".gitim/run/gitim.sock");
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(socket).expect("bind socket");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || loop {
        let (stream, _) = listener.accept().expect("daemon connection");
        let mut line = String::new();
        BufReader::new(stream.try_clone().expect("clone request stream"))
            .read_line(&mut line)
            .expect("read request");
        if !line.is_empty() {
            let _ = sender.send(serde_json::from_str(&line).expect("request json"));
            break;
        }
    });
    (handle, receiver)
}

fn mutation_response() -> Value {
    json!({
        "commit_id":"abc123",
        "result":{
            "canonical_ref":{"slug":"release-check","revision":REVISION},
            "current_revision":REVISION,
            "control_revision":1,
            "event_revision":1
        },
        "local_state":"integrated"
    })
}

fn load_response(resource_count: usize) -> Value {
    let resources = (0..resource_count)
        .map(|index| {
            json!({
                "path":format!("references/{index}.md"),
                "byte_size":index + 1,
                "media_type":"text/markdown",
                "text":true
            })
        })
        .collect::<Vec<_>>();
    json!({
        "canonical_ref":{"slug":"release-check","revision":REVISION},
        "revision":{
            "schema_version":1,
            "id":REVISION,
            "skill":"release-check",
            "content_sha256":"a".repeat(64),
            "resources":resources,
            "created_by":"alice",
            "created_at":"2026-07-31T00:00:00Z"
        },
        "skill_markdown":"# Release check\n",
        "resources":resources,
        "archived":false
    })
}

#[test]
fn root_help_is_grouped_into_five_tasks() {
    gitim()
        .args(["skill", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gitim skill <COMMAND>"))
        .stdout(predicate::str::contains("Use a Skill:"))
        .stdout(predicate::str::contains("Create or improve:"))
        .stdout(predicate::str::contains("Inspect details:"))
        .stdout(predicate::str::contains("Review changes:"))
        .stdout(predicate::str::contains("Administer:"));
}

#[test]
fn nested_help_exposes_the_complete_command_sets() {
    let proposal = gitim()
        .args(["skill", "proposal", "--help"])
        .output()
        .expect("proposal help");
    assert!(proposal.status.success());
    let proposal = String::from_utf8(proposal.stdout).expect("utf8 help");
    for command in [
        "list",
        "show",
        "resource",
        "discussion",
        "comment",
        "withdraw",
        "reject",
        "publish",
    ] {
        assert!(proposal.contains(command), "missing proposal {command}");
    }

    let role = gitim()
        .args(["skill", "role", "--help"])
        .output()
        .expect("role help");
    assert!(role.status.success());
    let role = String::from_utf8(role.stdout).expect("utf8 help");
    for command in [
        "owner-add",
        "owner-remove",
        "maintainer-add",
        "maintainer-remove",
    ] {
        assert!(role.contains(command), "missing role {command}");
    }

    let admin = gitim()
        .args(["skill", "admin", "--help"])
        .output()
        .expect("admin help");
    assert!(admin.status.success());
    let admin = String::from_utf8(admin.stdout).expect("utf8 help");
    for command in ["update", "archive", "unarchive"] {
        assert!(admin.contains(command), "missing admin {command}");
    }
}

#[test]
fn create_uses_explicit_request_id_and_clears_successful_journal() {
    let clone = fake_clone();
    let source = package(clone.path());
    let (server, request) = serve_once(&clone, mutation_response());
    gitim()
        .current_dir(clone.path())
        .args([
            "--json",
            "skill",
            "create",
            "release-check",
            "--from",
            source.to_str().unwrap(),
            "--display-name",
            "Release Check",
            "--description",
            "Verify releases.",
            "--request-id",
            REQUEST,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(REQUEST));
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({
            "method":"skill_create",
            "request":{
                "request_id":REQUEST,
                "slug":"release-check",
                "display_name":"Release Check",
                "description":"Verify releases.",
                "source_directory":source
            }
        })
    );
    assert!(!clone
        .path()
        .join(".gitim/request-journal")
        .join(format!("{REQUEST}.json"))
        .exists());
}

#[test]
fn propose_generates_and_dispatches_a_request_id() {
    let clone = fake_clone();
    let source = package(clone.path());
    let (server, request) = serve_once(&clone, mutation_response());
    let output = gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "propose",
            "release-check",
            "--from",
            source.to_str().unwrap(),
            "--base",
            REVISION,
            "--summary",
            "Tighten release checks.",
        ])
        .output()
        .expect("run propose");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    let request = request.recv().unwrap();
    let request_id = request["request"]["request_id"]
        .as_str()
        .expect("request id");
    assert!(request_id.starts_with("q-"));
    assert_eq!(request_id.len(), 28);
    assert!(String::from_utf8_lossy(&output.stderr).contains(request_id));
}

#[test]
fn proposal_transitions_all_accept_request_ids() {
    for operation in ["withdraw", "reject", "publish"] {
        let mut command = gitim();
        command.args([
            "skill",
            "proposal",
            operation,
            PROPOSAL,
            "--state-revision",
            "2",
            "--request-id",
            REQUEST,
            "--help",
        ]);
        command.assert().success();
    }
}

#[test]
fn every_visible_write_help_accepts_request_id() {
    for args in [
        vec!["skill", "create", "--help"],
        vec!["skill", "propose", "--help"],
        vec!["skill", "proposal", "comment", "--help"],
        vec!["skill", "proposal", "withdraw", "--help"],
        vec!["skill", "proposal", "reject", "--help"],
        vec!["skill", "proposal", "publish", "--help"],
        vec!["skill", "role", "owner-add", "--help"],
        vec!["skill", "role", "owner-remove", "--help"],
        vec!["skill", "role", "maintainer-add", "--help"],
        vec!["skill", "role", "maintainer-remove", "--help"],
        vec!["skill", "admin", "update", "--help"],
        vec!["skill", "admin", "archive", "--help"],
        vec!["skill", "admin", "unarchive", "--help"],
    ] {
        gitim()
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains("--request-id"));
    }
}

#[test]
fn load_prints_markdown_and_a_bounded_resource_index() {
    let clone = fake_clone();
    let (server, request) = serve_once(&clone, load_response(256));
    let output = gitim()
        .current_dir(clone.path())
        .args(["skill", "load", &format!("release-check@{REVISION}")])
        .output()
        .expect("run load");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    assert_eq!(
        request.recv().unwrap(),
        json!({
            "method":"skill_load",
            "reference":{"slug":"release-check","revision":REVISION}
        })
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 output");
    assert!(stdout.contains("# Release check"));
    assert_eq!(stdout.matches("references/").count(), 256);
}

#[test]
fn load_rejects_an_oversized_resource_index_as_protocol_failure() {
    let clone = fake_clone();
    let (server, _) = serve_once(&clone, load_response(257));
    gitim()
        .current_dir(clone.path())
        .args(["skill", "load", "release-check"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("resource index"));
    server.join().unwrap();
}

#[test]
fn binary_resource_requires_output() {
    let clone = fake_clone();
    let response = json!({
        "canonical_ref":{"slug":"release-check","revision":REVISION},
        "path":"assets/logo.png",
        "media_type":"image/png",
        "text":false,
        "bytes":[0,159,146,150]
    });
    let (server, _) = serve_once(&clone, response);
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            &format!("release-check@{REVISION}"),
            "assets/logo.png",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("--output"));
    server.join().unwrap();
}

#[test]
fn resource_output_requires_explicit_directory_creation_and_overwrite() {
    let clone = fake_clone();
    let response = json!({
        "canonical_ref":{"slug":"release-check","revision":REVISION},
        "path":"assets/logo.png",
        "media_type":"image/png",
        "text":false,
        "bytes":[0,159,146,150]
    });
    let output = clone.path().join("downloads/images/logo.png");

    let (server, _) = serve_once(&clone, response.clone());
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            "release-check",
            "assets/logo.png",
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("--create-dirs"));
    server.join().unwrap();

    let (server, _) = serve_once(&clone, response.clone());
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            "release-check",
            "assets/logo.png",
            "--output",
            output.to_str().unwrap(),
            "--create-dirs",
        ])
        .assert()
        .success();
    server.join().unwrap();
    assert_eq!(fs::read(&output).unwrap(), [0, 159, 146, 150]);

    let (server, _) = serve_once(&clone, response.clone());
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            "release-check",
            "assets/logo.png",
            "--output",
            output.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--force"));
    server.join().unwrap();

    let (server, _) = serve_once(&clone, response);
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "resource",
            "release-check",
            "assets/logo.png",
            "--output",
            output.to_str().unwrap(),
            "--force",
        ])
        .assert()
        .success();
    server.join().unwrap();
}

#[test]
fn structured_skill_error_exits_two_and_protocol_error_exits_one() {
    let clone = fake_clone();
    let (server, _) = serve_once_raw(
        &clone,
        format!(
            "{}\n",
            json!({"ok":false,"error":"missing","error_code":"skill_not_found"})
        ),
    );
    gitim()
        .current_dir(clone.path())
        .args(["skill", "list"])
        .assert()
        .failure()
        .code(2);
    server.join().unwrap();

    let (server, _) = serve_once_raw(&clone, "{malformed\n".to_string());
    gitim()
        .current_dir(clone.path())
        .args(["skill", "list"])
        .assert()
        .failure()
        .code(1);
    server.join().unwrap();
}

#[test]
fn success_exits_zero_and_daemon_not_ready_exits_three() {
    let clone = fake_clone();
    let (server, _) = serve_once(&clone, json!({"skills":[],"next_cursor":null}));
    gitim()
        .current_dir(clone.path())
        .args(["skill", "list"])
        .assert()
        .success();
    server.join().unwrap();

    let socket = clone.path().join(".gitim/run/gitim.sock");
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(socket).expect("bind socket");
    let closer = thread::spawn(move || {
        let (probe, _) = listener.accept().expect("readiness probe");
        drop(probe);
    });
    gitim()
        .current_dir(clone.path())
        .args(["skill", "list"])
        .assert()
        .failure()
        .code(3);
    closer.join().unwrap();
}

#[test]
fn unknown_daemon_error_code_is_a_protocol_failure() {
    let clone = fake_clone();
    let (server, _) = serve_once_raw(
        &clone,
        format!(
            "{}\n",
            json!({"ok":false,"error":"unknown","error_code":"future_code"})
        ),
    );
    gitim()
        .current_dir(clone.path())
        .args(["skill", "list"])
        .assert()
        .failure()
        .code(1);
    server.join().unwrap();
}

#[test]
fn retry_reuses_the_sole_matching_pending_request() {
    let clone = fake_clone();
    let source = package(clone.path());
    let command = [
        "skill",
        "create",
        "release-check",
        "--from",
        source.to_str().unwrap(),
        "--display-name",
        "Release Check",
        "--description",
        "Verify releases.",
    ];

    let (server, first_request) = serve_once_without_response(&clone);
    gitim()
        .current_dir(clone.path())
        .args(command)
        .assert()
        .failure()
        .code(1);
    server.join().unwrap();
    let first_request = first_request.recv().unwrap();
    let first_id = first_request["request"]["request_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(clone
        .path()
        .join(".gitim/request-journal")
        .join(format!("{first_id}.json"))
        .exists());

    let (server, retried_request) = serve_once(&clone, mutation_response());
    gitim()
        .current_dir(clone.path())
        .args(command)
        .assert()
        .success()
        .stderr(predicate::str::contains(&first_id));
    server.join().unwrap();
    assert_eq!(
        retried_request.recv().unwrap()["request"]["request_id"],
        first_id
    );
}

#[test]
fn explicit_request_id_refuses_a_fingerprint_mismatch() {
    let clone = fake_clone();
    let source = package(clone.path());
    let (server, _) = serve_once_without_response(&clone);
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "create",
            "release-check",
            "--from",
            source.to_str().unwrap(),
            "--display-name",
            "Release Check",
            "--description",
            "Verify releases.",
            "--request-id",
            REQUEST,
        ])
        .assert()
        .failure()
        .code(1);
    server.join().unwrap();

    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "create",
            "release-check",
            "--from",
            source.to_str().unwrap(),
            "--display-name",
            "Release Check",
            "--description",
            "Different request.",
            "--request-id",
            REQUEST,
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("fingerprint"));
}

#[test]
fn definitive_domain_failure_removes_the_pending_journal() {
    let clone = fake_clone();
    let source = package(clone.path());
    let (server, _) = serve_once_raw(
        &clone,
        format!(
            "{}\n",
            json!({"ok":false,"error":"exists","error_code":"skill_exists"})
        ),
    );
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "create",
            "release-check",
            "--from",
            source.to_str().unwrap(),
            "--display-name",
            "Release Check",
            "--description",
            "Verify releases.",
            "--request-id",
            REQUEST,
        ])
        .assert()
        .failure()
        .code(2);
    server.join().unwrap();
    assert!(!clone
        .path()
        .join(".gitim/request-journal")
        .join(format!("{REQUEST}.json"))
        .exists());
}

#[test]
fn mutation_success_without_data_is_a_protocol_failure_after_journal_cleanup() {
    let clone = fake_clone();
    let source = package(clone.path());
    let (server, _) = serve_once(&clone, Value::Null);
    gitim()
        .current_dir(clone.path())
        .args([
            "skill",
            "create",
            "release-check",
            "--from",
            source.to_str().unwrap(),
            "--display-name",
            "Release Check",
            "--description",
            "Verify releases.",
            "--request-id",
            REQUEST,
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("missing"));
    server.join().unwrap();
    assert!(!clone
        .path()
        .join(".gitim/request-journal")
        .join(format!("{REQUEST}.json"))
        .exists());
}

#[test]
fn json_load_contains_one_bounded_resource_index_and_a_canonical_ref() {
    let clone = fake_clone();
    let (server, _) = serve_once(&clone, load_response(2));
    let output = gitim()
        .current_dir(clone.path())
        .args(["--json", "skill", "load", "release-check"])
        .output()
        .expect("load json");
    assert!(output.status.success());
    server.join().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["canonical_ref"],
        format!("skill:release-check@{REVISION}")
    );
    assert_eq!(value["resources"].as_array().unwrap().len(), 2);
    assert!(
        value["revision"].get("resources").is_none(),
        "resource descriptors must not be duplicated"
    );
}
