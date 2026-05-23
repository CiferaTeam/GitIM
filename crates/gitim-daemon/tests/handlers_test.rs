#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gitim_core::types::config::Config;
use gitim_core::types::{ChannelMeta, UserMeta};
use gitim_daemon::api::Request;
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::{AppState, SharedState};
use std::sync::Arc;
use tokio::sync::broadcast;

fn setup_test_state(tmp: &std::path::Path) -> SharedState {
    let remote = tmp.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare"])
        .current_dir(&remote)
        .output()
        .unwrap();

    let repo = tmp.join("repo");
    std::process::Command::new("git")
        .args(["clone", remote.to_str().unwrap(), repo.to_str().unwrap()])
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .output()
        .unwrap();

    // Initial commit so main branch exists
    std::fs::write(repo.join(".keep"), "").unwrap();
    std::process::Command::new("git")
        .args(["add", ".keep"])
        .current_dir(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["push", "-u", "origin", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();

    let (event_tx, _) = broadcast::channel(16);
    Arc::new(AppState::new(repo, Config::default(), event_tx, None))
}

/// Register a user by creating meta.yaml and adding to in-memory user list.
async fn register_test_user(state: &SharedState, handler: &str) {
    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir).unwrap();
    let meta = UserMeta {
        display_name: handler.to_string(),
        role: "member".to_string(),
        introduction: "test user".to_string(),
    };
    std::fs::write(
        users_dir.join(format!("{}.meta.yaml", handler)),
        serde_yaml::to_string(&meta).unwrap(),
    )
    .unwrap();
    let rel = format!("users/{}.meta.yaml", handler);
    let _ = state
        .git_storage
        .add_and_commit(&[&rel], &format!("user: register @{}", handler));
    let mut users = state.users.write().await;
    if !users.contains(&handler.to_string()) {
        users.push(handler.to_string());
        users.sort();
    }
}

/// Create a channel with meta.yaml and empty .thread file.
fn create_test_channel(state: &SharedState, name: &str, created_by: &str) {
    create_test_channel_with_members(state, name, created_by, &[]);
}

fn create_test_channel_with_members(
    state: &SharedState,
    name: &str,
    created_by: &str,
    members: &[&str],
) {
    let ch_dir = state.repo_root.join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    let meta = ChannelMeta {
        display_name: name.to_string(),
        created_by: created_by.to_string(),
        created_at: "20260323T000000Z".to_string(),
        introduction: "test channel".to_string(),
        members: members.iter().map(|m| m.to_string()).collect(),
    };
    std::fs::write(
        ch_dir.join(format!("{}.meta.yaml", name)),
        serde_yaml::to_string(&meta).unwrap(),
    )
    .unwrap();
    std::fs::write(ch_dir.join(format!("{}.thread", name)), "").unwrap();
    let meta_rel = format!("channels/{}.meta.yaml", name);
    let thread_rel = format!("channels/{}.thread", name);
    let _ = state.git_storage.add_and_commit(
        &[&meta_rel, &thread_rel],
        &format!("init: channel {}", name),
    );
}

#[tokio::test]
async fn test_join_channel_self() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "general", "alice");

    let resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "join failed: {:?}", resp.error);

    // Verify .thread has join event
    let thread = std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
    assert!(thread.contains("[E:join]"), "thread missing join event");
    assert!(thread.contains("@alice"), "thread missing author");

    // Verify meta.yaml has alice in members
    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
    assert!(
        meta.members.contains(&"alice".to_string()),
        "alice not in members"
    );
}

#[tokio::test]
async fn test_join_channel_self_when_channel_already_has_members() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel_with_members(&state, "general", "alice", &["alice"]);

    let resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "join failed: {:?}", resp.error);

    let thread = std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
    assert!(thread.contains("[E:join]"), "thread missing join event");
    assert!(thread.contains("@bob"), "thread missing joining author");

    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
    assert_eq!(
        meta.members,
        vec!["alice".to_string(), "bob".to_string()],
        "members should include the existing member and joining author"
    );
}

#[tokio::test]
async fn test_join_channel_pull_others() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "general", "alice");

    // Alice joins first
    let resp1 = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp1.ok, "alice join failed: {:?}", resp1.error);

    // Alice pulls bob in
    let resp2 = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec!["bob".to_string()],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp2.ok, "pull bob failed: {:?}", resp2.error);

    // Verify both in members
    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
    assert!(
        meta.members.contains(&"alice".to_string()),
        "alice not in members"
    );
    assert!(
        meta.members.contains(&"bob".to_string()),
        "bob not in members"
    );

    // Verify thread has 2 events
    let thread = std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
    let join_count = thread.matches("[E:join]").count();
    assert_eq!(join_count, 2, "expected 2 join events, got {}", join_count);
}

