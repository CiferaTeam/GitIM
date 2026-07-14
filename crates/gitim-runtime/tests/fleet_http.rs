#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
use tokio::sync::{broadcast, Notify};
use tower::ServiceExt;

use gitim_runtime::fleet;
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, ActivityScope, AgentActivityEvent};
use gitim_runtime::user_config::{
    self, FleetNodeEntry, FleetWorkspaceMapping, UserConfig, WorkspaceEntry,
};
use gitim_runtime::workspace::WorkspaceContext;

mod common;
use common::HomeGuard;

const REMOTE_RUNTIME_ID: &str = "01234567-89ab-cdef-0123-456789abcdef";
const REMOTE_RUNTIME_ID_UPPERCASE: &str = "01234567-89AB-CDEF-0123-456789ABCDEF";
const QUICK_SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";

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

async fn remote_workspaces() -> axum::Json<serde_json::Value> {
    axum::Json(json!({
        "workspaces": [
            {
                "slug": "remote-room",
                "workspace_name": "Remote Room",
                "path": "/remote/room",
                "provider": "github",
                "initialized": true,
                "remote_identity": "github.com/org/repo",
            },
            {
                "slug": "local-only",
                "workspace_name": "Local Only",
                "path": "/remote/local-only",
                "provider": "local",
                "initialized": true,
            }
        ]
    }))
}

async fn remote_health() -> axum::Json<serde_json::Value> {
    axum::Json(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID_UPPERCASE,
    }))
}

async fn remote_agents() -> axum::Json<serde_json::Value> {
    axum::Json(json!({
        "ok": true,
        "agents": [
            {
                "id": "cfo",
                "handler": "cfo",
                "display_name": "cfo",
                "status": "running",
                "last_activity": "2026-05-15T00:00:00Z",
                "messages_processed": 13,
                "repo_path": "/remote/room/cfo",
                "provider": "codex",
                "model": "gpt-5.5",
                "usage_summary": {
                    "provider_reports_usage": true,
                    "first_seen": "2026-05-15T00:00:00Z",
                    "last_updated": "2026-05-15T00:10:00Z",
                    "totals": {
                        "input": 100,
                        "output": 20,
                        "cache_read": 300,
                        "cache_creation": 40,
                        "turns": 2
                    },
                    "today": {
                        "input": 30,
                        "output": 10,
                        "cache_read": 50,
                        "cache_creation": 0,
                        "turns": 1
                    },
                    "by_day": [
                        {
                            "date": "2026-05-15",
                            "bucket": {
                                "input": 30,
                                "output": 10,
                                "cache_read": 50,
                                "cache_creation": 0,
                                "turns": 1
                            }
                        }
                    ]
                }
            }
        ]
    }))
}

async fn spawn_remote_runtime() -> (
    String,
    broadcast::Sender<AgentActivityEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, _) = broadcast::channel(16);
    let app = Router::new()
        .route("/health", get(remote_health))
        .route("/workspaces", get(remote_workspaces))
        .route("/workspaces/{slug}/agents", get(remote_agents))
        .route("/workspaces/{slug}/agents/events", get(remote_agent_events))
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

async fn spawn_remote_runtime_without_health() -> (
    String,
    broadcast::Sender<AgentActivityEvent>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, _) = broadcast::channel(16);
    let app = Router::new()
        .route("/workspaces", get(remote_workspaces))
        .route("/workspaces/{slug}/agents", get(remote_agents))
        .route("/workspaces/{slug}/agents/events", get(remote_agent_events))
        .with_state(tx.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind legacy remote test runtime");
    let addr = listener.local_addr().expect("legacy remote addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("legacy remote server");
    });
    (format!("http://{addr}"), tx, handle)
}

async fn spawn_remote_runtime_with_health(
    health: serde_json::Value,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/health",
            get({
                let health = health.clone();
                move || {
                    let health = health.clone();
                    async move { axum::Json(health) }
                }
            }),
        )
        .route("/workspaces", get(remote_workspaces));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind remote health test runtime");
    let addr = listener.local_addr().expect("remote health addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("remote health server");
    });
    (format!("http://{addr}"), handle)
}

async fn chunked_oversized_health() -> axum::response::Response {
    let chunks = futures::stream::iter([
        Ok::<_, Infallible>(axum::body::Bytes::from(vec![b'x'; 40 * 1024])),
        Ok::<_, Infallible>(axum::body::Bytes::from(vec![b'y'; 40 * 1024])),
    ]);
    axum::response::Response::builder()
        .header("content-type", "application/json")
        .body(Body::from_stream(chunks))
        .unwrap()
}

async fn spawn_chunked_oversized_health_runtime() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().route("/health", get(chunked_oversized_health));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind chunked health runtime");
    let addr = listener.local_addr().expect("chunked health addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("chunked health server");
    });
    (format!("http://{addr}"), handle)
}

#[derive(Clone)]
struct DelayedHealth {
    entered: std::sync::Arc<Notify>,
    release: std::sync::Arc<Notify>,
}

