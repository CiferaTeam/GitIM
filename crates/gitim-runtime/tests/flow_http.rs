#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HTTP integration tests for the runtime's flow read endpoints.
//!
//! Mirrors the `cron_http.rs` pattern: a fake Unix-socket daemon answers
//! method-specific JSON, the axum router runs in-process via `tower::oneshot`.
//!
//! Coverage: show existing (200), show missing (404), show daemon error (non-2xx),
//! list empty (200), validate missing (404).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

async fn send(router: axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn send_json(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn inject_human_workspace(
    state: &SharedRuntimeState,
    slug: &str,
    workspace_path: PathBuf,
    human_repo: PathBuf,
) {
    let mut ctx = WorkspaceContext::new(slug.to_string(), slug.to_string(), workspace_path);
    ctx.human_repo = Some(human_repo);
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug.to_string(), ctx);
}

type ResponseTable = std::collections::HashMap<String, Value>;

struct ScriptedDaemon {
    task: JoinHandle<()>,
}

impl ScriptedDaemon {
    fn spawn(repo_root: &std::path::Path, table: Arc<Mutex<ResponseTable>>) -> Self {
        let run_dir = repo_root.join(".gitim/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let socket_path = run_dir.join("gitim.sock");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();

        let task = tokio::spawn(async move {
            while let Ok((stream, _addr)) = listener.accept().await {
                let table = table.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = BufReader::new(reader);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let request: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => return,
                    };
                    let method = request
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    let resp_value = {
                        let map = table.lock().await;
                        map.get(&method).cloned().unwrap_or_else(|| {
                            json!({"ok": false, "error": format!("no scripted response for method {method}")})
                        })
                    };
                    let mut serialized = resp_value.to_string();
                    serialized.push('\n');
                    let _ = writer.write_all(serialized.as_bytes()).await;
                });
            }
        });

        Self { task }
    }
}

impl Drop for ScriptedDaemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct TestEnv {
    router: axum::Router,
    table: Arc<Mutex<ResponseTable>>,
    _daemon: ScriptedDaemon,
    _tmp: TempDir,
}

fn setup() -> TestEnv {
    let tmp = TempDir::new().unwrap();
    let workspace_path = tmp.path().join("workspace");
    let human_repo = tmp.path().join("human");
    std::fs::create_dir_all(&workspace_path).unwrap();
    std::fs::create_dir_all(&human_repo).unwrap();

    let table: Arc<Mutex<ResponseTable>> = Arc::new(Mutex::new(ResponseTable::new()));
    let daemon = ScriptedDaemon::spawn(&human_repo, table.clone());
    let (router, state) = create_router();
    inject_human_workspace(&state, "test-ws", workspace_path, human_repo);

    TestEnv {
        router,
        table,
        _daemon: daemon,
        _tmp: tmp,
    }
}

async fn set(table: &Arc<Mutex<ResponseTable>>, method: &str, value: Value) {
    table.lock().await.insert(method.to_string(), value);
}

// -- flows/replace (PUT) --

