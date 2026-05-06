//! Regression test: poll must surface channel archival as a `channel_meta`
//! event so the UI can refetch the archived list.
//!
//! Before this was wired up, `git diff` would show only the `archive/channels/*`
//! paths after an archive (the `channels/*` paths delete → don't appear in the
//! map of added content), and the poll handler had no branch for that prefix.
//! Net effect: a channel that was archived out-of-band silently vanished from
//! every UI surface. The human's clone would have the data on disk but no way
//! to know it had appeared.

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

async fn setup_state_with_alice() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.yaml"),
        "display_name: Alice\nrole: dev\nintroduction: hi\n",
    )
    .unwrap();

    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap()
    };
    run_git(&["init"]);
    run_git(&["add", "."]);
    run_git(&["commit", "-m", "init"]);

    let (tx, _) = broadcast::channel::<Event>(100);
    let state = Arc::new(AppState::new(
        root,
        make_config(),
        tx,
        Some("alice".to_string()),
    ));
    {
        let mut users = state.users.write().await;
        *users = vec!["alice".to_string()];
    }
    (tmp, state)
}

/// Archiving a channel must surface as a `channel_meta` event in the next
/// poll keyed by the plain channel name (not the `archive/channels/...`
/// path).
#[tokio::test]
async fn poll_surfaces_archived_channel_as_channel_meta() {
    let (_tmp, state) = setup_state_with_alice().await;

    // Create the channel
    let create_req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": "ephemeral",
        "author": "alice",
    }))
    .unwrap();
    let create_resp = handle_request(create_req, state.clone()).await;
    assert!(create_resp.ok, "create failed: {:?}", create_resp.error);

    // Take a cursor HERE — before archive, after create
    let poll_cursor_resp = handle_request(Request::Poll { since: None }, state.clone()).await;
    let cursor = poll_cursor_resp.data.unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Archive the channel
    let archive_req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_channel",
        "channel": "ephemeral",
        "author": "alice",
    }))
    .unwrap();
    let archive_resp = handle_request(archive_req, state.clone()).await;
    assert!(archive_resp.ok, "archive failed: {:?}", archive_resp.error);

    // Poll since the pre-archive cursor — must see a channel_meta event
    // for "ephemeral" (keyed off the bare name, not the archive path).
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
    let archived_meta_hit = changes
        .iter()
        .any(|c| c["kind"] == "channel_meta" && c["channel"] == "ephemeral");
    assert!(
        archived_meta_hit,
        "expected a channel_meta event for 'ephemeral' after archive, got: {:#?}",
        changes
    );
}

/// The synthetic change must use the bare channel name — not
/// `archive/channels/ephemeral` or similar — so the frontend's name-matching
/// path doesn't break.
#[tokio::test]
async fn archived_channel_meta_uses_bare_channel_name() {
    let (_tmp, state) = setup_state_with_alice().await;

    let create_req: Request = serde_json::from_value(serde_json::json!({
        "method": "create_channel",
        "name": "weird-name",
        "author": "alice",
    }))
    .unwrap();
    let _ = handle_request(create_req, state.clone()).await;

    let cursor = handle_request(Request::Poll { since: None }, state.clone())
        .await
        .data
        .unwrap()["commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    let archive_req: Request = serde_json::from_value(serde_json::json!({
        "method": "archive_channel",
        "channel": "weird-name",
        "author": "alice",
    }))
    .unwrap();
    let _ = handle_request(archive_req, state.clone()).await;

    let changes = handle_request(
        Request::Poll {
            since: Some(cursor),
        },
        state.clone(),
    )
    .await
    .data
    .unwrap()["changes"]
        .as_array()
        .cloned()
        .unwrap();

    for c in &changes {
        if c["kind"] == "channel_meta" {
            let ch = c["channel"].as_str().unwrap_or("");
            assert!(
                !ch.contains('/'),
                "channel name must not contain a '/': got {:?}",
                ch
            );
            assert!(
                !ch.starts_with("archive"),
                "channel name must not start with 'archive': got {:?}",
                ch
            );
        }
    }
}
