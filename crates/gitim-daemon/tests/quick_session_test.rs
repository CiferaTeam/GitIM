#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{
    init_bare_and_clone, make_state, run_git, setup_repo_alice_bob, write_alice, write_bob,
};
use gitim_core::responses::{
    ClaimQuickSessionTurnResponse, CreateQuickSessionResponse, ListQuickSessionsResponse,
    ReadQuickSessionResponse, SendQuickSessionMessageResponse,
};
use gitim_core::types::{QuickSessionStatus, ThreadEntry};
use gitim_daemon::api::{Request, Response};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::{PendingMessage, SharedState};
use gitim_sync::sync_loop::{run_sync_cycle, AuthCircuit};

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const OTHER_SESSION_ID: &str = "qs-01K00000000000000000000000";
const ATTEMPT_ID: &str = "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const OTHER_ATTEMPT_ID: &str = "qa-01K00000000000000000000000";

fn data<T: serde::de::DeserializeOwned>(response: Response) -> T {
    assert!(response.ok, "request failed: {:?}", response.error);
    serde_json::from_value(response.data.unwrap()).unwrap()
}

async fn set_actor(state: &SharedState, actor: &str) {
    *state.current_user.write().await = Some(actor.to_string());
}

async fn create(
    state: SharedState,
    session_id: &str,
    agent_id: &str,
    first_message: &str,
    author: &str,
) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::CreateQuickSession {
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            first_message: first_message.to_string(),
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn send_human(state: SharedState, body: &str, request_id: &str, author: &str) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::SendQuickSessionMessage {
            session_id: SESSION_ID.to_string(),
            body: body.to_string(),
            reply_to: None,
            request_id: Some(request_id.to_string()),
            attempt_id: None,
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn claim(state: SharedState, input_line: u64, attempt_id: &str, author: &str) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::ClaimQuickSessionTurn {
            session_id: SESSION_ID.to_string(),
            input_line,
            attempt_id: attempt_id.to_string(),
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn set_title(state: SharedState, attempt_id: &str, author: &str) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::SetQuickSessionTitle {
            session_id: SESSION_ID.to_string(),
            title: "Investigate flaky build".to_string(),
            attempt_id: attempt_id.to_string(),
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn set_summary(state: SharedState, attempt_id: &str, author: &str) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::SetQuickSessionSummary {
            session_id: SESSION_ID.to_string(),
            summary: "Durable handoff".to_string(),
            attempt_id: attempt_id.to_string(),
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn mark_error(state: SharedState, attempt_id: &str, author: &str) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::MarkQuickSessionError {
            session_id: SESSION_ID.to_string(),
            attempt_id: attempt_id.to_string(),
            error: "provider failed".to_string(),
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn send_agent(
    state: SharedState,
    body: &str,
    input_line: u64,
    attempt_id: &str,
    author: &str,
) -> Response {
    set_actor(&state, author).await;
    handle_request(
        Request::SendQuickSessionMessage {
            session_id: SESSION_ID.to_string(),
            body: body.to_string(),
            reply_to: Some(input_line),
            request_id: None,
            attempt_id: Some(attempt_id.to_string()),
            author: Some(author.to_string()),
        },
        state,
    )
    .await
}

async fn read(state: SharedState) -> ReadQuickSessionResponse {
    data(
        handle_request(
            Request::ReadQuickSession {
                session_id: SESSION_ID.to_string(),
                limit: None,
                since: None,
            },
            state.clone(),
        )
        .await,
    )
}

fn message_lines(detail: &ReadQuickSessionResponse) -> Vec<(u64, String, String)> {
    detail
        .session
        .entries
        .iter()
        .filter_map(|entry| match entry {
            ThreadEntry::Message(message) => Some((
                message.line_number,
                message.author.to_string(),
                message.body.clone(),
            )),
            ThreadEntry::Event(_) => None,
        })
        .collect()
}

fn install_rejecting_hook(state: &SharedState) {
    let hook = state.repo_root.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn bare_head(bare: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(bare)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[tokio::test]
async fn create_is_one_commit_canonical_and_idempotent() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    let before = state.git_storage.rev_parse("HEAD").unwrap();

    let created: CreateQuickSessionResponse = data(
        create(
            state.clone(),
            SESSION_ID,
            "bob",
            "Please inspect CI",
            "alice",
        )
        .await,
    );
    assert_eq!(created.line_number, 1);
    assert_eq!(created.r#ref, format!("session:{SESSION_ID}"));
    assert_eq!(created.session.meta.status, QuickSessionStatus::NeedsTitle);
    assert_eq!(created.session.meta.revision, 2);
    assert_eq!(created.session.entries.len(), 1);
    assert_eq!(
        std::fs::read_to_string(
            state
                .repo_root
                .join(format!("quick-sessions/{SESSION_ID}/discussion.thread")),
        )
        .unwrap()
        .lines()
        .count(),
        1
    );

    let after = state.git_storage.rev_parse("HEAD").unwrap();
    let count = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{before}..{after}")])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1");

    let retry: CreateQuickSessionResponse = data(
        create(
            state.clone(),
            SESSION_ID,
            "bob",
            "Please inspect CI\n",
            "alice",
        )
        .await,
    );
    assert_eq!(retry, created);
    assert_eq!(state.git_storage.rev_parse("HEAD").unwrap(), after);

    let collision = create(state, SESSION_ID, "bob", "Different first message", "alice").await;
    assert!(!collision.ok);
    assert_eq!(
        collision.error_code.as_deref(),
        Some("quick_session_id_collision")
    );
}

#[tokio::test]
async fn create_rejects_invalid_unknown_and_departed_agents() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    let invalid = create(state.clone(), "../escape", "bob", "hello", "alice").await;
    assert!(!invalid.ok);
    assert!(!state.repo_root.join("escape").exists());

    let unknown = create(state.clone(), SESSION_ID, "carol", "hello", "alice").await;
    assert!(!unknown.ok);

    std::fs::create_dir_all(state.repo_root.join("archive/users")).unwrap();
    std::fs::rename(
        state.repo_root.join("users/bob.meta.yaml"),
        state.repo_root.join("archive/users/bob.meta.yaml"),
    )
    .unwrap();
    let departed = create(state, SESSION_ID, "bob", "hello", "alice").await;
    assert!(!departed.ok);
}

#[tokio::test]
async fn dispatch_rejects_quick_session_author_impersonation() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    let head_before = state.git_storage.rev_parse("HEAD").unwrap();

    let response = handle_request(
        Request::CreateQuickSession {
            session_id: SESSION_ID.to_string(),
            agent_id: "alice".to_string(),
            first_message: "impersonated".to_string(),
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;

    assert!(!response.ok);
    assert_eq!(
        response.error_code.as_deref(),
        Some("quick_session_forbidden")
    );
    assert!(!state.repo_root.join("quick-sessions").exists());
    assert_eq!(state.git_storage.rev_parse("HEAD").unwrap(), head_before);
}

#[tokio::test]
async fn quick_session_send_acks_locally_until_sync_confirms_push() {
    let root = tempfile::tempdir().unwrap();
    let bare = root.path().join("remote.git");
    let clone = root.path().join("clone");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::create_dir_all(&clone).unwrap();
    init_bare_and_clone(&bare, &clone);
    write_alice(&clone);
    write_bob(&clone);
    run_git(&clone, &["add", "."]);
    run_git(&clone, &["commit", "-m", "users"]);
    run_git(&clone, &["push", "-u", "origin", "HEAD"]);
    let state = make_state(clone, Some("alice"), &["alice", "bob"]).await;

    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    let create_head = state.git_storage.rev_parse("HEAD").unwrap();
    let remote_before_send = bare_head(&bare);
    let response: SendQuickSessionMessageResponse =
        data(send_human(state.clone(), "queued for sync", "req-pending", "alice").await);

    assert_eq!(response.line_number, 2);
    assert_eq!(bare_head(&bare), remote_before_send);
    let pending = state
        .pending_push
        .read()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|message| {
        message.channel == format!("quick_session:{SESSION_ID}")
            && matches!(message.line_number, 1 | 2)
    }));
    assert_eq!(pending[0].commit_id, create_head);
    assert_eq!(
        pending[1].commit_id,
        state.git_storage.rev_parse("HEAD").unwrap()
    );
    drop(pending);

    let mut events = state.event_tx.subscribe();
    run_git(
        &state.repo_root,
        &["push", "origin", &format!("{create_head}:refs/heads/main")],
    );
    state.confirm_pending_pushes(&create_head, &create_head);
    let first_push = events.try_recv().unwrap();
    match first_push {
        gitim_daemon::api::Event::MessagesPushed {
            channel,
            line_numbers,
        } => {
            assert_eq!(channel, format!("quick_session:{SESSION_ID}"));
            assert_eq!(line_numbers, vec![1]);
        }
        other => panic!("unexpected event: {other:?}"),
    }
    let pending = state
        .pending_push
        .read()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].line_number, 2);
    drop(pending);

    let mut circuit = AuthCircuit::new(state.auth_failed.clone());
    run_sync_cycle(
        &state.git_storage,
        &mut circuit,
        &state.commit_lock,
        &|pushed_head, included_head| state.confirm_pending_pushes(&pushed_head, &included_head),
        &|_, _, _| {},
        &|_| {},
        &|| {},
        None,
    );

    assert_ne!(bare_head(&bare), remote_before_send);
    assert!(state
        .pending_push
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
    let second_push = events.try_recv().unwrap();
    match second_push {
        gitim_daemon::api::Event::MessagesPushed {
            channel,
            line_numbers,
        } => {
            assert_eq!(channel, format!("quick_session:{SESSION_ID}"));
            assert_eq!(line_numbers, vec![2]);
        }
        other => panic!("unexpected event: {other:?}"),
    }

    let confirmed_head = state.git_storage.rev_parse("HEAD").unwrap();
    let commit_guard = state
        .commit_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let confirm_state = state.clone();
    let confirm_head = confirmed_head.clone();
    let confirm = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        confirm_state.confirm_pending_pushes(&confirm_head, &confirm_head);
        done_tx.send(()).unwrap();
    });
    started_rx.recv().unwrap();
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "push confirmation must wait for the writer's commit lock"
    );
    state
        .pending_push
        .write()
        .unwrap_or_else(|error| error.into_inner())
        .push(PendingMessage {
            channel: format!("quick_session:{SESSION_ID}"),
            line_number: 99,
            commit_id: confirmed_head,
        });
    drop(commit_guard);
    confirm.join().unwrap();
    done_rx.recv().unwrap();

    assert!(state
        .pending_push
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
    match events.try_recv().unwrap() {
        gitim_daemon::api::Event::MessagesPushed {
            channel,
            line_numbers,
        } => {
            assert_eq!(channel, format!("quick_session:{SESSION_ID}"));
            assert_eq!(line_numbers, vec![99]);
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn creator_send_and_agent_reply_are_idempotent() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );

    let sent: SendQuickSessionMessageResponse =
        data(send_human(state.clone(), "second", "req-1", "alice").await);
    let retried: SendQuickSessionMessageResponse =
        data(send_human(state.clone(), "different ignored body", "req-1", "alice").await);
    assert_eq!(sent, retried);
    assert_eq!(sent.line_number, 2);

    data::<ClaimQuickSessionTurnResponse>(claim(state.clone(), 2, ATTEMPT_ID, "bob").await);
    data::<gitim_core::responses::SetQuickSessionTitleResponse>(
        set_title(state.clone(), ATTEMPT_ID, "bob").await,
    );
    let reply: SendQuickSessionMessageResponse =
        data(send_agent(state.clone(), "done", 2, ATTEMPT_ID, "bob").await);
    let retry: SendQuickSessionMessageResponse = data(
        send_agent(
            state.clone(),
            "different ignored reply",
            2,
            ATTEMPT_ID,
            "bob",
        )
        .await,
    );
    assert_eq!(reply, retry);
    assert_eq!(message_lines(&read(state).await).len(), 3);
}

#[tokio::test]
async fn claim_is_agent_only_and_targets_latest_creator_line() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<SendQuickSessionMessageResponse>(
        send_human(state.clone(), "latest", "req-2", "alice").await,
    );

    let forbidden = claim(state.clone(), 2, ATTEMPT_ID, "alice").await;
    assert_eq!(
        forbidden.error_code.as_deref(),
        Some("quick_session_forbidden")
    );
    let stale_line = claim(state.clone(), 1, ATTEMPT_ID, "bob").await;
    assert_eq!(
        stale_line.error_code.as_deref(),
        Some("quick_session_invalid_state")
    );

    let claimed: ClaimQuickSessionTurnResponse =
        data(claim(state.clone(), 2, ATTEMPT_ID, "bob").await);
    let duplicate: ClaimQuickSessionTurnResponse =
        data(claim(state.clone(), 2, ATTEMPT_ID, "bob").await);
    assert_eq!(claimed, duplicate);
    let competing = claim(state, 2, OTHER_ATTEMPT_ID, "bob").await;
    assert_eq!(
        competing.error_code.as_deref(),
        Some("quick_session_invalid_state")
    );
}

#[tokio::test]
async fn title_gate_and_attempt_compare_and_set_protect_agent_writes() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<ClaimQuickSessionTurnResponse>(claim(state.clone(), 1, ATTEMPT_ID, "bob").await);

    let no_title = send_agent(state.clone(), "reply", 1, ATTEMPT_ID, "bob").await;
    assert_eq!(
        no_title.error_code.as_deref(),
        Some("quick_session_title_required")
    );
    assert_eq!(message_lines(&read(state.clone()).await).len(), 1);

    let stale_title = set_title(state.clone(), OTHER_ATTEMPT_ID, "bob").await;
    assert_eq!(
        stale_title.error_code.as_deref(),
        Some("quick_session_stale_attempt")
    );
    let stale_summary = set_summary(state.clone(), OTHER_ATTEMPT_ID, "bob").await;
    assert_eq!(
        stale_summary.error_code.as_deref(),
        Some("quick_session_stale_attempt")
    );
    let stale_error = mark_error(state.clone(), OTHER_ATTEMPT_ID, "bob").await;
    assert_eq!(
        stale_error.error_code.as_deref(),
        Some("quick_session_stale_attempt")
    );
    data::<gitim_core::responses::SetQuickSessionTitleResponse>(
        set_title(state.clone(), ATTEMPT_ID, "bob").await,
    );
    let wrong_reply = send_agent(state.clone(), "reply", 2, ATTEMPT_ID, "bob").await;
    assert_eq!(
        wrong_reply.error_code.as_deref(),
        Some("quick_session_invalid_state")
    );
}

#[tokio::test]
async fn queued_input_survives_running_turn_and_mark_error_recovers() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<ClaimQuickSessionTurnResponse>(claim(state.clone(), 1, ATTEMPT_ID, "bob").await);
    data::<gitim_core::responses::SetQuickSessionTitleResponse>(
        set_title(state.clone(), ATTEMPT_ID, "bob").await,
    );
    data::<SendQuickSessionMessageResponse>(
        send_human(state.clone(), "queued", "req-3", "alice").await,
    );
    let running = read(state.clone()).await;
    assert_eq!(running.session.meta.status, QuickSessionStatus::Running);
    assert_eq!(running.session.meta.last_human_line, Some(2));

    data::<gitim_core::responses::MarkQuickSessionErrorResponse>(
        mark_error(state.clone(), ATTEMPT_ID, "bob").await,
    );
    assert_eq!(
        read(state.clone()).await.session.meta.status,
        QuickSessionStatus::Active
    );
    data::<ClaimQuickSessionTurnResponse>(claim(state, 2, OTHER_ATTEMPT_ID, "bob").await);
}