async fn delayed_health(State(state): State<DelayedHealth>) -> axum::Json<serde_json::Value> {
    state.entered.notify_one();
    state.release.notified().await;
    remote_health().await
}

async fn spawn_delayed_health_runtime() -> (
    String,
    std::sync::Arc<Notify>,
    std::sync::Arc<Notify>,
    tokio::task::JoinHandle<()>,
) {
    let entered = std::sync::Arc::new(Notify::new());
    let release = std::sync::Arc::new(Notify::new());
    let state = DelayedHealth {
        entered: entered.clone(),
        release: release.clone(),
    };
    let app = Router::new()
        .route("/health", get(delayed_health))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed health runtime");
    let addr = listener.local_addr().expect("delayed health addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("delayed health server");
    });
    (format!("http://{addr}"), entered, release, handle)
}

async fn remote_agent_events_unavailable() -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

async fn spawn_remote_runtime_unavailable() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route("/health", get(remote_health))
        .route("/workspaces", get(remote_workspaces))
        .route(
            "/workspaces/{slug}/agents/events",
            get(remote_agent_events_unavailable),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind remote test runtime");
    let addr = listener.local_addr().expect("remote addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("remote server");
    });
    (format!("http://{addr}"), handle)
}

fn inject_github_workspace(
    state: &gitim_runtime::http::SharedRuntimeState,
    slug: &str,
    remote_url: &str,
) {
    let mut ctx = WorkspaceContext::new(
        slug.to_string(),
        format!("{slug} workspace"),
        std::path::PathBuf::from(format!("/tmp/{slug}")),
    );
    ctx.git_config = Some(WorkspaceConfig {
        workspace: format!("/tmp/{slug}"),
        created_at: "2026-05-15T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some(remote_url.to_string()),
            token: Some("tok".to_string()),
            github_email: None,
        },
    });
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