#[tokio::test]
async fn test_leave_channel_self() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "general", "alice");

    // Alice joins
    let resp1 = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp1.ok, "join failed: {:?}", resp1.error);

    // Alice pulls bob in so leaving her won't empty the member list.
    let resp_bob = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec!["bob".to_string()],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp_bob.ok, "bob join failed: {:?}", resp_bob.error);

    // Bob leaves. This keeps the self-leave success case separate from the
    // channel-creator guard.
    let resp2 = handle_request(
        Request::LeaveChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp2.ok, "leave failed: {:?}", resp2.error);

    // Verify meta.yaml members now contains only alice
    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
    assert_eq!(
        meta.members,
        vec!["alice".to_string()],
        "members should be [alice], got: {:?}",
        meta.members
    );

    // Verify thread has both join and leave events
    let thread = std::fs::read_to_string(state.repo_root.join("channels/general.thread")).unwrap();
    assert!(thread.contains("[E:join]"), "thread missing join event");
    assert!(thread.contains("[E:leave]"), "thread missing leave event");
}

#[tokio::test]
async fn test_leave_channel_creator_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel_with_members(&state, "general", "alice", &["alice", "bob"]);

    let resp = handle_request(
        Request::LeaveChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!resp.ok, "channel creator self-leave should fail");
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("transfer ownership before leaving"),
        "error should mention ownership transfer, got: {}",
        err
    );

    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
    assert_eq!(
        meta.members,
        vec!["alice".to_string(), "bob".to_string()],
        "creator leave rejection should not mutate members"
    );
}

#[tokio::test]
async fn test_leave_channel_last_member_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "general", "alice");

    // Alice joins as the only member.
    let resp_join = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp_join.ok, "join failed: {:?}", resp_join.error);

    // Alice tries to leave — should be rejected because it would empty members.
    let resp_leave = handle_request(
        Request::LeaveChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(
        !resp_leave.ok,
        "last-member self-leave should fail, got ok response"
    );
    let err = resp_leave.error.unwrap_or_default();
    assert!(
        err.contains("last member"),
        "error should mention last-member constraint, got: {}",
        err
    );

    // meta.members still contains alice (the leave was rejected before any write)
    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/general.meta.yaml")).unwrap();
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();
    assert_eq!(meta.members, vec!["alice".to_string()]);
}

#[tokio::test]
async fn test_read_returns_entries_with_type() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "general", "alice");

    // Alice joins (creates an event entry)
    let join_resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(join_resp.ok, "join failed: {:?}", join_resp.error);

    // Alice sends a message
    let send_resp = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "hello".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);

    // Read the channel
    let read_resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: None,
            since: None,
        },
        state.clone(),
    )
    .await;
    assert!(read_resp.ok, "read failed: {:?}", read_resp.error);

    let data = read_resp.data.unwrap();
    let entries = data["entries"].as_array().expect("expected entries array");
    assert_eq!(
        entries.len(),
        2,
        "expected 2 entries, got {}",
        entries.len()
    );

    // First entry is the join event
    assert_eq!(entries[0]["type"], "event");
    assert_eq!(entries[0]["event_type"], "join");
    assert_eq!(entries[0]["author"], "alice");

    // Second entry is the message
    assert_eq!(entries[1]["type"], "message");
    assert_eq!(entries[1]["body"], "hello");
    assert_eq!(entries[1]["author"], "alice");

    // Verify "messages" key is absent
    assert!(
        data.get("messages").is_none(),
        "should not have 'messages' key"
    );
}