#[tokio::test]
async fn archive_running_rejects_late_completion_and_unarchive_is_readable() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<ClaimQuickSessionTurnResponse>(claim(state.clone(), 1, ATTEMPT_ID, "bob").await);
    set_actor(&state, "alice").await;

    data::<gitim_core::responses::ArchiveQuickSessionResponse>(
        handle_request(
            Request::ArchiveQuickSession {
                session_id: SESSION_ID.to_string(),
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await,
    );
    let archived = read(state.clone()).await;
    assert!(archived.session.archived);
    assert_eq!(archived.session.meta.status, QuickSessionStatus::Archived);
    assert!(archived.session.meta.attempt_id.is_none());
    let late = set_title(state.clone(), ATTEMPT_ID, "bob").await;
    assert!(!late.ok);
    set_actor(&state, "alice").await;

    data::<gitim_core::responses::UnarchiveQuickSessionResponse>(
        handle_request(
            Request::UnarchiveQuickSession {
                session_id: SESSION_ID.to_string(),
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await,
    );
    let active = read(state).await;
    assert!(!active.session.archived);
    assert_eq!(active.session.meta.status, QuickSessionStatus::NeedsTitle);
}

#[tokio::test]
async fn mutations_roll_back_on_commit_failure() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    install_rejecting_hook(&state);
    let failed = create(state.clone(), SESSION_ID, "bob", "first", "alice").await;
    assert!(!failed.ok);
    assert!(!state
        .repo_root
        .join(format!("quick-sessions/{SESSION_ID}"))
        .exists());
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty());

    std::fs::remove_file(state.repo_root.join(".git/hooks/pre-commit")).unwrap();
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    install_rejecting_hook(&state);
    let archive = handle_request(
        Request::ArchiveQuickSession {
            session_id: SESSION_ID.to_string(),
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!archive.ok);
    assert!(state
        .repo_root
        .join(format!("quick-sessions/{SESSION_ID}"))
        .exists());
    assert!(!state
        .repo_root
        .join(format!("archive/quick-sessions/{SESSION_ID}"))
        .exists());
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "archive rollback left a dirty tree: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[tokio::test]
async fn send_and_unarchive_restore_exact_bytes_on_commit_failure() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    let active_dir = state.repo_root.join(format!("quick-sessions/{SESSION_ID}"));
    let meta_before = std::fs::read(active_dir.join("session.meta.yaml")).unwrap();
    let thread_before = std::fs::read(active_dir.join("discussion.thread")).unwrap();
    install_rejecting_hook(&state);

    let send = send_human(state.clone(), "must roll back", "req-rollback", "alice").await;
    assert!(!send.ok);
    assert_eq!(
        std::fs::read(active_dir.join("session.meta.yaml")).unwrap(),
        meta_before
    );
    assert_eq!(
        std::fs::read(active_dir.join("discussion.thread")).unwrap(),
        thread_before
    );

    std::fs::remove_file(state.repo_root.join(".git/hooks/pre-commit")).unwrap();
    data::<gitim_core::responses::ArchiveQuickSessionResponse>(
        handle_request(
            Request::ArchiveQuickSession {
                session_id: SESSION_ID.to_string(),
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await,
    );
    let archive_dir = state
        .repo_root
        .join(format!("archive/quick-sessions/{SESSION_ID}"));
    let archived_meta = std::fs::read(archive_dir.join("session.meta.yaml")).unwrap();
    let archived_thread = std::fs::read(archive_dir.join("discussion.thread")).unwrap();
    install_rejecting_hook(&state);

    let unarchive = handle_request(
        Request::UnarchiveQuickSession {
            session_id: SESSION_ID.to_string(),
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!unarchive.ok);
    assert!(!active_dir.exists());
    assert_eq!(
        std::fs::read(archive_dir.join("session.meta.yaml")).unwrap(),
        archived_meta
    );
    assert_eq!(
        std::fs::read(archive_dir.join("discussion.thread")).unwrap(),
        archived_thread
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty());
}

#[tokio::test]
async fn meta_only_transition_restores_exact_bytes_on_commit_failure() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<ClaimQuickSessionTurnResponse>(claim(state.clone(), 1, ATTEMPT_ID, "bob").await);
    let meta_path = state
        .repo_root
        .join(format!("quick-sessions/{SESSION_ID}/session.meta.yaml"));
    let meta_before = std::fs::read(&meta_path).unwrap();
    let head_before = state.git_storage.rev_parse("HEAD").unwrap();
    install_rejecting_hook(&state);

    let response = set_title(state.clone(), ATTEMPT_ID, "bob").await;

    assert!(!response.ok);
    assert_eq!(std::fs::read(&meta_path).unwrap(), meta_before);
    assert_eq!(state.git_storage.rev_parse("HEAD").unwrap(), head_before);
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&state.repo_root)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty());
}

#[tokio::test]
async fn completed_reply_retry_stays_idempotent_while_new_attempt_rejects_stale_target() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<ClaimQuickSessionTurnResponse>(claim(state.clone(), 1, ATTEMPT_ID, "bob").await);
    data::<gitim_core::responses::SetQuickSessionTitleResponse>(
        set_title(state.clone(), ATTEMPT_ID, "bob").await,
    );
    let first_reply: SendQuickSessionMessageResponse =
        data(send_agent(state.clone(), "first reply", 1, ATTEMPT_ID, "bob").await);
    data::<SendQuickSessionMessageResponse>(
        send_human(state.clone(), "new input", "req-new", "alice").await,
    );
    let new_claim: ClaimQuickSessionTurnResponse =
        data(claim(state.clone(), 3, OTHER_ATTEMPT_ID, "bob").await);
    let session_dir = state.repo_root.join(format!("quick-sessions/{SESSION_ID}"));
    let meta_before = std::fs::read(session_dir.join("session.meta.yaml")).unwrap();
    let thread_before = std::fs::read(session_dir.join("discussion.thread")).unwrap();
    let head_before = state.git_storage.rev_parse("HEAD").unwrap();

    let exact_retry: SendQuickSessionMessageResponse =
        data(send_agent(state.clone(), "first reply", 1, ATTEMPT_ID, "bob").await);
    assert_eq!(exact_retry.line_number, first_reply.line_number);
    assert_eq!(exact_retry.revision, new_claim.revision);
    assert_eq!(
        std::fs::read(session_dir.join("session.meta.yaml")).unwrap(),
        meta_before
    );
    assert_eq!(
        std::fs::read(session_dir.join("discussion.thread")).unwrap(),
        thread_before
    );
    assert_eq!(state.git_storage.rev_parse("HEAD").unwrap(), head_before);

    let response = handle_request(
        Request::SendQuickSessionMessage {
            session_id: SESSION_ID.to_string(),
            body: "late reply".to_string(),
            reply_to: Some(3),
            request_id: Some("req-looking-valid".to_string()),
            attempt_id: Some(ATTEMPT_ID.to_string()),
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;

    assert!(!response.ok);
    assert_eq!(
        response.error_code.as_deref(),
        Some("quick_session_stale_attempt")
    );
    assert_eq!(
        std::fs::read(session_dir.join("session.meta.yaml")).unwrap(),
        meta_before
    );
    assert_eq!(
        std::fs::read(session_dir.join("discussion.thread")).unwrap(),
        thread_before
    );
    assert_eq!(state.git_storage.rev_parse("HEAD").unwrap(), head_before);
}

#[tokio::test]
async fn list_filters_actionability_and_read_pagination() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );
    data::<CreateQuickSessionResponse>(
        create(state.clone(), OTHER_SESSION_ID, "alice", "other", "bob").await,
    );

    let listed: ListQuickSessionsResponse = data(
        handle_request(
            Request::ListQuickSessions {
                archived: false,
                agent_id: Some("bob".to_string()),
                actionable: true,
                limit: Some(1000),
            },
            state.clone(),
        )
        .await,
    );
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].id, SESSION_ID);

    let page: ReadQuickSessionResponse = data(
        handle_request(
            Request::ReadQuickSession {
                session_id: SESSION_ID.to_string(),
                limit: Some(1),
                since: Some(0),
            },
            state,
        )
        .await,
    );
    assert_eq!(page.session.entries.len(), 1);
}