fn post_fleet_node_as(node_id: &str, base_url: &str) -> Request<Body> {
    let body = json!({
        "node_id": node_id,
        "base_url": base_url,
        "node_ip": "100.64.0.10",
        "node_name": "mac-mini",
        "workspaces": [],
    });

    Request::builder()
        .method(Method::POST)
        .uri("/fleet/nodes")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_fleet_node(base_url: &str) -> Request<Body> {
    post_fleet_node_as("remote-runtime-a", base_url)
}

fn fleet_node(
    node_id: &str,
    base_url: &str,
    runtime_id: Option<&str>,
    local_workspace_id: &str,
    remote_workspace_id: &str,
) -> FleetNodeEntry {
    FleetNodeEntry {
        node_id: node_id.to_string(),
        runtime_id: runtime_id.map(str::to_string),
        base_url: base_url.to_string(),
        node_ip: None,
        node_name: None,
        workspaces: vec![remote_workspace_id.to_string()],
        workspace_mappings: vec![FleetWorkspaceMapping {
            remote_workspace_id: remote_workspace_id.to_string(),
            local_workspace_id: local_workspace_id.to_string(),
            workspace_identity: "github.com/org/repo".to_string(),
        }],
        ssh_tunnel: None,
    }
}

fn bare_fleet_node(node_id: &str, base_url: &str, runtime_id: Option<&str>) -> FleetNodeEntry {
    FleetNodeEntry {
        node_id: node_id.to_string(),
        runtime_id: runtime_id.map(str::to_string),
        base_url: base_url.to_string(),
        node_ip: None,
        node_name: None,
        workspaces: Vec::new(),
        workspace_mappings: Vec::new(),
        ssh_tunnel: None,
    }
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

fn fleet_agents_request() -> Request<Body> {
    Request::builder()
        .uri("/fleet/agents")
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
async fn fleet_agents_lists_remote_agent_snapshots_with_node_metadata() {
    let _home_guard = HomeGuard::install();
    let (remote_base_url, _remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

    let add_resp = router
        .clone()
        .oneshot(post_fleet_node(&remote_base_url))
        .await
        .expect("add fleet node response");
    assert_eq!(add_resp.status(), StatusCode::OK);

    let resp = router
        .clone()
        .oneshot(fleet_agents_request())
        .await
        .expect("fleet agents response");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_json(resp).await;

    assert_eq!(body["ok"], true);
    let agents = body["agents"].as_array().expect("agents array");
    assert_eq!(agents.len(), 1, "{body}");
    let row = &agents[0];
    assert_eq!(row["node_id"], "remote-runtime-a");
    assert_eq!(row["node_ip"], "100.64.0.10");
    assert_eq!(row["node_name"], "mac-mini");
    assert_eq!(row["workspace_id"], "room");
    assert_eq!(row["remote_workspace_id"], "remote-room");
    assert_eq!(row["workspace_identity"], "github.com/org/repo");
    assert_eq!(row["agent"]["id"], "cfo");
    assert_eq!(row["agent"]["status"], "running");
    assert_eq!(row["agent"]["usage_summary"]["today"]["turns"], 1);

    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn add_fleet_node_hot_subscribes_remote_sse() {
    let home_guard = HomeGuard::install();
    let (remote_base_url, remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

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
    let add_body = response_json(add_resp).await;
    assert_eq!(add_body["node"]["runtime_id"], REMOTE_RUNTIME_ID);

    let cfg = user_config::read_from(Some(&home_guard.path().join(".gitim/runtime.json")));
    assert_eq!(cfg.fleet_nodes.len(), 1);
    assert_eq!(cfg.fleet_nodes[0].node_id, "remote-runtime-a");
    assert_eq!(
        cfg.fleet_nodes[0].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    let cfg_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home_guard.path().join(".gitim/runtime.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cfg_json["fleet_nodes"][0]["workspace_mappings"][0]["remote_workspace_id"],
        "remote-room"
    );
    assert_eq!(
        cfg_json["fleet_nodes"][0]["workspace_mappings"][0]["local_workspace_id"],
        "room"
    );
    assert_eq!(
        cfg_json["fleet_nodes"][0]["workspace_mappings"][0]["workspace_identity"],
        "github.com/org/repo"
    );

    let sender = tokio::spawn(async move {
        for _ in 0..20 {
            let _ = remote_tx.send(AgentActivityEvent {
                agent_id: "cfo".to_string(),
                workspace_id: "remote-room".to_string(),
                event_type: "tool_use".to_string(),
                detail: "remote event arrived".to_string(),
                timestamp: "2026-05-15T00:00:00Z".to_string(),
                scope: ActivityScope::default(),
                session_id: None,
                r#ref: None,
                session_revision: None,
                attempt_id: None,
                context_generation: None,
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let text = wait_for_frame_containing(&mut events_body, "remote event arrived").await;
    assert!(text.contains("\"node_id\":\"remote-runtime-a\""), "{text}");
    assert!(text.contains("\"node_ip\":\"100.64.0.10\""), "{text}");
    assert!(text.contains("\"workspace_id\":\"room\""), "{text}");
    assert!(
        text.contains("\"remote_workspace_id\":\"remote-room\""),
        "{text}"
    );
    assert!(
        text.contains("\"workspace_identity\":\"github.com/org/repo\""),
        "{text}"
    );
    assert!(text.contains("\"agent_id\":\"cfo\""), "{text}");
    assert!(text.contains("remote event arrived"), "{text}");

    sender.abort();
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_preserves_quick_session_activity_scope_fields() {
    let _home_guard = HomeGuard::install();
    let (remote_base_url, remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

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
                workspace_id: "remote-room".to_string(),
                event_type: "thinking".to_string(),
                detail: "scoped quick session event".to_string(),
                timestamp: "2026-07-11T00:00:00Z".to_string(),
                scope: ActivityScope::QuickSession,
                session_id: Some(QUICK_SESSION_ID.to_string()),
                r#ref: Some(format!("session:{QUICK_SESSION_ID}")),
                session_revision: Some(7),
                attempt_id: Some("qa-01JZZZZZZZZZZZZZZZZZZZZZZZ".to_string()),
                context_generation: Some(3),
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let text = wait_for_frame_containing(&mut events_body, "scoped quick session event").await;
    assert!(text.contains("\"scope\":\"quick_session\""), "{text}");
    assert!(
        text.contains(&format!("\"session_id\":\"{QUICK_SESSION_ID}\"")),
        "{text}"
    );
    assert!(text.contains("\"session_revision\":7"), "{text}");
    assert!(text.contains("\"context_generation\":3"), "{text}");

    sender.abort();
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_status_tracks_connected_and_last_event() {
    let _home_guard = HomeGuard::install();
    let (remote_base_url, remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

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
                workspace_id: "remote-room".to_string(),
                event_type: "tool_use".to_string(),
                detail: "updates status".to_string(),
                timestamp: "2026-05-15T00:00:00Z".to_string(),
                scope: ActivityScope::default(),
                session_id: None,
                r#ref: None,
                session_revision: None,
                attempt_id: None,
                context_generation: None,
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
    assert_eq!(entry["remote_workspace_id"], "remote-room");
    assert_eq!(entry["workspace_identity"], "github.com/org/repo");
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
    let (remote_base_url, remote_server) = spawn_remote_runtime_unavailable().await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

    let add_resp = router
        .clone()
        .oneshot(post_fleet_node(&remote_base_url))
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

    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_add_rejects_when_no_remote_identity_matches_local_workspaces() {
    let _home_guard = HomeGuard::install();
    let (remote_base_url, _remote_tx, remote_server) = spawn_remote_runtime().await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "different", "https://github.com/other/repo.git");

    let add_resp = router
        .clone()
        .oneshot(post_fleet_node(&remote_base_url))
        .await
        .expect("add fleet node response");
    assert_eq!(add_resp.status(), StatusCode::BAD_REQUEST);
    let body = response_json(add_resp).await;
    assert_eq!(body["error_code"], "no_matching_fleet_workspace");

    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_add_rejects_invalid_health_service_and_runtime_id() {
    let _home_guard = HomeGuard::install();

    for health in [
        json!({
            "service": "another-service",
            "runtime_id": REMOTE_RUNTIME_ID,
        }),
        json!({
            "service": "gitim-runtime",
            "runtime_id": "not-a-uuid",
        }),
    ] {
        let (remote_base_url, remote_server) = spawn_remote_runtime_with_health(health).await;
        let (router, state) = create_router();
        inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

        let response = router
            .oneshot(post_fleet_node(&remote_base_url))
            .await
            .expect("invalid fleet node response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error_code"], "invalid_fleet_node");

        remote_server.abort();
    }
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_runtime_id_is_unique_across_aliases_but_same_alias_can_update() {
    let _home_guard = HomeGuard::install();
    let (first_url, first_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID_UPPERCASE,
    }))
    .await;
    let (second_url, second_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID,
    }))
    .await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

    let first = router
        .clone()
        .oneshot(post_fleet_node_as("studio", &first_url))
        .await
        .expect("first fleet node response");
    assert_eq!(first.status(), StatusCode::OK);

    let update = router
        .clone()
        .oneshot(post_fleet_node_as("studio", &second_url))
        .await
        .expect("same-alias update response");
    assert_eq!(update.status(), StatusCode::OK);
    let update_body = response_json(update).await;
    assert_eq!(update_body["node"]["runtime_id"], REMOTE_RUNTIME_ID);
    assert_eq!(update_body["node"]["base_url"], second_url);

    let duplicate = router
        .oneshot(post_fleet_node_as("studio-copy", &first_url))
        .await
        .expect("duplicate fleet runtime response");
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    let duplicate_body = response_json(duplicate).await;
    assert_eq!(duplicate_body["error_code"], "duplicate_runtime_id");

    first_server.abort();
    second_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn legacy_fleet_node_activates_sse_when_health_is_unavailable() {
    let home_guard = HomeGuard::install();
    let (remote_url, remote_tx, remote_server) = spawn_remote_runtime_without_health().await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let mut config = UserConfig::default();
    config.upsert_fleet_node(fleet_node(
        "legacy-studio",
        &remote_url,
        None,
        "room",
        "remote-room",
    ));
    user_config::write_to(&config, &runtime_path).unwrap();

    let (router, state) = create_router();
    let events_response = router
        .clone()
        .oneshot(fleet_events_request())
        .await
        .expect("fleet events response");
    let mut events_body = events_response.into_body().into_data_stream();

    fleet::recover_from_config(state.clone());
    assert_eq!(
        state.lock().unwrap().fleet_nodes["legacy-studio"]
            .entry
            .runtime_id,
        None
    );

    let sender = tokio::spawn(async move {
        for _ in 0..20 {
            let _ = remote_tx.send(AgentActivityEvent {
                agent_id: "cfo".to_string(),
                workspace_id: "remote-room".to_string(),
                event_type: "tool_use".to_string(),
                detail: "legacy node stayed live".to_string(),
                timestamp: "2026-05-15T00:00:00Z".to_string(),
                scope: ActivityScope::default(),
                session_id: None,
                r#ref: None,
                session_revision: None,
                attempt_id: None,
                context_generation: None,
            });
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    let frame = wait_for_frame_containing(&mut events_body, "legacy node stayed live").await;
    assert!(frame.contains("\"node_id\":\"legacy-studio\""), "{frame}");

    sender.abort();
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn legacy_runtime_id_backfill_updates_live_node_and_config() {
    let home_guard = HomeGuard::install();
    let (remote_url, remote_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID_UPPERCASE,
    }))
    .await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let entry = fleet_node("legacy-studio", &remote_url, None, "room", "remote-room");
    let mut config = UserConfig::default();
    config.upsert_fleet_node(entry.clone());
    user_config::write_to(&config, &runtime_path).unwrap();
    let (_router, state) = create_router();
    fleet::activate_node(state.clone(), entry);

    let discovered = fleet::discover_legacy_runtime_id_once(&state, "legacy-studio")
        .await
        .expect("legacy identity discovery");

    assert_eq!(discovered.as_deref(), Some(REMOTE_RUNTIME_ID));
    assert_eq!(
        state.lock().unwrap().fleet_nodes["legacy-studio"]
            .entry
            .runtime_id
            .as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    assert_eq!(
        user_config::read_from(Some(&runtime_path)).fleet_nodes[0]
            .runtime_id
            .as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );

    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn stale_legacy_backfill_does_not_overwrite_replaced_alias() {
    let home_guard = HomeGuard::install();
    let (remote_url, entered, release, remote_server) = spawn_delayed_health_runtime().await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let original = fleet_node("legacy-studio", &remote_url, None, "room", "remote-room");
    let mut config = UserConfig::default();
    config.upsert_fleet_node(original.clone());
    user_config::write_to(&config, &runtime_path).unwrap();
    let (_router, state) = create_router();
    fleet::activate_node(state.clone(), original);

    let discovery_state = state.clone();
    let discovery = tokio::spawn(async move {
        fleet::discover_legacy_runtime_id_once(&discovery_state, "legacy-studio").await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("health discovery should start");

    let replacement_runtime_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let replacement = fleet_node(
        "legacy-studio",
        "http://127.0.0.1:9",
        Some(replacement_runtime_id),
        "room",
        "remote-room",
    );
    user_config::mutate_at(&runtime_path, |cfg| {
        cfg.upsert_fleet_node(replacement.clone())
    })
    .unwrap();
    fleet::activate_node(state.clone(), replacement.clone());
    release.notify_one();

    assert_eq!(discovery.await.unwrap().unwrap(), None);
    let live = &state.lock().unwrap().fleet_nodes["legacy-studio"].entry;
    assert_eq!(live.base_url, replacement.base_url);
    assert_eq!(live.runtime_id.as_deref(), Some(replacement_runtime_id));
    let disk = user_config::read_from(Some(&runtime_path));
    assert_eq!(disk.fleet_nodes[0].base_url, replacement.base_url);
    assert_eq!(
        disk.fleet_nodes[0].runtime_id.as_deref(),
        Some(replacement_runtime_id)
    );

    remote_server.abort();
}

#[tokio::test]
async fn peer_snapshot_includes_mapped_and_legacy_workspace_nodes() {
    let (_router, state) = create_router();
    fleet::activate_node(
        state.clone(),
        fleet_node(
            "mapped",
            "http://127.0.0.1:9",
            Some(REMOTE_RUNTIME_ID),
            "room",
            "remote-room",
        ),
    );
    fleet::activate_node(
        state.clone(),
        FleetNodeEntry {
            node_id: "legacy".to_string(),
            runtime_id: None,
            base_url: "http://127.0.0.1:10".to_string(),
            node_ip: None,
            node_name: None,
            workspaces: vec!["room".to_string()],
            workspace_mappings: Vec::new(),
            ssh_tunnel: None,
        },
    );
    fleet::activate_node(
        state.clone(),
        fleet_node(
            "other-workspace",
            "http://127.0.0.1:11",
            Some("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
            "other",
            "remote-other",
        ),
    );

    let peers = fleet::snapshot_workspace_peers(&state, "room");
    assert_eq!(peers.len(), 2);
    assert_eq!(peers[0].node_id, "legacy");
    assert_eq!(peers[0].runtime_id, None);
    assert_eq!(peers[0].base_url, "http://127.0.0.1:10");
    assert_eq!(peers[0].remote_workspace_id, "room");
    assert_eq!(peers[1].node_id, "mapped");
    assert_eq!(peers[1].runtime_id.as_deref(), Some(REMOTE_RUNTIME_ID));
    assert_eq!(peers[1].remote_workspace_id, "remote-room");
}

#[test]
#[serial(home_env)]
fn fleet_transition_serializes_same_alias_disk_and_live_updates() {
    let home_guard = HomeGuard::install();
    let (_router, state) = create_router();
    let first_entry = bare_fleet_node(
        "studio",
        "http://127.0.0.1:18001",
        Some("11111111-1111-4111-8111-111111111111"),
    );
    let second_entry = bare_fleet_node(
        "studio",
        "http://127.0.0.1:18002",
        Some("22222222-2222-4222-8222-222222222222"),
    );
    let transition_lock = state.lock().unwrap().fleet_transition_lock.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    std::thread::scope(|scope| {
        let first_state = state.clone();
        let first_persist = first_entry.clone();
        let first_live = first_entry.clone();
        let first = scope.spawn(move || {
            fleet::apply_fleet_transition(
                &first_state,
                move |config, _| config.upsert_fleet_node(first_persist),
                move |state, &()| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    fleet::activate_node(state.clone(), first_live);
                },
            )
            .unwrap();
        });
        entered_rx.recv().unwrap();
        assert!(matches!(
            transition_lock.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));

        let second_state = state.clone();
        let second_persist = second_entry.clone();
        let second_live = second_entry.clone();
        let second = scope.spawn(move || {
            fleet::apply_fleet_transition(
                &second_state,
                move |config, _| config.upsert_fleet_node(second_persist),
                move |state, &()| fleet::activate_node(state.clone(), second_live),
            )
            .unwrap();
        });

        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
    });

    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let disk = user_config::read_from(Some(&runtime_path));
    let live = state.lock().unwrap();
    assert_eq!(disk.fleet_nodes.len(), 1);
    assert_eq!(disk.fleet_nodes[0].base_url, second_entry.base_url);
    assert_eq!(
        live.fleet_nodes["studio"].entry.base_url,
        second_entry.base_url
    );
}

#[test]
#[serial(home_env)]
fn fleet_transition_serializes_upsert_delete_disk_and_live_order() {
    let home_guard = HomeGuard::install();
    let (_router, state) = create_router();
    let entry = bare_fleet_node(
        "studio",
        "http://127.0.0.1:18001",
        Some("11111111-1111-4111-8111-111111111111"),
    );
    let transition_lock = state.lock().unwrap().fleet_transition_lock.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    std::thread::scope(|scope| {
        let upsert_state = state.clone();
        let persist_entry = entry.clone();
        let live_entry = entry.clone();
        let upsert = scope.spawn(move || {
            fleet::apply_fleet_transition(
                &upsert_state,
                move |config, _| config.upsert_fleet_node(persist_entry),
                move |state, &()| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    fleet::activate_node(state.clone(), live_entry);
                },
            )
            .unwrap();
        });
        entered_rx.recv().unwrap();
        assert!(matches!(
            transition_lock.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));

        let delete_state = state.clone();
        let delete = scope.spawn(move || {
            fleet::apply_fleet_transition(
                &delete_state,
                |config, _| config.remove_fleet_node("studio"),
                |state, _| {
                    fleet::remove_node(state, "studio");
                },
            )
            .unwrap();
        });

        release_tx.send(()).unwrap();
        upsert.join().unwrap();
        delete.join().unwrap();
    });

    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let disk = user_config::read_from(Some(&runtime_path));
    let live = state.lock().unwrap();
    assert!(disk.fleet_nodes.is_empty());
    assert!(!live.fleet_nodes.contains_key("studio"));
}

#[tokio::test]
#[serial(home_env)]
async fn fleet_add_rejects_canonical_runtime_id_duplicate_from_legacy_config() {
    let home_guard = HomeGuard::install();
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let mut config = UserConfig::default();
    config.upsert_fleet_node(bare_fleet_node(
        "existing-alias",
        "http://127.0.0.1:17001",
        Some(REMOTE_RUNTIME_ID_UPPERCASE),
    ));
    user_config::write_to(&config, &runtime_path).unwrap();
    let (remote_url, remote_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID,
    }))
    .await;
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");

    let response = router
        .oneshot(post_fleet_node_as("new-alias", &remote_url))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error_code"],
        "duplicate_runtime_id"
    );
    assert_eq!(
        user_config::read_from(Some(&runtime_path))
            .fleet_nodes
            .len(),
        1
    );
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn legacy_backfill_matches_trailing_slash_base_url() {
    let home_guard = HomeGuard::install();
    let (remote_url, remote_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID,
    }))
    .await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let stored = bare_fleet_node("legacy", &format!("{remote_url}/"), None);
    let mut config = UserConfig::default();
    config.upsert_fleet_node(stored.clone());
    user_config::write_to(&config, &runtime_path).unwrap();
    let (_router, state) = create_router();
    fleet::activate_node(state.clone(), fleet::normalize_node(stored));

    let discovered = fleet::discover_legacy_runtime_id_once(&state, "legacy")
        .await
        .unwrap();

    assert_eq!(discovered.as_deref(), Some(REMOTE_RUNTIME_ID));
    assert_eq!(
        user_config::read_from(Some(&runtime_path)).fleet_nodes[0]
            .runtime_id
            .as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    remote_server.abort();
}

#[test]
#[serial(home_env)]
fn recovery_uses_first_valid_alias_and_runtime_id_occurrence() {
    let home_guard = HomeGuard::install();
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let first = bare_fleet_node("alpha", "http://127.0.0.1:17001", Some(REMOTE_RUNTIME_ID));
    let duplicate_alias = bare_fleet_node(
        " alpha ",
        "http://127.0.0.1:17002",
        Some("22222222-2222-4222-8222-222222222222"),
    );
    let duplicate_runtime = bare_fleet_node(
        "beta",
        "http://127.0.0.1:17003",
        Some(REMOTE_RUNTIME_ID_UPPERCASE),
    );
    let unique = bare_fleet_node(
        "gamma",
        "http://127.0.0.1:17004",
        Some("33333333-3333-4333-8333-333333333333"),
    );
    let config = UserConfig {
        fleet_nodes: vec![first.clone(), duplicate_alias, duplicate_runtime, unique],
        ..UserConfig::default()
    };
    user_config::write_to(&config, &runtime_path).unwrap();
    let (_router, state) = create_router();

    fleet::recover_from_config(state.clone());

    let live = state.lock().unwrap();
    assert_eq!(live.fleet_nodes.len(), 2);
    assert_eq!(live.fleet_nodes["alpha"].entry.base_url, first.base_url);
    assert!(live.fleet_nodes.contains_key("gamma"));
    assert!(!live.fleet_nodes.contains_key("beta"));
}

#[tokio::test]
async fn peer_snapshot_deduplicates_identical_workspace_mappings() {
    let (_router, state) = create_router();
    let mut entry = fleet_node(
        "mapped",
        "http://127.0.0.1:9",
        Some(REMOTE_RUNTIME_ID),
        "room",
        "remote-room",
    );
    entry
        .workspace_mappings
        .push(entry.workspace_mappings[0].clone());
    fleet::activate_node(state.clone(), entry);

    let peers = fleet::snapshot_workspace_peers(&state, "room");

    assert_eq!(peers.len(), 1);
}

#[tokio::test]
async fn remote_health_rejects_body_larger_than_64_kib() {
    let (remote_url, remote_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID,
        "padding": "x".repeat(70 * 1024),
    }))
    .await;

    let error = fleet::fetch_remote_runtime_id(&remote_url)
        .await
        .expect_err("oversized health response must be rejected");

    assert!(error.contains("64 KiB"), "{error}");
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn new_alias_waits_for_legacy_identity_then_rejects_canonical_duplicate() {
    let home_guard = HomeGuard::install();
    let (legacy_url, entered, release, legacy_server) = spawn_delayed_health_runtime().await;
    let (new_url, _new_tx, new_server) = spawn_remote_runtime().await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let legacy = fleet_node("legacy-a", &legacy_url, None, "room", "remote-room");
    let mut config = UserConfig::default();
    config.upsert_fleet_node(legacy.clone());
    user_config::write_to(&config, &runtime_path).unwrap();
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");
    fleet::activate_node(state.clone(), legacy);

    let post = tokio::spawn(async move {
        router
            .oneshot(post_fleet_node_as("new-b", &new_url))
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("new alias upsert must discover unresolved legacy identities");

    assert!(
        !post.is_finished(),
        "upsert committed while legacy identity was unresolved"
    );
    assert_eq!(
        user_config::read_from(Some(&runtime_path))
            .fleet_nodes
            .len(),
        1
    );
    assert_eq!(state.lock().unwrap().fleet_nodes.len(), 1);
    release.notify_one();

    let response = post.await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error_code"],
        "duplicate_runtime_id"
    );
    let disk = user_config::read_from(Some(&runtime_path));
    assert_eq!(disk.fleet_nodes.len(), 1);
    assert_eq!(disk.fleet_nodes[0].node_id, "legacy-a");
    assert_eq!(
        disk.fleet_nodes[0].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    let live = state.lock().unwrap();
    assert_eq!(live.fleet_nodes.len(), 1);
    assert!(live.fleet_nodes.contains_key("legacy-a"));

    legacy_server.abort();
    new_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn new_alias_returns_conflict_while_legacy_identity_stays_unresolved() {
    let home_guard = HomeGuard::install();
    let (new_url, _new_tx, new_server) = spawn_remote_runtime().await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let legacy = fleet_node(
        "offline-legacy",
        "http://127.0.0.1:9",
        None,
        "room",
        "remote-room",
    );
    let mut config = UserConfig::default();
    config.upsert_fleet_node(legacy.clone());
    user_config::write_to(&config, &runtime_path).unwrap();
    let (router, state) = create_router();
    inject_github_workspace(&state, "room", "https://github.com/org/repo.git");
    fleet::activate_node(state.clone(), legacy);

    let response = router
        .oneshot(post_fleet_node_as("new-b", &new_url))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response_json(response).await;
    assert_eq!(body["error_code"], "fleet_identity_unresolved");
    assert!(body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("offline-legacy"));
    assert_eq!(
        user_config::read_from(Some(&runtime_path))
            .fleet_nodes
            .len(),
        1
    );
    assert_eq!(state.lock().unwrap().fleet_nodes.len(), 1);
    new_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn duplicate_legacy_backfills_converge_to_earliest_config_entry() {
    let home_guard = HomeGuard::install();
    let (first_url, first_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID_UPPERCASE,
    }))
    .await;
    let (second_url, second_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID,
    }))
    .await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let first = fleet_node("first", &first_url, None, "room", "remote-room");
    let second = fleet_node("second", &second_url, None, "room", "remote-room");
    let config = UserConfig {
        fleet_nodes: vec![first.clone(), second.clone()],
        ..UserConfig::default()
    };
    user_config::write_to(&config, &runtime_path).unwrap();
    let (_router, state) = create_router();
    fleet::activate_node(state.clone(), first);
    fleet::activate_node(state.clone(), second);

    assert_eq!(
        fleet::discover_legacy_runtime_id_once(&state, "second")
            .await
            .unwrap()
            .as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    assert_eq!(
        fleet::discover_legacy_runtime_id_once(&state, "first")
            .await
            .unwrap()
            .as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );

    let disk = user_config::read_from(Some(&runtime_path));
    assert_eq!(disk.fleet_nodes.len(), 2);
    assert_eq!(
        disk.fleet_nodes[0].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    assert_eq!(
        disk.fleet_nodes[1].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    let live = state.lock().unwrap();
    assert_eq!(live.fleet_nodes.len(), 1);
    assert!(live.fleet_nodes.contains_key("first"));
    drop(live);
    assert_eq!(fleet::snapshot_workspace_peers(&state, "room").len(), 1);

    first_server.abort();
    second_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn parallel_legacy_backfills_preserve_concurrent_runtime_config_mutation() {
    let home_guard = HomeGuard::install();
    let (first_url, first_entered, first_release, first_server) =
        spawn_delayed_health_runtime().await;
    let (second_url, second_entered, second_release, second_server) =
        spawn_delayed_health_runtime().await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let first = fleet_node("first", &first_url, None, "room", "remote-first");
    let second = fleet_node("second", &second_url, None, "room", "remote-second");
    user_config::write_to(
        &UserConfig {
            fleet_nodes: vec![first.clone(), second.clone()],
            ..UserConfig::default()
        },
        &runtime_path,
    )
    .unwrap();
    let (_router, state) = create_router();
    fleet::activate_node(state.clone(), first.clone());
    fleet::activate_node(state.clone(), second.clone());

    let discovery_state = state.clone();
    let discovery = tokio::spawn(async move {
        fleet::discover_asset_legacy_identities(&discovery_state, "room", "github.com/org/repo")
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), first_entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), second_entered.notified())
        .await
        .unwrap();

    let concurrent = fleet_node(
        "concurrent",
        "http://127.0.0.1:19090",
        Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        "other-room",
        "remote-other",
    );
    user_config::mutate_at(&runtime_path, |config| {
        config.listen_port = Some(19090);
        config.workspaces.push(WorkspaceEntry {
            slug: "other-room".to_string(),
            workspace_name: "Other Room".to_string(),
            path: "/tmp/other-room".to_string(),
        });
        config.upsert_fleet_node(concurrent.clone());
    })
    .unwrap();

    second_release.notify_one();
    first_release.notify_one();
    discovery.await.unwrap();

    let disk = user_config::read_from(Some(&runtime_path));
    assert_eq!(disk.listen_port, Some(19090));
    assert_eq!(disk.workspaces.len(), 1);
    assert_eq!(disk.workspaces[0].slug, "other-room");
    assert_eq!(disk.fleet_nodes.len(), 3);
    assert_eq!(
        disk.fleet_nodes[0].workspace_mappings,
        first.workspace_mappings
    );
    assert_eq!(
        disk.fleet_nodes[1].workspace_mappings,
        second.workspace_mappings
    );
    assert_eq!(disk.fleet_nodes[2], concurrent);
    assert_eq!(
        disk.fleet_nodes[0].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    assert_eq!(
        disk.fleet_nodes[1].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    let live = state.lock().unwrap();
    assert!(live.fleet_nodes.contains_key("first"));
    assert!(!live.fleet_nodes.contains_key("second"));

    first_server.abort();
    second_server.abort();
}

#[tokio::test]
async fn remote_health_rejects_chunked_body_larger_than_64_kib() {
    let (remote_url, remote_server) = spawn_chunked_oversized_health_runtime().await;

    let error = fleet::fetch_remote_runtime_id(&remote_url)
        .await
        .expect_err("chunked oversized health response must be rejected");

    assert!(error.contains("64 KiB"), "{error}");
    remote_server.abort();
}

#[tokio::test]
#[serial(home_env)]
async fn invalid_earlier_config_entry_cannot_win_legacy_backfill() {
    let home_guard = HomeGuard::install();
    let (valid_url, valid_server) = spawn_remote_runtime_with_health(json!({
        "service": "gitim-runtime",
        "runtime_id": REMOTE_RUNTIME_ID,
    }))
    .await;
    let runtime_path = home_guard.path().join(".gitim/runtime.json");
    let invalid = bare_fleet_node(
        "invalid-first",
        "not-a-valid-url",
        Some(REMOTE_RUNTIME_ID_UPPERCASE),
    );
    assert!(fleet::validate_node(&fleet::normalize_node(invalid.clone())).is_err());
    let valid = fleet_node("valid-second", &valid_url, None, "room", "remote-room");
    let config = UserConfig {
        fleet_nodes: vec![invalid, valid.clone()],
        ..UserConfig::default()
    };
    user_config::write_to(&config, &runtime_path).unwrap();
    let (_router, state) = create_router();
    fleet::activate_node(state.clone(), valid);

    let discovered = fleet::discover_legacy_runtime_id_once(&state, "valid-second")
        .await
        .unwrap();

    assert_eq!(discovered.as_deref(), Some(REMOTE_RUNTIME_ID));
    let disk = user_config::read_from(Some(&runtime_path));
    assert_eq!(
        disk.fleet_nodes[1].runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    let live = state.lock().unwrap();
    assert_eq!(live.fleet_nodes.len(), 1);
    assert_eq!(
        live.fleet_nodes["valid-second"].entry.runtime_id.as_deref(),
        Some(REMOTE_RUNTIME_ID)
    );
    drop(live);
    assert_eq!(fleet::snapshot_workspace_peers(&state, "room").len(), 1);
    valid_server.abort();
}
