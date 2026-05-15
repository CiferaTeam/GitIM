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

    let frame = tokio::time::timeout(Duration::from_secs(3), events_body.next())
        .await
        .expect("fleet event should arrive without restarting runtime")
        .expect("fleet stream should yield a frame")
        .expect("fleet frame should be ok");
    let text = std::str::from_utf8(&frame).expect("fleet frame utf8");
    assert!(text.contains("\"node_id\":\"remote-runtime-a\""), "{text}");
    assert!(text.contains("\"node_ip\":\"100.64.0.10\""), "{text}");
    assert!(text.contains("\"workspace_id\":\"room\""), "{text}");
    assert!(text.contains("\"agent_id\":\"cfo\""), "{text}");
    assert!(text.contains("remote event arrived"), "{text}");

    sender.abort();
    remote_server.abort();
}
