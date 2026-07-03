#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for quick session title gate enforcement.
//!
//! Verifies:
//! 1. QUICK_SESSION_TITLE_REQUIRED blocks non-creator writes at needs_title
//! 2. Creator (human) is exempt from title gate
//! 3. Setting title transitions session to active, allowing agent writes
//! 4. Archived sessions reject writes
//! 5. session_id validation rejects invalid/unsafe IDs

mod common;

use std::sync::Arc;

use gitim_core::parser::parse_thread;
use gitim_core::types::ThreadEntry;
use gitim_daemon::api::{Request, Response};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

/// Helper: call `handle_request` with a typed Request, returning Response.
async fn call(state: Arc<AppState>, req: Request) -> Response {
    handle_request(req, state).await
}

/// Setup: temp repo with alice + bob + test-agent registered.
async fn setup() -> (tempfile::TempDir, Arc<AppState>) {
    let (tmp, state) = common::setup_repo_with_users(&["alice", "bob", "test-agent"]).await;
    (tmp, state)
}

#[tokio::test]
async fn create_session_status_is_needs_title() {
    let (_tmp, state) = setup().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_quick_session",
        "agent_id": "test-agent",
        "first_message": "hello, please help me with something",
        "author": "alice",
    }))
    .unwrap();
    let resp = call(state.clone(), req).await;
    assert!(resp.ok, "create_quick_session failed: {:?}", resp.error);
    // Verify returned data has needs_title status
    let data = resp.data.unwrap();
    assert_eq!(data["status"], "needs_title");
}

#[tokio::test]
async fn created_thread_uses_standard_gitim_format() {
    let (tmp, state) = setup().await;
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_quick_session",
        "agent_id": "test-agent",
        "first_message": "hello\nwith continuation",
        "author": "alice",
    }))
    .unwrap();
    let resp = call(state.clone(), req).await;
    assert!(resp.ok, "create_quick_session failed: {:?}", resp.error);
    let session_id = resp.data.unwrap()["id"].as_str().unwrap().to_string();

    let thread_path = tmp
        .path()
        .join("quick-sessions")
        .join(&session_id)
        .join("discussion.thread");
    let raw = std::fs::read_to_string(thread_path).unwrap();
    let parsed = parse_thread(&raw).expect("quick session thread should parse");

    assert_eq!(parsed.entries.len(), 1);
    match &parsed.entries[0] {
        ThreadEntry::Message(message) => {
            assert_eq!(message.line_number, 1);
            assert_eq!(message.point_to, 0);
            assert_eq!(message.author.as_str(), "alice");
            assert_eq!(message.body, "hello\nwith continuation");
        }
        _ => panic!("expected message"),
    }
}

#[tokio::test]
async fn agent_write_blocked_when_needs_title() {
    let (_tmp, state) = setup().await;

    // Create session (alice is creator)
    let create: Request = serde_json::from_value(serde_json::json!({
        "method": "create_quick_session",
        "agent_id": "test-agent",
        "first_message": "hello",
        "author": "alice",
    }))
    .unwrap();
    let create_resp = call(state.clone(), create).await;
    assert!(create_resp.ok);
    let session_id = create_resp.data.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Agent tries to send message before title is set → blocked
    let agent_msg: Request = serde_json::from_value(serde_json::json!({
        "method": "send_quick_session_message",
        "session_id": session_id,
        "body": "I can help with that!",
        "author": "test-agent",
    }))
    .unwrap();
    let agent_resp = call(state.clone(), agent_msg).await;
    assert!(
        !agent_resp.ok,
        "agent write should be blocked at needs_title"
    );
    assert!(
        agent_resp
            .error
            .as_deref()
            .unwrap_or("")
            .contains("QUICK_SESSION_TITLE_REQUIRED"),
        "expected QUICK_SESSION_TITLE_REQUIRED in error, got: {:?}",
        agent_resp.error
    );
}

#[tokio::test]
async fn creator_write_allowed_when_needs_title() {
    let (_tmp, state) = setup().await;

    let create: Request = serde_json::from_value(serde_json::json!({
        "method": "create_quick_session",
        "agent_id": "test-agent",
        "first_message": "hello",
        "author": "alice",
    }))
    .unwrap();
    let create_resp = call(state.clone(), create).await;
    assert!(create_resp.ok);
    let session_id = create_resp.data.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Creator (alice) sends additional message → allowed
    let human_msg: Request = serde_json::from_value(serde_json::json!({
        "method": "send_quick_session_message",
        "session_id": session_id,
        "body": "also, what about this other thing?",
        "author": "alice",
    }))
    .unwrap();
    let human_resp = call(state.clone(), human_msg).await;
    assert!(
        human_resp.ok,
        "creator should be exempt from title gate: error={:?}",
        human_resp.error
    );
}

