use serde_json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
    let state = Arc::new(AppState::new(root, make_config(), event_tx, Some("alice".to_string())));

    {
        let mut users = state.users.write().await;
        users.push("alice".to_string());
    }

    (tmp, state)
}

#[test]
fn event_thread_changed_serializes_to_expected_json() {
    use gitim_daemon::api::Event;

    let event = Event::ThreadChanged {
        channel: "general".to_string(),
        kind: "channel".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "thread_changed");
    assert_eq!(json["channel"], "general");
    assert_eq!(json["kind"], "channel");
}

#[test]
fn event_thread_changed_dm_kind() {
    use gitim_daemon::api::Event;

    let event = Event::ThreadChanged {
        channel: "lewis--nexus".to_string(),
        kind: "dm".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "thread_changed");
    assert_eq!(json["kind"], "dm");
}

#[test]
fn event_messages_pushed_serializes() {
    use gitim_daemon::api::Event;

    let event = Event::MessagesPushed {
        channel: "general".to_string(),
        line_numbers: vec![1, 2],
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "messages_pushed");
    assert_eq!(json["channel"], "general");
    assert_eq!(json["line_numbers"], serde_json::json!([1, 2]));
}

#[test]
fn event_message_renumbered_serializes() {
    use gitim_daemon::api::Event;

    let event = Event::MessageRenumbered {
        channel: "general".to_string(),
        old_line: 5,
        new_line: 12,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "message_renumbered");
    assert_eq!(json["channel"], "general");
    assert_eq!(json["old_line"], 5);
    assert_eq!(json["new_line"], 12);
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
        author: Some("alice".to_string()),
    };
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let event = rx.try_recv().unwrap();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "thread_changed");
    assert_eq!(json["channel"], "general");
    assert_eq!(json["kind"], "channel");
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
        author: Some("alice".to_string()),
    };
    let resp = handle_request(req, state).await;
    assert!(resp.ok);

    let event = rx.try_recv().unwrap();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "thread_changed");
    assert_eq!(json["kind"], "dm");
}

#[tokio::test]
async fn unix_socket_subscribe_receives_push_events() {
    let (_tmp, state) = setup_test_repo().await;
    let socket_path = _tmp.path().join("test.sock");

    // Start socket server
    let server_state = state.clone();
    let server_path = socket_path.clone();
    tokio::spawn(async move {
        gitim_daemon::server::start_unix_socket(&server_path, server_state)
            .await
            .unwrap();
    });

    // Wait for socket to be ready
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Connect subscriber client
    let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Send subscribe request
    writer.write_all(b"{\"method\":\"subscribe\"}\n").await.unwrap();

    // Read subscribe response
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["data"]["subscribed"], true);
    line.clear();

    // Send a message via another connection to trigger broadcast
    let stream2 = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    let (reader2, mut writer2) = stream2.into_split();
    let mut reader2 = BufReader::new(reader2);
    writer2.write_all(b"{\"method\":\"send\",\"channel\":\"general\",\"body\":\"hello\",\"reply_to\":null,\"author\":\"alice\"}\n").await.unwrap();
    let mut line2 = String::new();
    reader2.read_line(&mut line2).await.unwrap(); // consume send response

    // Subscriber should receive push event
    let event_line = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        async {
            let mut l = String::new();
            reader.read_line(&mut l).await.unwrap();
            l
        }
    ).await.unwrap();

    let event: serde_json::Value = serde_json::from_str(&event_line).unwrap();
    assert_eq!(event["event"], "thread_changed");
    assert_eq!(event["channel"], "general");
    assert_eq!(event["kind"], "channel");
}

#[tokio::test]
async fn unix_socket_without_subscribe_no_push() {
    let (_tmp, state) = setup_test_repo().await;
    let socket_path = _tmp.path().join("test2.sock");

    let server_state = state.clone();
    let server_path = socket_path.clone();
    tokio::spawn(async move {
        gitim_daemon::server::start_unix_socket(&server_path, server_state)
            .await
            .unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Connect WITHOUT subscribing, just send a message
    let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    writer.write_all(b"{\"method\":\"send\",\"channel\":\"general\",\"body\":\"hello\",\"reply_to\":null,\"author\":\"alice\"}\n").await.unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["ok"], true);

    // Try to read more - should timeout (no push events)
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        async {
            let mut l = String::new();
            reader.read_line(&mut l).await.unwrap();
            l
        }
    ).await;

    assert!(result.is_err(), "should timeout - no push events without subscribe");
}

#[tokio::test]
async fn http_sse_receives_push_events() {
    let (_tmp, state) = setup_test_repo().await;

    let router = gitim_daemon::http::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    // Connect to SSE endpoint
    let client = reqwest::Client::new();
    let mut resp = client
        .get(format!("http://{}/api/events", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Send a message to trigger broadcast
    let send_req = serde_json::json!({
        "method": "send",
        "channel": "general",
        "body": "hello from sse test",
        "reply_to": null,
        "author": "alice"
    });
    let send_resp = client
        .post(format!("http://{}/api", addr))
        .json(&send_req)
        .send()
        .await
        .unwrap();
    assert!(send_resp.status().is_success());

    // Read SSE event
    let chunk = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        resp.chunk(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();

    let chunk_str = String::from_utf8(chunk.to_vec()).unwrap();
    assert!(chunk_str.contains("thread_changed"), "SSE should contain event data: {}", chunk_str);
    assert!(chunk_str.contains("general"), "SSE should contain channel name: {}", chunk_str);
}
