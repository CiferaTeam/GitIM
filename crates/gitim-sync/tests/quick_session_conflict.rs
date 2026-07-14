#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gitim_core::formatter::format_message;
use gitim_core::parser::parse_thread;
use gitim_core::types::{
    apply_quick_session_transition, Handler, QuickSessionMeta, QuickSessionStatus,
    QuickSessionTransition,
};
use gitim_sync::git::GitStorage;
use gitim_sync::sync_loop::{run_sync_cycle, AuthCircuit};
use tempfile::TempDir;

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const ATTEMPT_A: &str = "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const ATTEMPT_B: &str = "qa-01K00000000000000000000000";

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_session(root: &Path, meta: &QuickSessionMeta, thread: &str) {
    let directory = root.join(format!("quick-sessions/{SESSION_ID}"));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("session.meta.yaml"),
        serde_yaml::to_string(meta).unwrap(),
    )
    .unwrap();
    std::fs::write(directory.join("discussion.thread"), thread).unwrap();
}

fn archive_session(root: &Path, meta: &QuickSessionMeta, thread: &str) {
    let active = root.join(format!("quick-sessions/{SESSION_ID}"));
    let archived = root.join(format!("archive/quick-sessions/{SESSION_ID}"));
    std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
    if active.exists() {
        std::fs::rename(&active, &archived).unwrap();
    } else {
        std::fs::create_dir_all(&archived).unwrap();
    }
    std::fs::write(
        archived.join("session.meta.yaml"),
        serde_yaml::to_string(meta).unwrap(),
    )
    .unwrap();
    std::fs::write(archived.join("discussion.thread"), thread).unwrap();
}

fn sync_and_verify_archive(
    bare: &TempDir,
    local: &TempDir,
    archived_meta: &QuickSessionMeta,
    completed_meta: &QuickSessionMeta,
) {
    let repo = GitStorage::new(local.path());
    let pushed = Arc::new(AtomicBool::new(false));
    let pushed_callback = pushed.clone();
    let mut circuit = AuthCircuit::new(Arc::new(AtomicBool::new(false)));
    run_sync_cycle(
        &repo,
        &mut circuit,
        &Mutex::new(()),
        &move |_, _| pushed_callback.store(true, Ordering::SeqCst),
        &|_, _, _| {},
        &|_| {},
        &|| {},
        Some(&("Bob".to_string(), "bob@test.com".to_string())),
    );

    assert!(pushed.load(Ordering::SeqCst));
    assert!(!repo.has_stale_rebase_state());
    assert!(!local
        .path()
        .join(format!("quick-sessions/{SESSION_ID}"))
        .exists());
    assert!(local
        .path()
        .join(format!("archive/quick-sessions/{SESSION_ID}"))
        .exists());
    let verify = TempDir::new().unwrap();
    run_git(
        verify.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            verify.path().to_str().unwrap(),
        ],
    );
    assert!(!verify
        .path()
        .join(format!("quick-sessions/{SESSION_ID}"))
        .exists());
    let directory = verify
        .path()
        .join(format!("archive/quick-sessions/{SESSION_ID}"));
    let merged_thread = std::fs::read_to_string(directory.join("discussion.thread")).unwrap();
    let parsed = parse_thread(&merged_thread).unwrap();
    assert_eq!(parsed.messages().len(), 2);
    assert_eq!(parsed.messages()[1].body, "agent completion");
    assert_eq!(parsed.messages()[1].line_number, 2);

    let merged_meta: QuickSessionMeta = serde_yaml::from_str(
        &std::fs::read_to_string(directory.join("session.meta.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(merged_meta.status, QuickSessionStatus::Archived);
    assert_eq!(
        merged_meta.archived_at.as_deref(),
        archived_meta.archived_at.as_deref()
    );
    assert_eq!(merged_meta.archived_from, Some(QuickSessionStatus::Active));
    assert_eq!(
        merged_meta.last_completed_attempt_id,
        completed_meta.last_completed_attempt_id
    );
    assert_eq!(merged_meta.last_completed_input_line, Some(1));
    assert_eq!(merged_meta.last_completed_line, Some(2));
    assert!(merged_meta.processing_input_line.is_none());
    assert!(merged_meta.processing_started_at.is_none());
    assert!(merged_meta.attempt_id.is_none());
    assert!(
        merged_meta.revision > archived_meta.revision.max(completed_meta.revision),
        "conflict resolution must advance the durable revision"
    );

    let mut unarchived = merged_meta;
    apply_quick_session_transition(
        &mut unarchived,
        QuickSessionTransition::Unarchive {
            actor: "alice".to_string(),
            now: "2026-07-11T00:00:07Z".to_string(),
        },
    )
    .expect("creator can unarchive the converged session");
    assert_eq!(unarchived.status, QuickSessionStatus::Active);
    assert_eq!(unarchived.last_completed_input_line, Some(1));
}

fn setup() -> (TempDir, TempDir, TempDir, QuickSessionMeta, String) {
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
    run_git(clone_a.path(), &["config", "user.email", "alice@test.com"]);
    run_git(clone_a.path(), &["config", "user.name", "Alice"]);

    let alice = Handler::new("alice").unwrap();
    let mut meta = QuickSessionMeta::new(
        SESSION_ID.to_string(),
        "bob".to_string(),
        "alice".to_string(),
        "2026-07-11T00:00:00Z".to_string(),
    );
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::HumanMessage {
            actor: "alice".to_string(),
            line_number: 1,
            request_id: None,
            preview: "first".to_string(),
            now: "2026-07-11T00:00:01Z".to_string(),
        },
    )
    .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::Claim {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: ATTEMPT_A.to_string(),
            now: "2026-07-11T00:00:02Z".to_string(),
        },
    )
    .unwrap();
    apply_quick_session_transition(
        &mut meta,
        QuickSessionTransition::SetTitle {
            actor: "bob".to_string(),
            attempt_id: ATTEMPT_A.to_string(),
            title: "Concurrent session".to_string(),
            now: "2026-07-11T00:00:03Z".to_string(),
        },
    )
    .unwrap();
    let thread = format_message(1, 0, &alice, "20260711T000001Z", "first");
    write_session(clone_a.path(), &meta, &thread);
    run_git(clone_a.path(), &["add", "."]);
    run_git(clone_a.path(), &["commit", "-m", "session base"]);
    run_git(clone_a.path(), &["push", "-u", "origin", "HEAD"]);
    run_git(
        clone_b.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            clone_b.path().to_str().unwrap(),
        ],
    );
    run_git(clone_b.path(), &["config", "user.email", "bob@test.com"]);
    run_git(clone_b.path(), &["config", "user.name", "Bob"]);
    (bare, clone_a, clone_b, meta, thread)
}