#[tokio::test]
async fn replace_flow_returns_200_with_data() {
    let env = setup();
    set(
        &env.table,
        "flow_replace",
        json!({
            "ok": true,
            "data": {
                "slug": "release",
                "path": "flows/release/index.md",
                "status": "committed",
                "commit_id": "abc123"
            }
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "PUT",
        "/workspaces/test-ws/im/flows/release",
        json!({
            "nodes": [
                {"id": "changelog", "type": "agent_mention", "owner": "alice", "prompt": "gen changelog"}
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.get("slug").and_then(|v| v.as_str()), Some("release"));
}

#[tokio::test]
async fn replace_flow_missing_returns_404() {
    let env = setup();
    set(
        &env.table,
        "flow_replace",
        json!({
            "ok": false,
            "error": "flow not found: ghost",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "PUT",
        "/workspaces/test-ws/im/flows/ghost",
        json!({ "nodes": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

#[tokio::test]
async fn replace_flow_cycle_returns_422() {
    let env = setup();
    // Daemon validate failure (cycle) returns ok=false with NO error_code
    // (Response::error), which flow_write_response maps to 422.
    set(
        &env.table,
        "flow_replace",
        json!({
            "ok": false,
            "error": "cycle detected in flow DAG"
        }),
    )
    .await;

    let (status, _body) = send_json(
        env.router,
        "PUT",
        "/workspaces/test-ws/im/flows/release",
        json!({
            "nodes": [
                {"id": "a", "type": "agent_mention", "owner": "x", "needs": ["b"]},
                {"id": "b", "type": "agent_mention", "owner": "x", "needs": ["a"]}
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

// -- flows/show --

#[tokio::test]
async fn show_flow_existing_returns_200() {
    let env = setup();
    set(
        &env.table,
        "flow_show",
        json!({
            "ok": true,
            "data": {
                "slug": "daily-digest",
                "name": "Daily Digest",
                "description": "Morning briefing",
                "created_by": "alice",
                "created_at": "2026-05-01T09:00:00Z"
            }
        }),
    )
    .await;

    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/im/flows/daily-digest",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.get("slug").and_then(|v| v.as_str()),
        Some("daily-digest")
    );
}

#[tokio::test]
async fn show_flow_missing_returns_404() {
    let env = setup();
    set(
        &env.table,
        "flow_show",
        json!({
            "ok": false,
            "error": "flow 'ghost' does not exist",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/im/flows/ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

#[tokio::test]
async fn show_flow_daemon_error_returns_non_2xx() {
    let env = setup();
    // Any daemon-side error other than not_found must not return 200.
    set(
        &env.table,
        "flow_show",
        json!({
            "ok": false,
            "error": "internal error reading flow",
            "error_code": "io_error"
        }),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/im/flows/some-flow").await;
    assert!(
        !status.is_success(),
        "expected non-2xx status for daemon error, got {status}"
    );
    assert_eq!(body.get("ok").and_then(|v| v.as_bool()), Some(false));
}

// -- flows/list --

#[tokio::test]
async fn list_flows_empty_returns_200() {
    let env = setup();
    set(
        &env.table,
        "flow_list",
        json!({"ok": true, "data": {"flows": []}}),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/im/flows").await;
    assert_eq!(status, StatusCode::OK);
    let flows = body.get("flows").and_then(|v| v.as_array()).unwrap();
    assert!(flows.is_empty());
}

// -- flows/validate --

#[tokio::test]
async fn validate_flow_missing_returns_404() {
    let env = setup();
    set(
        &env.table,
        "flow_validate",
        json!({
            "ok": false,
            "error": "flow 'ghost' does not exist",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/im/flows/ghost/validate",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

// -- flow runs --

#[tokio::test]
async fn flow_run_start_returns_run_id() {
    let env = setup();
    set(
        &env.table,
        "flow_run_start",
        json!({
            "ok": true,
            "data": {
                "run_id": "20260518T120000-deadbe",
                "flow_slug": "release",
                "channel": "release-discuss",
                "status": "pending"
            }
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "POST",
        "/workspaces/test-ws/im/flows/release/runs",
        json!({"channel": "release-discuss"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.get("run_id").and_then(|v| v.as_str()),
        Some("20260518T120000-deadbe")
    );
    assert_eq!(
        body.get("flow_slug").and_then(|v| v.as_str()),
        Some("release")
    );
    assert_eq!(
        body.get("channel").and_then(|v| v.as_str()),
        Some("release-discuss")
    );
}

#[tokio::test]
async fn flow_run_show_for_unknown_run_returns_404() {
    let env = setup();
    set(
        &env.table,
        "flow_run_show",
        json!({
            "ok": false,
            "error": "run not found",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/im/runs/20260518T120000-deadbe",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}