#[tokio::test]
async fn test_poll_filters_non_member_channels() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "general", "alice");
    create_test_channel(&state, "random", "alice");

    // Bob joins random so its members list is non-empty (not legacy/open)
    let bob_join = handle_request(
        Request::JoinChannel {
            channel: "random".to_string(),
            targets: vec![],
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(bob_join.ok, "bob join random failed: {:?}", bob_join.error);

    // Push initial state to origin
    state.git_storage.push().ok();

    // Alice joins general only
    let join_resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(join_resp.ok, "join general failed: {:?}", join_resp.error);

    // Set current_user to alice
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("alice".to_string());
    }

    // Get cursor before changes
    state.git_storage.push().ok();
    let poll_before = handle_request(Request::Poll { since: None }, state.clone()).await;
    let cursor = poll_before.data.unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Alice sends to general (she is a member) — should succeed
    let send_general = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "hello general".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(
        send_general.ok,
        "send general failed: {:?}",
        send_general.error
    );

    // Alice sends to random (she is NOT a member) — should be rejected
    let send_random = handle_request(
        Request::Send {
            channel: "random".to_string(),
            body: "hello random".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!send_random.ok, "send random should have been rejected");
    assert!(
        send_random.error.as_ref().unwrap().contains("not a member"),
        "expected 'not a member' error, got: {:?}",
        send_random.error
    );

    // Push so poll can see changes
    state.git_storage.push().ok();

    // Poll with cursor
    let poll_resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

    let data = poll_resp.data.unwrap();
    let changes = data["changes"].as_array().unwrap();

    // Should only contain general-related changes, not random
    let channel_names: Vec<&str> = changes
        .iter()
        .filter(|c| c["kind"] == "channel" || c["kind"] == "channel_meta")
        .filter_map(|c| c["channel"].as_str())
        .collect();
    assert!(
        channel_names.contains(&"general"),
        "general should be in poll results: {:?}",
        channel_names
    );
    assert!(
        !channel_names.contains(&"random"),
        "random should NOT be in poll results (not a member): {:?}",
        channel_names
    );
}

#[tokio::test]
async fn test_poll_admin_bypass_returns_all_channels() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "general", "alice");
    create_test_channel(&state, "random", "alice");

    // Bob joins random so its members list is non-empty
    let bob_join = handle_request(
        Request::JoinChannel {
            channel: "random".to_string(),
            targets: vec![],
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(bob_join.ok, "bob join random failed: {:?}", bob_join.error);

    // Alice joins general only
    let join_resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(join_resp.ok, "join general failed: {:?}", join_resp.error);

    // Set current_user to alice and enable admin mode
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("alice".to_string());
    }
    state
        .is_admin
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Get cursor before changes
    state.git_storage.push().ok();
    let poll_before = handle_request(Request::Poll { since: None }, state.clone()).await;
    let cursor = poll_before.data.unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Alice sends to general (she is a member)
    let send_general = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "hello general".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(
        send_general.ok,
        "send general failed: {:?}",
        send_general.error
    );

    // Bob sends to random (he is a member)
    let send_random = handle_request(
        Request::Send {
            channel: "random".to_string(),
            body: "hello random".to_string(),
            reply_to: None,
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(
        send_random.ok,
        "send random failed: {:?}",
        send_random.error
    );

    // Push so poll can see changes
    state.git_storage.push().ok();

    // Poll with cursor — admin should see ALL channels
    let poll_resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(poll_resp.ok, "poll failed: {:?}", poll_resp.error);

    let data = poll_resp.data.unwrap();
    let changes = data["changes"].as_array().unwrap();

    let channel_names: Vec<&str> = changes
        .iter()
        .filter(|c| c["kind"] == "channel" || c["kind"] == "channel_meta")
        .filter_map(|c| c["channel"].as_str())
        .collect();
    assert!(
        channel_names.contains(&"general"),
        "general should be in admin poll results: {:?}",
        channel_names
    );
    assert!(
        channel_names.contains(&"random"),
        "random SHOULD be in admin poll results (admin bypass): {:?}",
        channel_names
    );
}

#[tokio::test]
async fn test_send_member_channel_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "general", "alice");

    // Alice joins general
    let join_resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(join_resp.ok, "join failed: {:?}", join_resp.error);

    // Alice sends to general — should succeed
    let send_resp = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "hello from member".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);
}

#[tokio::test]
async fn test_send_non_member_channel_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "general", "alice");

    // Bob joins so members list is non-empty
    let join_resp = handle_request(
        Request::JoinChannel {
            channel: "general".to_string(),
            targets: vec![],
            author: Some("bob".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(join_resp.ok, "bob join failed: {:?}", join_resp.error);

    // Alice sends to general — she is NOT a member
    let send_resp = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "should be rejected".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!send_resp.ok, "send should have been rejected");
    assert!(
        send_resp.error.as_ref().unwrap().contains("not a member"),
        "expected 'not a member' error, got: {:?}",
        send_resp.error
    );
}

#[tokio::test]
async fn test_send_open_channel_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    // create_test_channel creates with empty members (open channel)
    create_test_channel(&state, "general", "alice");

    // Alice sends to general — open channel, should succeed
    let send_resp = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "open channel message".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);
}