#[test]
fn concurrent_creator_input_and_agent_completion_reconcile_session_markers() {
    let (bare, clone_a, clone_b, base_meta, base_thread) = setup();
    let alice = Handler::new("alice").unwrap();
    let bob = Handler::new("bob").unwrap();

    let mut remote_meta = base_meta.clone();
    apply_quick_session_transition(
        &mut remote_meta,
        QuickSessionTransition::HumanMessage {
            actor: "alice".to_string(),
            line_number: 2,
            request_id: Some("request-2".to_string()),
            preview: "queued while running".to_string(),
            now: "2026-07-11T00:00:04Z".to_string(),
        },
    )
    .unwrap();
    let remote_thread = format!(
        "{base_thread}{}",
        format_message(2, 0, &alice, "20260711T000004Z", "queued while running")
    );
    write_session(clone_a.path(), &remote_meta, &remote_thread);
    run_git(clone_a.path(), &["add", "."]);
    run_git(clone_a.path(), &["commit", "-m", "creator input"]);
    run_git(clone_a.path(), &["push"]);

    let mut local_meta = base_meta;
    apply_quick_session_transition(
        &mut local_meta,
        QuickSessionTransition::AgentReply {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: ATTEMPT_A.to_string(),
            output_line: 2,
            preview: "agent completion".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    let local_thread = format!(
        "{base_thread}{}",
        format_message(2, 1, &bob, "20260711T000005Z", "agent completion")
    );
    write_session(clone_b.path(), &local_meta, &local_thread);
    run_git(clone_b.path(), &["add", "."]);
    run_git(clone_b.path(), &["commit", "-m", "agent completion"]);

    let repo = GitStorage::new(clone_b.path());
    let pushed = Arc::new(AtomicBool::new(false));
    let pushed_callback = pushed.clone();
    let auth_failed = Arc::new(AtomicBool::new(false));
    let mut circuit = AuthCircuit::new(auth_failed);
    run_sync_cycle(
        &repo,
        &mut circuit,
        &Mutex::new(()),
        &move |_, _| pushed_callback.store(true, Ordering::SeqCst),
        &|_, _, _| {},
        &|_| {},
        &|| {},
        Some(&("Bob".to_string(), "bob@test.com".to_string())),
    );

    assert!(pushed.load(Ordering::SeqCst));
    assert!(!repo.has_stale_rebase_state());
    let verify = TempDir::new().unwrap();
    run_git(
        verify.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            verify.path().to_str().unwrap(),
        ],
    );
    let directory = verify.path().join(format!("quick-sessions/{SESSION_ID}"));
    let merged_thread = std::fs::read_to_string(directory.join("discussion.thread")).unwrap();
    let parsed = parse_thread(&merged_thread).unwrap();
    assert_eq!(parsed.messages().len(), 3);
    assert_eq!(parsed.messages()[1].body, "queued while running");
    assert_eq!(parsed.messages()[2].body, "agent completion");
    assert_eq!(parsed.messages()[2].line_number, 3);

    let merged_meta: QuickSessionMeta = serde_yaml::from_str(
        &std::fs::read_to_string(directory.join("session.meta.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(merged_meta.status, QuickSessionStatus::Active);
    assert_eq!(merged_meta.last_human_line, Some(2));
    assert_eq!(merged_meta.last_completed_input_line, Some(1));
    assert_eq!(merged_meta.last_completed_line, Some(3));
    assert!(merged_meta.processing_input_line.is_none());
    assert!(merged_meta.attempt_id.is_none());
    assert!(merged_meta.revision > remote_meta.revision.max(local_meta.revision));

    apply_quick_session_transition(
        &mut merged_meta.clone(),
        QuickSessionTransition::Claim {
            actor: "bob".to_string(),
            input_line: 2,
            attempt_id: ATTEMPT_B.to_string(),
            now: "2026-07-11T00:00:06Z".to_string(),
        },
    )
    .expect("queued creator input should remain claimable");
}

#[test]
fn metadata_only_claim_and_title_merge_with_remote_creator_append() {
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
    run_git(clone_a.path(), &["config", "user.email", "alice@test.com"]);
    run_git(clone_a.path(), &["config", "user.name", "Alice"]);

    let alice = Handler::new("alice").unwrap();
    let mut base_meta = QuickSessionMeta::new(
        SESSION_ID.to_string(),
        "bob".to_string(),
        "alice".to_string(),
        "2026-07-11T00:00:00Z".to_string(),
    );
    apply_quick_session_transition(
        &mut base_meta,
        QuickSessionTransition::HumanMessage {
            actor: "alice".to_string(),
            line_number: 1,
            request_id: Some("request-1".to_string()),
            preview: "first".to_string(),
            now: "2026-07-11T00:00:01Z".to_string(),
        },
    )
    .unwrap();
    let base_thread = format_message(1, 0, &alice, "20260711T000001Z", "first");
    write_session(clone_a.path(), &base_meta, &base_thread);
    run_git(clone_a.path(), &["add", "."]);
    run_git(clone_a.path(), &["commit", "-m", "session base"]);
    run_git(clone_a.path(), &["push", "-u", "origin", "HEAD"]);
    run_git(
        clone_b.path().parent().unwrap(),
        &[
            "clone",
            bare.path().to_str().unwrap(),
            clone_b.path().to_str().unwrap(),
        ],
    );
    run_git(clone_b.path(), &["config", "user.email", "bob@test.com"]);
    run_git(clone_b.path(), &["config", "user.name", "Bob"]);

    let mut remote_meta = base_meta.clone();
    apply_quick_session_transition(
        &mut remote_meta,
        QuickSessionTransition::HumanMessage {
            actor: "alice".to_string(),
            line_number: 2,
            request_id: Some("request-2".to_string()),
            preview: "queued while claiming".to_string(),
            now: "2026-07-11T00:00:02Z".to_string(),
        },
    )
    .unwrap();
    let remote_thread = format!(
        "{base_thread}{}",
        format_message(2, 0, &alice, "20260711T000002Z", "queued while claiming")
    );
    write_session(clone_a.path(), &remote_meta, &remote_thread);
    run_git(clone_a.path(), &["add", "."]);
    run_git(clone_a.path(), &["commit", "-m", "creator append"]);
    run_git(clone_a.path(), &["push"]);

    let mut local_meta = base_meta;
    apply_quick_session_transition(
        &mut local_meta,
        QuickSessionTransition::Claim {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: ATTEMPT_A.to_string(),
            now: "2026-07-11T00:00:03Z".to_string(),
        },
    )
    .unwrap();
    apply_quick_session_transition(
        &mut local_meta,
        QuickSessionTransition::SetTitle {
            actor: "bob".to_string(),
            attempt_id: ATTEMPT_A.to_string(),
            title: "Metadata-only title".to_string(),
            now: "2026-07-11T00:00:04Z".to_string(),
        },
    )
    .unwrap();
    write_session(clone_b.path(), &local_meta, &base_thread);
    run_git(
        clone_b.path(),
        &[
            "add",
            &format!("quick-sessions/{SESSION_ID}/session.meta.yaml"),
        ],
    );
    run_git(clone_b.path(), &["commit", "-m", "agent claim and title"]);

    let repo = GitStorage::new(clone_b.path());
    let pushed = Arc::new(AtomicBool::new(false));
    let pushed_callback = pushed.clone();
    let mut circuit = AuthCircuit::new(Arc::new(AtomicBool::new(false)));
    run_sync_cycle(
        &repo,
        &mut circuit,
        &Mutex::new(()),
        &move |_, _| pushed_callback.store(true, Ordering::SeqCst),
        &|_, _, _| {},
        &|_| {},
        &|| {},
        Some(&("Bob".to_string(), "bob@test.com".to_string())),
    );

    assert!(pushed.load(Ordering::SeqCst));
    assert!(!repo.has_stale_rebase_state());
    let merged_meta: QuickSessionMeta = serde_yaml::from_str(
        &std::fs::read_to_string(
            clone_b
                .path()
                .join(format!("quick-sessions/{SESSION_ID}/session.meta.yaml")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(merged_meta.title.as_deref(), Some("Metadata-only title"));
    assert_eq!(merged_meta.status, QuickSessionStatus::Running);
    assert_eq!(merged_meta.processing_input_line, Some(1));
    assert_eq!(merged_meta.attempt_id.as_deref(), Some(ATTEMPT_A));
    assert_eq!(merged_meta.last_human_line, Some(2));
    assert_eq!(
        merged_meta.last_human_request_id.as_deref(),
        Some("request-2")
    );

    let mut next = merged_meta;
    apply_quick_session_transition(
        &mut next,
        QuickSessionTransition::AgentReply {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: ATTEMPT_A.to_string(),
            output_line: 3,
            preview: "done".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    apply_quick_session_transition(
        &mut next,
        QuickSessionTransition::Claim {
            actor: "bob".to_string(),
            input_line: 2,
            attempt_id: ATTEMPT_B.to_string(),
            now: "2026-07-11T00:00:06Z".to_string(),
        },
    )
    .expect("remote creator append should remain claimable after the current turn");
}

#[test]
fn local_archive_wins_remote_agent_reply_and_preserves_completion() {
    let (bare, remote, local, base_meta, base_thread) = setup();
    let bob = Handler::new("bob").unwrap();

    let mut completed_meta = base_meta.clone();
    apply_quick_session_transition(
        &mut completed_meta,
        QuickSessionTransition::AgentReply {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: ATTEMPT_A.to_string(),
            output_line: 2,
            preview: "agent completion".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    let completed_thread = format!(
        "{base_thread}{}",
        format_message(2, 1, &bob, "20260711T000005Z", "agent completion")
    );
    write_session(remote.path(), &completed_meta, &completed_thread);
    run_git(remote.path(), &["add", "."]);
    run_git(remote.path(), &["commit", "-m", "agent completion"]);
    run_git(remote.path(), &["push"]);

    let mut archived_meta = base_meta;
    apply_quick_session_transition(
        &mut archived_meta,
        QuickSessionTransition::Archive {
            actor: "alice".to_string(),
            now: "2026-07-11T00:00:04Z".to_string(),
        },
    )
    .unwrap();
    archive_session(local.path(), &archived_meta, &base_thread);
    run_git(local.path(), &["add", "-A"]);
    run_git(local.path(), &["commit", "-m", "archive session"]);

    sync_and_verify_archive(&bare, &local, &archived_meta, &completed_meta);
}

#[test]
fn remote_archive_wins_local_agent_reply_and_preserves_completion() {
    let (bare, remote, local, base_meta, base_thread) = setup();
    let bob = Handler::new("bob").unwrap();

    let mut archived_meta = base_meta.clone();
    apply_quick_session_transition(
        &mut archived_meta,
        QuickSessionTransition::Archive {
            actor: "alice".to_string(),
            now: "2026-07-11T00:00:04Z".to_string(),
        },
    )
    .unwrap();
    archive_session(remote.path(), &archived_meta, &base_thread);
    run_git(remote.path(), &["add", "-A"]);
    run_git(remote.path(), &["commit", "-m", "archive session"]);
    run_git(remote.path(), &["push"]);

    let mut completed_meta = base_meta;
    apply_quick_session_transition(
        &mut completed_meta,
        QuickSessionTransition::AgentReply {
            actor: "bob".to_string(),
            input_line: 1,
            attempt_id: ATTEMPT_A.to_string(),
            output_line: 2,
            preview: "agent completion".to_string(),
            now: "2026-07-11T00:00:05Z".to_string(),
        },
    )
    .unwrap();
    let completed_thread = format!(
        "{base_thread}{}",
        format_message(2, 1, &bob, "20260711T000005Z", "agent completion")
    );
    write_session(local.path(), &completed_meta, &completed_thread);
    run_git(local.path(), &["add", "."]);
    run_git(local.path(), &["commit", "-m", "agent completion"]);

    sync_and_verify_archive(&bare, &local, &archived_meta, &completed_meta);
}