#[tokio::test]
async fn set_title_transitions_to_active_and_allows_agent_write() {
    let (_tmp, state) = setup().await;

    // Create
    let create: Request = serde_json::from_value(serde_json::json!({
        "method": "create_quick_session",
        "agent_id": "test-agent",
        "first_message": "hello",
        "author": "alice",
    }))
    .unwrap();
    let create_resp = call(state.clone(), create).await;
    assert!(create_resp.ok);
    let session_id = create_resp.data.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Set title
    let set_title: Request = serde_json::from_value(serde_json::json!({
        "method": "set_quick_session_title",
        "session_id": session_id,
        "title": "Test Session Title",
    }))
    .unwrap();
    let title_resp = call(state.clone(), set_title).await;
    assert!(title_resp.ok, "set_title failed: {:?}", title_resp.error);

    // Verify status is now active
    let title_data = title_resp.data.unwrap();
    assert_eq!(
        title_data["status"], "active",
        "status should be active after title set"
    );

    // Agent sends message → NOW allowed
    let agent_msg: Request = serde_json::from_value(serde_json::json!({
        "method": "send_quick_session_message",
        "session_id": session_id,
        "body": "sure, let me help with that!",
        "author": "test-agent",
    }))
    .unwrap();
    let agent_resp = call(state.clone(), agent_msg).await;
    assert!(
        agent_resp.ok,
        "agent should be allowed to write after title is set: error={:?}",
        agent_resp.error
    );
}

#[tokio::test]
async fn archived_session_rejects_writes() {
    let (_tmp, state) = setup().await;

    // Create + set title to make active
    let create: Request = serde_json::from_value(serde_json::json!({
        "method": "create_quick_session",
        "agent_id": "test-agent",
        "first_message": "hello",
        "author": "alice",
    }))
    .unwrap();
    let create_resp = call(state.clone(), create).await;
    assert!(create_resp.ok);
    let session_id = create_resp.data.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let title_resp = call(
        state.clone(),
        serde_json::from_value(serde_json::json!({
            "method": "set_quick_session_title",
            "session_id": session_id,
            "title": "test",
        }))
        .unwrap(),
    )
    .await;
    assert!(title_resp.ok);

    // Archive
    let archive: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_quick_session",
        "session_id": session_id,
        "author": "alice",
    }))
    .unwrap();
    let archive_resp = call(state.clone(), archive).await;
    assert!(archive_resp.ok, "archive failed: {:?}", archive_resp.error);

    // Try to send message to archived session
    let msg: Request = serde_json::from_value(serde_json::json!({
        "method": "send_quick_session_message",
        "session_id": session_id,
        "body": "hello?",
        "author": "alice",
    }))
    .unwrap();
    let msg_resp = call(state.clone(), msg).await;
    assert!(!msg_resp.ok, "archived session should reject writes");
    assert!(
        msg_resp.error.as_deref().unwrap_or("").contains("archived"),
        "expected 'archived' in error, got: {:?}",
        msg_resp.error
    );
}

#[tokio::test]
async fn invalid_session_id_rejected() {
    let (_tmp, state) = setup().await;

    // Try to send a message with a path-traversal session_id
    let req: Request = serde_json::from_value(serde_json::json!({
        "method": "send_quick_session_message",
        "session_id": "../../etc/passwd",
        "body": "evil",
        "author": "alice",
    }))
    .unwrap();
    let resp = call(state.clone(), req).await;
    assert!(!resp.ok, "path-traversal session_id should be rejected");
    assert!(
        resp.error
            .as_deref()
            .unwrap_or("")
            .contains("invalid session_id"),
        "should contain 'invalid session_id', got: {:?}",
        resp.error
    );

    // Try read with invalid session_id
    let read_req: Request = serde_json::from_value(serde_json::json!({
        "method": "read_quick_session",
        "session_id": "not-a-valid-qs-id",
    }))
    .unwrap();
    let read_resp = call(state.clone(), read_req).await;
    assert!(
        !read_resp.ok,
        "invalid session_id should be rejected on read"
    );
    assert!(
        read_resp
            .error
            .as_deref()
            .unwrap_or("")
            .contains("invalid session_id"),
        "should contain 'invalid session_id', got: {:?}",
        read_resp.error
    );
}
