//! Fleet observer HTTP tests.
//!
//! These target the optional multi-node observer path: adding a remote node via
//! the running runtime should persist the node and start the SSE subscription
//! immediately, without requiring a restart.

use std::convert::Infallible;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures::{Stream, StreamExt};
use http_body_util::BodyExt;
use serde_json::json;
use serial_test::serial;
use tokio::sync::broadcast;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, AgentActivityEvent};
use gitim_runtime::user_config;

mod common;
use common::HomeGuard;

async fn remote_agent_events(
    State(tx): State<broadcast::Sender<AgentActivityEvent>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = tx.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|result| {
        futures::future::ready(match result {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(SseEvent::default().data(data)))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        })
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn spawn_remote_runtime() -> (
    String,
    broadcast::Sender<AgentActivityEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, _) = broadcast::channel(16);
    let app = Router::new()
        .route("/workspaces/room/agents/events", get(remote_agent_events))
        .with_state(tx.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind remote test runtime");
    let addr = listener.local_addr().expect("remote addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("remote server");
    });
    (format!("http://{addr}"), tx, handle)
}

fn post_fleet_node(base_url: &str) -> Request<Body> {
    let body = json!({
        "node_id": "remote-runtime-a",
        "base_url": base_url,
        "node_ip": "100.64.0.10",
        "node_name": "mac-mini",
        "workspaces": ["room"],
    });

    Request::builder()
        .method(Method::POST)
        .uri("/fleet/nodes")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn fleet_events_request() -> Request<Body> {
    Request::builder()
        .uri("/fleet/events")
        .body(Body::empty())
        .unwrap()
}

fn fleet_status_request() -> Request<Body> {
    Request::builder()
        .uri("/fleet/status")
        .body(Body::empty())
        .unwrap()
}

async fn response_json(resp: axum::response::Response) -> serde_json::Value {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("response body is JSON")
}

async fn wait_for_status(
    router: Router,
    node_id: &str,
    workspace: &str,
    status: &str,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let resp = router
            .clone()
            .oneshot(fleet_status_request())
            .await
            .expect("fleet status response");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = response_json(resp).await;
        if let Some(entry) = body["nodes"].as_array().and_then(|nodes| {
            nodes.iter().find(|entry| {
                entry["node_id"] == node_id
                    && entry["workspace_id"] == workspace
                    && entry["status"] == status
            })
        }) {
            return entry.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "status {status} for {node_id}/{workspace} did not appear; last body: {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_status_with_last_event(
    router: Router,
    node_id: &str,
    workspace: &str,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let entry = wait_for_status(router.clone(), node_id, workspace, "connected").await;
        if entry["last_event_at"].as_str().is_some() {
            return entry;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "connected status for {node_id}/{workspace} did not record last_event_at; last entry: {entry}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_frame_containing(
    body: &mut (impl Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
    needle: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let frame = tokio::time::timeout(Duration::from_millis(500), body.next())
            .await
            .expect("fleet stream should produce frames")
            .expect("fleet stream should not end")
            .expect("fleet frame should be ok");
        let text = std::str::from_utf8(&frame)
            .expect("fleet frame utf8")
            .to_string();
        if text.contains(needle) {
            return text;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "frame containing {needle:?} did not arrive; last frame: {text}"
        );
    }
}

#[tokio::test]
#[serial(home_env)]
async fn add_fleet_node_hot_subscribes_remote_sse() {
    let home_guard = HomeGuard::install();
    let (remote_base_url, remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, _state) = create_router();

    let events_resp = router
        .clone()
        .oneshot(fleet_events_request())
        .await
        .expect("fleet events response");
    assert_eq!(events_resp.status(), StatusCode::OK);
    let mut events_body = events_resp.into_body().into_data_stream();

    let add_resp = router
        .clone()
        .oneshot(post_fleet_node(&remote_base_url))
        .await
        .expect("add fleet node response");
    assert_eq!(add_resp.status(), StatusCode::OK);

    let cfg = user_config::read_from(Some(&home_guard.path().join(".gitim/runtime.json")));
    assert_eq!(cfg.fleet_nodes.len(), 1);
    assert_eq!(cfg.fleet_nodes[0].node_id, "remote-runtime-a");

    let sender = tokio::spawn(async move {
        for _ in 0..20 {
            let _ = remote_tx.send(AgentActivityEvent {
                agent_id: "cfo".to_string(),
                workspace_id: "room".to_string(),
                event_type: "tool_use".to_string(),
                detail: "remote event arrived".to_string(),
                timestamp: "2026-05-15T00:00:00Z".to_string(),
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let text = wait_for_frame_containing(&mut events_body, "remote event arrived").await;
    assert!(text.contains("\"node_id\":\"remote-runtime-a\""), "{text}");
    assert!(text.contains("\"node_ip\":\"100.64.0.10\""), "{text}");
    assert!(text.contains("\"workspace_id\":\"room\""), "{text}");
    assert!(text.contains("\"agent_id\":\"cfo\""), "{text}");
    assert!(text.contains("remote event arrived"), "{text}");

    sender.abort();
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_status_tracks_connected_and_last_event() {
    let _home_guard = HomeGuard::install();
    let (remote_base_url, remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, _state) = create_router();

    let events_resp = router
        .clone()
        .oneshot(fleet_events_request())
        .await
        .expect("fleet events response");
    assert_eq!(events_resp.status(), StatusCode::OK);
    let mut events_body = events_resp.into_body().into_data_stream();

    let add_resp = router
        .clone()
        .oneshot(post_fleet_node(&remote_base_url))
        .await
        .expect("add fleet node response");
    assert_eq!(add_resp.status(), StatusCode::OK);

    let sender = tokio::spawn(async move {
        for _ in 0..20 {
            let _ = remote_tx.send(AgentActivityEvent {
                agent_id: "cfo".to_string(),
                workspace_id: "room".to_string(),
                event_type: "tool_use".to_string(),
                detail: "updates status".to_string(),
                timestamp: "2026-05-15T00:00:00Z".to_string(),
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let status_event =
        wait_for_frame_containing(&mut events_body, "\"status\":\"connected\"").await;
    assert!(
        status_event.contains("\"kind\":\"node_status\""),
        "{status_event}"
    );

    let entry = wait_for_status_with_last_event(router, "remote-runtime-a", "room").await;
    assert_eq!(entry["node_ip"], "100.64.0.10");
    assert_eq!(entry["node_name"], "mac-mini");
    assert!(
        entry["last_connected_at"].as_str().is_some(),
        "connected node should record last_connected_at: {entry}"
    );
    assert!(entry["last_event_at"].as_str().is_some());

    sender.abort();
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_status_marks_unreachable_node_down() {
    let _home_guard = HomeGuard::install();
    let (router, _state) = create_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind unused port");
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let add_resp = router
        .clone()
        .oneshot(post_fleet_node(&base_url))
        .await
        .expect("add fleet node response");
    assert_eq!(add_resp.status(), StatusCode::OK);

    let entry = wait_for_status(router, "remote-runtime-a", "room", "down").await;
    assert!(
        entry["retry_count"].as_u64().unwrap_or_default() >= 1,
        "down node should increment retry_count: {entry}"
    );
    assert!(
        entry["last_error"].as_str().is_some(),
        "down node should retain last_error: {entry}"
    );
}
