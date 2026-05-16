//! Integration tests for the routing-recipients field that daemon poll
//! attaches to each message entry.
//!
//! Channel messages get `recipients` computed from `gitim_core::recipients`
//! (channel owner + parent chain + explicit mentions). DM messages get
//! `recipients = sorted member pair`. Event entries never carry recipients.

use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_core::types::Config;
use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

fn run_git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build a temp git repo with alice/bob/charlie registered, alice as the
/// daemon's identity. Returns `(tmp, state, initial_cursor)` where
/// `initial_cursor` is the commit id at setup time — pass it as `since`
/// for the test's poll call.
async fn setup_repo() -> (TempDir, Arc<AppState>, String) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("dm")).unwrap();
    for h in ["alice", "bob", "charlie"] {
        std::fs::write(
            root.join(format!("users/{}.meta.yaml", h)),
            format!("display_name: {}\nrole: dev\nintroduction: hi\n", h),
        )
        .unwrap();
    }

    run_git(&root, &["init"]);
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "init"]);

    let (tx, _) = broadcast::channel::<Event>(100);
    let state = Arc::new(AppState::new(
        root.clone(),
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec![
            "alice".to_string(),
            "bob".to_string(),
            "charlie".to_string(),
        ];
    }

    // Get the cursor as of the initial commit so test polls only see
    // diffs we add below.
    let cursor_resp = handle_request(Request::Poll { since: None }, state.clone()).await;
    let cursor = cursor_resp.data.unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    (tmp, state, cursor)
}

/// Channel messages must carry `recipients` computed via the 3-rule
/// routing policy (owner + parent chain + mentions, sorted-deduped).
#[tokio::test]
async fn poll_attaches_recipients_to_channel_message_entries() {
    let (_tmp, state, cursor) = setup_repo().await;
    let root = state.repo_root.clone();

    // Write channel meta: alice as owner, all three as members.
    std::fs::write(
        root.join("channels/general.meta.yaml"),
        "display_name: General\n\
         created_by: alice\n\
         created_at: 2026-05-17T00:00:00Z\n\
         introduction: ''\n\
         members:\n  - alice\n  - bob\n  - charlie\n",
    )
    .unwrap();

    // Two messages: alice root, bob reply with explicit @charlie mention.
    std::fs::write(
        root.join("channels/general.thread"),
        "[L000001][P000000][@alice][20260517T100000Z] hello\n\
         [L000002][P000001][@bob][20260517T100100Z] hi <@charlie>\n",
    )
    .unwrap();

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "add channel"]);

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
    let general = changes
        .iter()
        .find(|c| c["channel"] == "general" && c["kind"] == "channel")
        .expect("expected a channel-kind change for 'general'");
    let entries = general["entries"].as_array().unwrap();
    assert!(entries.len() >= 2, "expected at least two entries");

    // alice's root: rule 1 only → ["alice"]
    let r1: Vec<String> = entries[0]["recipients"]
        .as_array()
        .expect("recipients on root message")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(r1, vec!["alice".to_string()]);

    // bob's reply @charlie: rule 1 (alice) + rule 2 (alice from parent) +
    // rule 3 (charlie) → ["alice", "charlie"] after dedup+sort
    let r2: Vec<String> = entries[1]["recipients"]
        .as_array()
        .expect("recipients on reply message")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(r2, vec!["alice".to_string(), "charlie".to_string()]);
}

/// DM messages bypass the 3-rule policy: recipients is always the sorted
/// member pair derived from the filename stem.
#[tokio::test]
async fn poll_attaches_recipients_to_dm_message_entries() {
    let (_tmp, state, cursor) = setup_repo().await;
    let root = state.repo_root.clone();

    std::fs::write(
        root.join("dm/alice--bob.thread"),
        "[L000001][P000000][@alice][20260517T100000Z] hey bob\n",
    )
    .unwrap();

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "add dm"]);

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
    let dm = changes
        .iter()
        .find(|c| c["kind"] == "dm")
        .expect("expected a dm-kind change");
    let entries = dm["entries"].as_array().unwrap();
    let recipients: Vec<String> = entries[0]["recipients"]
        .as_array()
        .expect("recipients on dm message")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    // Sorted member pair.
    assert_eq!(recipients, vec!["alice".to_string(), "bob".to_string()]);
}

/// Event entries (join, leave, etc.) must NOT carry recipients —
/// routing is a per-message concept and events are workspace-wide.
#[tokio::test]
async fn poll_does_not_attach_recipients_to_event_entries() {
    let (_tmp, state, cursor) = setup_repo().await;
    let root = state.repo_root.clone();

    // Channel meta + thread file containing only an event line. The
    // canonical join-event shape uses point_to=000000 and a leading
    // event marker.
    std::fs::write(
        root.join("channels/general.meta.yaml"),
        "display_name: General\n\
         created_by: alice\n\
         created_at: 2026-05-17T00:00:00Z\n\
         introduction: ''\n\
         members:\n  - alice\n",
    )
    .unwrap();
    std::fs::write(
        root.join("channels/general.thread"),
        "[L000001][P000000][@alice][20260517T100000Z] \
         <event type=\"join\">{}</event>\n",
    )
    .unwrap();

    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "add channel with event"]);

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
    let general = changes
        .iter()
        .find(|c| c["channel"] == "general" && c["kind"] == "channel")
        .expect("expected channel-kind change");
    let entries = general["entries"].as_array().unwrap();

    // Find any event entries (the parser surfaces them as type:"event")
    // and assert they do NOT carry recipients. We don't assert *which*
    // entries are events because the parser's event-detection logic is
    // not in this test's scope — we just check the invariant.
    for entry in entries {
        if entry["type"] == "event" {
            assert!(
                entry.get("recipients").is_none(),
                "event entry must not carry recipients: {:#?}",
                entry
            );
        }
    }
}
