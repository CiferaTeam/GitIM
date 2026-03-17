use serde_json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

use gitim_daemon::api::{Event, Request};
use gitim_daemon::handlers::handle_request;
use gitim_daemon::state::AppState;
use gitim_core::types::Config;

fn make_config() -> Config {
    serde_yaml::from_str("version: 1").unwrap()
}

async fn setup_test_repo() -> (TempDir, Arc<AppState>) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();

    std::fs::create_dir_all(root.join("channels")).unwrap();
    std::fs::create_dir_all(root.join("users")).unwrap();
    std::fs::write(
        root.join("users/alice.meta.json"),
        r#"{"display_name":"Alice","role":"dev","introduction":"hi"}"#,
    ).unwrap();

    let (event_tx, _) = broadcast::channel::<Event>(256);
    let state = Arc::new(AppState::new(root, make_config(), event_tx));

    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

    (tmp, state)
}

#[test]
fn event_serializes_to_expected_json() {
    use gitim_daemon::api::Event;

    let event = Event {
        event: "thread_changed".to_string(),
        channel: "general".to_string(),
        kind: "channel".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "thread_changed");
    assert_eq!(json["channel"], "general");
    assert_eq!(json["kind"], "channel");
}

#[test]
fn event_dm_kind() {
    use gitim_daemon::api::Event;

    let event = Event {
        event: "thread_changed".to_string(),
        channel: "lewis--nexus".to_string(),
        kind: "dm".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "dm");
}

#[test]
fn subscribe_request_deserializes() {
    use gitim_daemon::api::Request;

    let json = r#"{"method": "subscribe"}"#;
    let req: Request = serde_json::from_str(json).unwrap();
    assert!(matches!(req, Request::Subscribe));
}

#[tokio::test]
async fn handle_send_broadcasts_channel_event() {
    let (_tmp, state) = setup_test_repo().await;
    let mut rx = state.event_tx.subscribe();

    let req = Request::Send {
        channel: "general".to_string(),
        body: "hello".to_string(),
        reply_to: None,
        author: "alice".to_string(),
    };
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let event = rx.try_recv().unwrap();
    assert_eq!(event.event, "thread_changed");
    assert_eq!(event.channel, "general");
    assert_eq!(event.kind, "channel");
}

#[tokio::test]
async fn handle_send_broadcasts_dm_event() {
    let (_tmp, state) = setup_test_repo().await;

    // Register bob too
    {
        let mut users = state.users.write().await;
        users.push("bob".to_string());
    }
    std::fs::write(
        state.repo_root.join("users/bob.meta.json"),
        r#"{"display_name":"Bob","role":"dev","introduction":"hi"}"#,
    ).unwrap();
    std::fs::create_dir_all(state.repo_root.join("dm")).unwrap();

    let mut rx = state.event_tx.subscribe();

    let req = Request::Send {
        channel: "dm:alice,bob".to_string(),
        body: "hey".to_string(),
        reply_to: None,
        author: "alice".to_string(),
    };
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let event = rx.try_recv().unwrap();
    assert_eq!(event.event, "thread_changed");
    assert_eq!(event.kind, "dm");
}