#[tokio::test]
async fn test_send_dm_participant_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;

    // Alice sends DM to dm:alice,bob — she is a participant
    let send_resp = handle_request(
        Request::Send {
            channel: "dm:alice,bob".to_string(),
            body: "hey bob".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(send_resp.ok, "dm send failed: {:?}", send_resp.error);
}

#[tokio::test]
async fn test_send_dm_non_participant_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    register_test_user(&state, "charlie").await;

    // Charlie sends to dm:alice,bob — he is NOT a participant
    let send_resp = handle_request(
        Request::Send {
            channel: "dm:alice,bob".to_string(),
            body: "sneaky message".to_string(),
            reply_to: None,
            author: Some("charlie".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!send_resp.ok, "dm send should have been rejected");
    assert!(
        send_resp.error.as_ref().unwrap().contains("not a member"),
        "expected 'not a member' error, got: {:?}",
        send_resp.error
    );
}

#[tokio::test]
async fn test_send_invalid_channel_name_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;

    let resp = handle_request(
        Request::Send {
            channel: "../../etc/passwd".to_string(),
            body: "pwn".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!resp.ok, "send to traversal path should be rejected");
    assert!(
        resp.error
            .as_ref()
            .unwrap()
            .contains("invalid channel name"),
        "expected 'invalid channel name' error, got: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_read_invalid_channel_name_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;

    let resp = handle_request(
        Request::Read {
            channel: "../../etc/passwd".to_string(),
            limit: None,
            since: None,
        },
        state.clone(),
    )
    .await;
    assert!(!resp.ok, "read from traversal path should be rejected");
    assert!(
        resp.error
            .as_ref()
            .unwrap()
            .contains("invalid channel name"),
        "expected 'invalid channel name' error, got: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_send_nonexistent_channel_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    // DO NOT create a channel — "nonexistent" has no meta.json

    let resp = handle_request(
        Request::Send {
            channel: "nonexistent".to_string(),
            body: "hello".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(!resp.ok, "send to nonexistent channel should be rejected");
    assert!(
        resp.error.as_ref().unwrap().contains("does not exist"),
        "expected 'does not exist' error, got: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_send_dm_unregistered_participant_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    // ghost is NOT registered

    let resp = handle_request(
        Request::Send {
            channel: "dm:alice,ghost".to_string(),
            body: "hello ghost".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(
        !resp.ok,
        "send to DM with unregistered participant should be rejected"
    );
    assert!(
        resp.error
            .as_ref()
            .unwrap()
            .contains("not a registered user"),
        "expected 'not a registered user' error, got: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_create_channel_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;

    let resp = handle_request(
        Request::CreateChannel {
            name: "random".to_string(),
            display_name: Some("Random".to_string()),
            introduction: Some("A random channel".to_string()),
            author: Some("alice".to_string()),
            invitees: vec![],
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "create_channel failed: {:?}", resp.error);

    let data = resp.data.unwrap();
    assert_eq!(data["channel"], "random");
    assert_eq!(data["created_by"], "alice");

    // Verify meta.yaml exists with correct content
    let meta_str =
        std::fs::read_to_string(state.repo_root.join("channels/random.meta.yaml")).unwrap();
    let meta: serde_yaml::Value = serde_yaml::from_str(&meta_str).unwrap();
    assert_eq!(meta["display_name"], "Random");
    assert_eq!(meta["created_by"], "alice");
    assert_eq!(meta["introduction"], "A random channel");
    let members = meta["members"].as_sequence().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0], "alice");

    // Verify .thread exists with a join event
    let thread = std::fs::read_to_string(state.repo_root.join("channels/random.thread")).unwrap();
    assert!(thread.contains("[E:join]"), "thread missing join event");
    assert!(thread.contains("@alice"), "thread missing author");
    assert!(
        !thread.contains("\"targets\""),
        "empty invitees should not produce targets: {thread}"
    );
}

#[tokio::test]
async fn test_create_channel_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "general", "alice");

    let resp = handle_request(
        Request::CreateChannel {
            name: "general".to_string(),
            display_name: None,
            introduction: None,
            author: Some("alice".to_string()),
            invitees: vec![],
        },
        state.clone(),
    )
    .await;
    assert!(!resp.ok, "create_channel should fail for existing channel");
    assert!(
        resp.error.as_ref().unwrap().contains("already exists"),
        "expected 'already exists' error, got: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_create_channel_invalid_name() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;

    let resp = handle_request(
        Request::CreateChannel {
            name: "../../bad".to_string(),
            display_name: None,
            introduction: None,
            author: Some("alice".to_string()),
            invitees: vec![],
        },
        state.clone(),
    )
    .await;
    assert!(!resp.ok, "create_channel should fail for invalid name");
    assert!(
        resp.error
            .as_ref()
            .unwrap()
            .contains("invalid channel name"),
        "expected 'invalid channel name' error, got: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_create_channel_then_send() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;

    // Create channel
    let create_resp = handle_request(
        Request::CreateChannel {
            name: "dev".to_string(),
            display_name: None,
            introduction: None,
            author: Some("alice".to_string()),
            invitees: vec![],
        },
        state.clone(),
    )
    .await;
    assert!(
        create_resp.ok,
        "create_channel failed: {:?}",
        create_resp.error
    );

    // Send message to the new channel
    let send_resp = handle_request(
        Request::Send {
            channel: "dev".to_string(),
            body: "hello dev channel".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(
        send_resp.ok,
        "send to new channel failed: {:?}",
        send_resp.error
    );
}

#[tokio::test]
async fn test_create_channel_with_invitees() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    register_test_user(&state, "carol").await;

    let resp = handle_request(
        Request::CreateChannel {
            name: "team-alpha".to_string(),
            display_name: None,
            introduction: None,
            author: Some("alice".to_string()),
            invitees: vec!["bob".to_string(), "carol".to_string()],
        },
        state.clone(),
    )
    .await;
    assert!(
        resp.ok,
        "create_channel with invitees failed: {:?}",
        resp.error
    );

    let meta_str = std::fs::read_to_string(state.repo_root.join("channels/team-alpha.meta.yaml"))
        .expect("meta.yaml should exist after successful create");
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();

    assert_eq!(
        meta.members,
        vec!["alice".to_string(), "bob".to_string(), "carol".to_string()],
        "members should be [author, invitees...] in order; got: {:?}",
        meta.members
    );

    let thread = std::fs::read_to_string(state.repo_root.join("channels/team-alpha.thread"))
        .expect("thread should exist");
    assert!(
        thread.contains("[@alice]") && thread.contains("[E:join]"),
        "thread should contain creator's E:join event; got: {thread}"
    );
    assert!(
        thread.contains("\"targets\":[\"bob\",\"carol\"]"),
        "thread should carry invitees as targets in order; got: {thread}"
    );
}

#[tokio::test]
async fn test_create_channel_invitees_are_deduped() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    register_test_user(&state, "carol").await;

    let resp = handle_request(
        Request::CreateChannel {
            name: "dedup-test".to_string(),
            display_name: None,
            introduction: None,
            author: Some("alice".to_string()),
            invitees: vec![
                "alice".to_string(),
                "bob".to_string(),
                "bob".to_string(),
                "carol".to_string(),
            ],
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "create_channel failed: {:?}", resp.error);

    let meta_str = std::fs::read_to_string(state.repo_root.join("channels/dedup-test.meta.yaml"))
        .expect("meta.yaml should exist");
    let meta: ChannelMeta = serde_yaml::from_str(&meta_str).unwrap();

    assert_eq!(
        meta.members,
        vec!["alice".to_string(), "bob".to_string(), "carol".to_string()],
        "invitees should be deduped while preserving first-seen order; got: {:?}",
        meta.members
    );
}

#[tokio::test]
async fn test_create_channel_invitee_unregistered_rejects() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    // "ghost" is intentionally NOT registered

    let resp = handle_request(
        Request::CreateChannel {
            name: "ghost-channel".to_string(),
            display_name: None,
            introduction: None,
            author: Some("alice".to_string()),
            invitees: vec!["ghost".to_string()],
        },
        state.clone(),
    )
    .await;

    assert!(
        !resp.ok,
        "create_channel should reject unregistered invitee; got ok=true"
    );
    let err = resp.error.as_deref().unwrap_or("");
    assert!(
        err.contains("ghost") || err.contains("not registered"),
        "error message should mention 'ghost' or 'not registered'; got: {:?}",
        resp.error
    );

    // Channel must NOT have been created (full transactional reject)
    assert!(
        !state
            .repo_root
            .join("channels/ghost-channel.meta.yaml")
            .exists(),
        "meta.yaml must NOT be created when an invitee is unregistered"
    );
    assert!(
        !state
            .repo_root
            .join("channels/ghost-channel.thread")
            .exists(),
        "thread file must NOT be created when an invitee is unregistered"
    );
}

fn make_guest_state(tmp: &std::path::Path) -> SharedState {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&repo)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .output()
        .unwrap();

    let (tx, _) = broadcast::channel(16);
    let state = Arc::new(AppState::new(repo, Config::default(), tx, None));
    state
        .is_guest
        .store(true, std::sync::atomic::Ordering::SeqCst);
    state
}

#[tokio::test]
async fn guest_send_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_guest_state(tmp.path());

    let resp = handle_request(
        Request::Send {
            channel: "general".to_string(),
            body: "hello".to_string(),
            reply_to: None,
            author: None,
        },
        state,
    )
    .await;

    assert!(!resp.ok, "guest send should fail");
    assert!(
        resp.error.as_deref().unwrap().contains("guest"),
        "error should mention guest mode: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn guest_create_channel_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_guest_state(tmp.path());

    let resp = handle_request(
        Request::CreateChannel {
            name: "test-ch".to_string(),
            display_name: None,
            introduction: None,
            author: None,
            invitees: vec![],
        },
        state,
    )
    .await;

    assert!(!resp.ok, "guest create_channel should fail");
    assert!(
        resp.error.as_deref().unwrap().contains("guest"),
        "error should mention guest mode: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn guest_read_operations_are_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_guest_state(tmp.path());

    let resp = handle_request(Request::Status, state.clone()).await;
    assert!(resp.ok, "guest status should succeed");

    let resp = handle_request(Request::ListChannels, state.clone()).await;
    assert!(resp.ok, "guest list_channels should succeed");

    let resp = handle_request(
        Request::ListUsers {
            include_archived: false,
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "guest list_users should succeed");
}

#[tokio::test]
async fn test_archive_card_rejected_in_guest_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_guest_state(tmp.path());

    let resp = handle_request(
        Request::ArchiveCard {
            channel: "dev".to_string(),
            card_id: "20260101-120000-abc".to_string(),
            author: "alice".to_string(),
        },
        state,
    )
    .await;

    assert!(!resp.ok, "guest archive_card should fail");
    assert!(
        resp.error.as_deref().unwrap().contains("guest"),
        "error should mention guest mode: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_unarchive_card_rejected_in_guest_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_guest_state(tmp.path());

    let resp = handle_request(
        Request::UnarchiveCard {
            channel: "dev".to_string(),
            card_id: "20260101-120000-abc".to_string(),
            author: "alice".to_string(),
        },
        state,
    )
    .await;

    assert!(!resp.ok, "guest unarchive_card should fail");
    assert!(
        resp.error.as_deref().unwrap().contains("guest"),
        "error should mention guest mode: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn test_list_archived_cards_allowed_in_guest_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_guest_state(tmp.path());

    let resp = handle_request(Request::ListArchivedCards { channel: None }, state).await;

    assert!(
        resp.ok,
        "guest list_archived_cards should succeed (read-only): {:?}",
        resp.error
    );
}

// ─── Card poll tests ────────────────────────────────────────────────

async fn poll_cursor(state: &SharedState) -> String {
    let resp = handle_request(Request::Poll { since: None }, state.clone()).await;
    resp.data.unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn do_create_card(state: &SharedState, channel: &str, title: &str, author: &str) -> String {
    let resp = handle_request(
        Request::CreateCard {
            channel: channel.to_string(),
            title: title.to_string(),
            labels: None,
            assignee: None,
            status: None,
            author: Some(author.to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "create_card failed: {:?}", resp.error);
    resp.data.unwrap()["card_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_poll_surfaces_card_meta() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "dev", "alice");
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("alice".to_string());
    }

    state.git_storage.push().ok();
    let cursor = poll_cursor(&state).await;

    let card_id = do_create_card(&state, "dev", "Implement X", "alice").await;
    state.git_storage.push().ok();

    let resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "poll failed: {:?}", resp.error);
    let data = resp.data.unwrap();
    let changes = data["changes"].as_array().unwrap().clone();

    let card_channel_key = format!("card:dev/{}", card_id);
    let card_meta_change = changes
        .iter()
        .find(|c| c["kind"] == "card_meta" && c["channel"] == card_channel_key);
    assert!(
        card_meta_change.is_some(),
        "expected card_meta change for '{}', got: {:?}",
        card_channel_key,
        changes
    );

    // Update status -> should produce another card_meta event
    let cursor2 = data["commit_id"].as_str().unwrap().to_string();
    let upd = handle_request(
        Request::UpdateCard {
            channel: "dev".to_string(),
            card_id: card_id.clone(),
            status: Some("doing".to_string()),
            labels: None,
            assignee: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(upd.ok, "update_card failed: {:?}", upd.error);
    state.git_storage.push().ok();

    let resp2 = handle_request(
        Request::Poll {
            since: Some(cursor2),
        },
        state.clone(),
    )
    .await;
    let changes2 = resp2.data.unwrap()["changes"].as_array().unwrap().clone();
    assert!(
        changes2
            .iter()
            .any(|c| c["kind"] == "card_meta" && c["channel"] == card_channel_key),
        "expected card_meta event after status update, got: {:?}",
        changes2
    );
}

#[tokio::test]
async fn test_poll_card_create_with_assignee_surfaces_assignment_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "dev", "alice");
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("bob".to_string());
    }

    state.git_storage.push().ok();
    let cursor = poll_cursor(&state).await;

    let created = handle_request(
        Request::CreateCard {
            channel: "dev".to_string(),
            title: "Implement X".to_string(),
            labels: None,
            assignee: Some("bob".to_string()),
            status: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(created.ok, "create_card failed: {:?}", created.error);
    let card_id = created.data.unwrap()["card_id"]
        .as_str()
        .unwrap()
        .to_string();
    state.git_storage.push().ok();

    let resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "poll failed: {:?}", resp.error);
    let changes = resp.data.unwrap()["changes"].as_array().unwrap().clone();

    let card_channel_key = format!("card:dev/{}", card_id);
    let card_meta_change = changes
        .iter()
        .find(|c| c["kind"] == "card_meta" && c["channel"] == card_channel_key)
        .expect("expected card_meta change");
    let entries = card_meta_change["entries"]
        .as_array()
        .expect("card_meta should include routed entries for assignment");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["type"], "card_event");
    assert_eq!(entry["event_type"], "card_assignment");
    assert_eq!(entry["assignee"], "bob");
    assert_eq!(entry["mentions"], serde_json::json!([]));
}

#[tokio::test]
async fn test_poll_card_update_assignee_surfaces_assignment_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "dev", "alice");
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("bob".to_string());
    }

    let card_id = do_create_card(&state, "dev", "Implement Y", "alice").await;
    state.git_storage.push().ok();
    let cursor = poll_cursor(&state).await;

    let updated = handle_request(
        Request::UpdateCard {
            channel: "dev".to_string(),
            card_id: card_id.clone(),
            status: None,
            labels: None,
            assignee: Some("bob".to_string()),
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(updated.ok, "update_card failed: {:?}", updated.error);
    state.git_storage.push().ok();

    let resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "poll failed: {:?}", resp.error);
    let changes = resp.data.unwrap()["changes"].as_array().unwrap().clone();

    let card_channel_key = format!("card:dev/{}", card_id);
    let card_meta_change = changes
        .iter()
        .find(|c| c["kind"] == "card_meta" && c["channel"] == card_channel_key)
        .expect("expected card_meta change");
    let entries = card_meta_change["entries"]
        .as_array()
        .expect("card_meta should include routed entries for assignment");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["type"], "card_event");
    assert_eq!(entry["event_type"], "card_assignment");
    assert_eq!(entry["assignee"], "bob");
}

#[tokio::test]
async fn test_poll_card_create_with_title_mention_surfaces_mention_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "charlie").await;
    create_test_channel(&state, "dev", "alice");
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("charlie".to_string());
    }

    state.git_storage.push().ok();
    let cursor = poll_cursor(&state).await;

    let created = handle_request(
        Request::CreateCard {
            channel: "dev".to_string(),
            title: "Follow up with <@charlie>".to_string(),
            labels: None,
            assignee: None,
            status: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(created.ok, "create_card failed: {:?}", created.error);
    let card_id = created.data.unwrap()["card_id"]
        .as_str()
        .unwrap()
        .to_string();
    state.git_storage.push().ok();

    let resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    assert!(resp.ok, "poll failed: {:?}", resp.error);
    let changes = resp.data.unwrap()["changes"].as_array().unwrap().clone();

    let card_channel_key = format!("card:dev/{}", card_id);
    let card_meta_change = changes
        .iter()
        .find(|c| c["kind"] == "card_meta" && c["channel"] == card_channel_key)
        .expect("expected card_meta change");
    let entries = card_meta_change["entries"]
        .as_array()
        .expect("card_meta should include routed entries for title mention");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["type"], "card_event");
    assert_eq!(entry["event_type"], "card_mention");
    assert_eq!(entry["assignee"], serde_json::Value::Null);
    assert_eq!(entry["mentions"], serde_json::json!(["charlie"]));
}

#[tokio::test]
async fn test_poll_surfaces_card_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "dev", "alice");
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("alice".to_string());
    }

    let card_id = do_create_card(&state, "dev", "T", "alice").await;
    state.git_storage.push().ok();
    let cursor = poll_cursor(&state).await;

    let send = handle_request(
        Request::SendCardMessage {
            channel: "dev".to_string(),
            card_id: card_id.clone(),
            body: "hello from card".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(send.ok, "send_card_message failed: {:?}", send.error);
    state.git_storage.push().ok();

    let resp = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await;
    let changes = resp.data.unwrap()["changes"].as_array().unwrap().clone();

    let card_channel_key = format!("card:dev/{}", card_id);
    let thread_change = changes
        .iter()
        .find(|c| c["kind"] == "card_thread" && c["channel"] == card_channel_key)
        .expect("expected card_thread change");
    let entries = thread_change["entries"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "entries should contain the sent message"
    );
    let first = &entries[0];
    assert_eq!(first["author"], "alice");
    assert_eq!(first["body"], "hello from card");
    assert_eq!(first["type"], "message");
}

#[tokio::test]
async fn test_poll_filters_card_by_channel_membership() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_test_state(tmp.path());
    register_test_user(&state, "alice").await;
    register_test_user(&state, "bob").await;
    create_test_channel(&state, "general", "alice");
    create_test_channel(&state, "private", "alice");

    // Alice joins "private" so its members becomes non-empty (closed channel)
    let alice_join = handle_request(
        Request::JoinChannel {
            channel: "private".to_string(),
            targets: vec![],
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(alice_join.ok);

    // Acting as alice, create card in private
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("alice".to_string());
    }
    let card_id = do_create_card(&state, "private", "secret", "alice").await;
    let send = handle_request(
        Request::SendCardMessage {
            channel: "private".to_string(),
            card_id: card_id.clone(),
            body: "classified".to_string(),
            reply_to: None,
            author: Some("alice".to_string()),
        },
        state.clone(),
    )
    .await;
    assert!(send.ok);
    state.git_storage.push().ok();

    // Switch current_user to bob and poll from the beginning
    {
        let mut cu = state.current_user.write().await;
        *cu = Some("bob".to_string());
    }
    let resp = handle_request(Request::Poll { since: None }, state.clone()).await;
    let changes = resp.data.unwrap()["changes"].as_array().unwrap().clone();

    // Bob is NOT member of "private". He must not see the card events from it.
    let bob_saw_private_card = changes.iter().any(|c| {
        let ch = c["channel"].as_str().unwrap_or("");
        ch.starts_with("card:private/")
    });
    assert!(
        !bob_saw_private_card,
        "bob (non-member) should NOT see private channel cards in poll, got: {:?}",
        changes
    );
}

// ---------------------------------------------------------------------------
// handle_read since + limit semantics
//
// Three calling modes covered (see docs/plans/2026-05-11-channel-history-pagination/):
//   - limit only             → tail-cut, last N entries (channel open default)
//   - since only             → all entries after since (no truncation)
//   - since + limit          → head-cut, first N entries after since
//                              (incremental poll and history paging share this branch)
//
// These tests bypass `handle_send` to avoid one git commit per message — they
// write directly into the channel's `.thread` file using the same formatter the
// daemon uses, then drive `Request::Read` through the standard dispatch.
// ---------------------------------------------------------------------------

fn seed_channel_with_n_messages(state: &SharedState, channel: &str, count: u64) {
    use gitim_core::formatter::format_message;
    use gitim_core::types::Handler;
    use std::io::Write;

    let author = Handler::new("alice").unwrap();
    let thread_path = state
        .repo_root
        .join("channels")
        .join(format!("{}.thread", channel));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&thread_path)
        .unwrap();
    for i in 1..=count {
        let content = format_message(i, 0, &author, "20260511T120000Z", &format!("m{}", i));
        f.write_all(content.as_bytes()).unwrap();
    }
}

fn read_line_numbers(resp_data: serde_json::Value) -> Vec<u64> {
    resp_data["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["line_number"].as_u64().unwrap())
        .collect()
}

async fn setup_seeded_channel(tmp: &std::path::Path, count: u64) -> SharedState {
    let state = setup_test_state(tmp);
    register_test_user(&state, "alice").await;
    create_test_channel(&state, "general", "alice");
    seed_channel_with_n_messages(&state, "general", count);
    state
}

#[tokio::test]
async fn test_read_limit_only_returns_last_n() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 100).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: Some(50),
            since: None,
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    assert_eq!(
        read_line_numbers(resp.data.unwrap()),
        (51..=100).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_read_since_only_returns_all_after() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 100).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: None,
            since: Some(80),
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    assert_eq!(
        read_line_numbers(resp.data.unwrap()),
        (81..=100).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_read_since_with_limit_paging_back() {
    // Frontend history-paging use case: oldest in view = 951, fetch [901..=950]
    // by passing since = oldest - limit - 1 = 900.
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 1000).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: Some(50),
            since: Some(900),
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    assert_eq!(
        read_line_numbers(resp.data.unwrap()),
        (901..=950).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_read_since_with_limit_incremental_poll() {
    // Incremental poll: caller knows latest seen line = 50, asks for next 30.
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 100).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: Some(30),
            since: Some(50),
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    assert_eq!(
        read_line_numbers(resp.data.unwrap()),
        (51..=80).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_read_since_zero_with_limit_takes_first_n() {
    // since=0 + limit=N → retain line>0 (all entries) then head-truncate to first N.
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 100).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: Some(10),
            since: Some(0),
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    assert_eq!(
        read_line_numbers(resp.data.unwrap()),
        (1..=10).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_read_since_beyond_max_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 50).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: Some(50),
            since: Some(100),
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    let entries = resp.data.unwrap()["entries"]
        .as_array()
        .expect("entries array")
        .clone();
    assert!(entries.is_empty(), "expected empty, got {:?}", entries);
}

#[tokio::test]
async fn test_read_limit_zero_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let state = setup_seeded_channel(tmp.path(), 50).await;

    let resp = handle_request(
        Request::Read {
            channel: "general".to_string(),
            limit: Some(0),
            since: None,
        },
        state,
    )
    .await;
    assert!(resp.ok, "read failed: {:?}", resp.error);
    let entries = resp.data.unwrap()["entries"]
        .as_array()
        .expect("entries array")
        .clone();
    assert!(entries.is_empty(), "expected empty, got {:?}", entries);
}