#[tokio::test]
async fn poll_routes_active_and_archived_sessions_to_the_assigned_agent() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    let cursor = state.git_storage.rev_parse("HEAD").unwrap();
    data::<CreateQuickSessionResponse>(
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
    );

    let response = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    let json = response.data.unwrap();
    let changes = json["changes"].as_array().unwrap();
    let meta = changes
        .iter()
        .find(|change| change["kind"] == "quick_session_meta")
        .unwrap();
    assert_eq!(meta["channel"], SESSION_ID);
    assert_eq!(meta["entries"][0]["recipients"][0], "bob");
    let thread = changes
        .iter()
        .find(|change| change["kind"] == "quick_session_thread")
        .unwrap();
    assert_eq!(thread["entries"][0]["recipients"][0], "bob");

    let before_archive = state.git_storage.rev_parse("HEAD").unwrap();
    data::<gitim_core::responses::ArchiveQuickSessionResponse>(
        handle_request(
            Request::ArchiveQuickSession {
                session_id: SESSION_ID.to_string(),
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await,
    );
    let archived = handle_request(
        Request::Poll {
            since: Some(before_archive),
        },
        state,
    )
    .await;
    let changes = archived.data.unwrap()["changes"]
        .as_array()
        .unwrap()
        .clone();
    assert!(changes.iter().any(|change| {
        change["kind"] == "quick_session_meta"
            && change["entries"][0]["status"] == "archived"
            && change["entries"][0].get("recipients").is_none()
    }));
    assert!(changes.iter().any(|change| {
        change["kind"] == "quick_session_thread"
            && change["entries"].as_array().is_some_and(|entries| {
                !entries.is_empty()
                    && entries
                        .iter()
                        .all(|entry| entry.get("recipients").is_none())
            })
    }));
}

#[tokio::test]
async fn guest_guard_rejects_every_quick_session_write() {
    let (_tmp, state) = setup_repo_alice_bob().await;
    state
        .is_guest
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let writes = [
        create(state.clone(), SESSION_ID, "bob", "first", "alice").await,
        send_human(state.clone(), "body", "req", "alice").await,
        claim(state.clone(), 1, ATTEMPT_ID, "bob").await,
        set_title(state.clone(), ATTEMPT_ID, "bob").await,
        set_summary(state.clone(), ATTEMPT_ID, "bob").await,
        mark_error(state.clone(), ATTEMPT_ID, "bob").await,
        handle_request(
            Request::ArchiveQuickSession {
                session_id: SESSION_ID.to_string(),
                author: Some("alice".to_string()),
            },
            state.clone(),
        )
        .await,
        handle_request(
            Request::UnarchiveQuickSession {
                session_id: SESSION_ID.to_string(),
                author: Some("alice".to_string()),
            },
            state,
        )
        .await,
    ];
    assert!(writes.iter().all(|response| !response.ok));
}

#[test]
fn raw_json_request_shapes_deserialize() {
    let methods = [
        serde_json::json!({"method":"create_quick_session","session_id":SESSION_ID,"agent_id":"bob","first_message":"hello"}),
        serde_json::json!({"method":"list_quick_sessions","archived":false,"actionable":true}),
        serde_json::json!({"method":"read_quick_session","session_id":SESSION_ID}),
        serde_json::json!({"method":"send_quick_session_message","session_id":SESSION_ID,"body":"hello","request_id":"req"}),
        serde_json::json!({"method":"set_quick_session_title","session_id":SESSION_ID,"title":"title","attempt_id":ATTEMPT_ID}),
        serde_json::json!({"method":"set_quick_session_summary","session_id":SESSION_ID,"summary":"summary","attempt_id":ATTEMPT_ID}),
        serde_json::json!({"method":"claim_quick_session_turn","session_id":SESSION_ID,"input_line":1,"attempt_id":ATTEMPT_ID}),
        serde_json::json!({"method":"mark_quick_session_error","session_id":SESSION_ID,"attempt_id":ATTEMPT_ID,"error":"failed"}),
        serde_json::json!({"method":"archive_quick_session","session_id":SESSION_ID}),
        serde_json::json!({"method":"unarchive_quick_session","session_id":SESSION_ID}),
    ];
    for value in methods {
        serde_json::from_value::<Request>(value).unwrap();
    }
}
