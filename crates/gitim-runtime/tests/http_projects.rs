#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HTTP integration tests for the `/im/projects` and
//! `/im/channels/{name}/project` runtime gateway endpoints.
//!
//! Uses the `ScriptedDaemon` pattern from `flow_http.rs`: a fake Unix-socket
//! daemon answers method-specific JSON, and the axum router runs in-process
//! via `tower::oneshot`. No real daemon is spawned.
//!
//! Coverage:
//! - GET /im/projects  → list_projects (200 with data, empty list)
//! - POST /im/projects → create_project (200 success, 200 + ok:false daemon error)
//! - PATCH /im/channels/{name}/project → set_channel_project (200 set, 200 clear)
//! - Write endpoints surface daemon errors (including departed-user) as
//!   200 with `ok: false` body — the api_response_to_json convention shared
//!   with /im/channels and /im/labels (not the flow-runs 422 style)
//! - Workspace 404 for unknown slug

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

// ── helpers ──────────────────────────────────────────────────────────────────

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

async fn script(table: &Arc<Mutex<ResponseTable>>, method: &str, value: Value) {
    table.lock().await.insert(method.to_string(), value);
}

// ── GET /im/projects ──────────────────────────────────────────────────────────

#[tokio::test]
async fn list_projects_returns_200_with_data() {
    let env = setup();
    script(
        &env.table,
        "list_projects",
        json!({
            "ok": true,
            "data": {
                "projects": [
                    {"slug": "alpha", "display_name": "Alpha", "introduction": "First project"}
                ]
            }
        }),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/im/projects").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    let projects = body["data"]["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["slug"], "alpha");
}

#[tokio::test]
async fn list_projects_returns_200_with_empty_list() {
    let env = setup();
    script(
        &env.table,
        "list_projects",
        json!({"ok": true, "data": {"projects": []}}),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/im/projects").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    assert_eq!(
        body["data"]["projects"].as_array().map(|a| a.len()),
        Some(0)
    );
}

#[tokio::test]
async fn list_projects_unknown_workspace_returns_404() {
    let env = setup();
    let (status, _body) = send(env.router, "GET", "/workspaces/no-such-ws/im/projects").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── POST /im/projects ────────────────────────────────────────────────────────

#[tokio::test]
async fn create_project_returns_200_on_success() {
    let env = setup();
    script(
        &env.table,
        "create_project",
        json!({
            "ok": true,
            "data": {
                "slug": "design",
                "display_name": "Design Sprint",
                "introduction": "UX work"
            }
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "POST",
        "/workspaces/test-ws/im/projects",
        json!({
            "slug": "design",
            "display_name": "Design Sprint",
            "introduction": "UX work"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    assert_eq!(body["data"]["slug"], "design");
}

#[tokio::test]
async fn create_project_daemon_error_passes_through() {
    // The daemon rejects the request (e.g. slug already taken).
    // api_response_to_json forwards the response body with ok=false.
    let env = setup();
    script(
        &env.table,
        "create_project",
        json!({
            "ok": false,
            "error": "project slug 'design' already exists"
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "POST",
        "/workspaces/test-ws/im/projects",
        json!({
            "slug": "design",
            "display_name": "Design Sprint",
            "introduction": "UX work"
        }),
    )
    .await;
    // api_response_to_json wraps daemon errors in a 200 body (same convention
    // as im_channels, im_labels — the frontend checks body.ok, not status).
    assert_eq!(status, StatusCode::OK);
    assert!(!body.get("ok").and_then(|v| v.as_bool()).unwrap_or(true));
}

#[tokio::test]
async fn create_project_departed_user_blocked() {
    // The daemon enforces ensure_author_not_departed. Script it to return the
    // same error it would emit for a departed user.
    let env = setup();
    script(
        &env.table,
        "create_project",
        json!({
            "ok": false,
            "error": "user @alice is departed"
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "POST",
        "/workspaces/test-ws/im/projects",
        json!({
            "slug": "design",
            "display_name": "D",
            "introduction": "x"
        }),
    )
    .await;
    // The runtime layer forwards the daemon's rejection verbatim.
    assert_eq!(status, StatusCode::OK);
    assert!(!body.get("ok").and_then(|v| v.as_bool()).unwrap_or(true));
    let error_msg = body["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("departed"),
        "expected departed error, got: {error_msg}"
    );
}

// ── PATCH /im/channels/{name}/project ────────────────────────────────────────

#[tokio::test]
async fn set_channel_project_returns_200_on_assign() {
    let env = setup();
    script(
        &env.table,
        "set_channel_project",
        json!({"ok": true, "data": {"channel": "general", "project": "design"}}),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "PATCH",
        "/workspaces/test-ws/im/channels/general/project",
        json!({"project": "design"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    assert_eq!(body["data"]["project"], "design");
}

#[tokio::test]
async fn set_channel_project_returns_200_on_clear() {
    // Sending `{"project": null}` clears the channel's project association.
    let env = setup();
    script(
        &env.table,
        "set_channel_project",
        json!({"ok": true, "data": {"channel": "general", "project": null}}),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "PATCH",
        "/workspaces/test-ws/im/channels/general/project",
        json!({"project": null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
}

#[tokio::test]
async fn set_channel_project_absent_key_treated_as_clear() {
    // When the `project` key is absent, serde's `#[serde(default)]` sets it
    // to None — same as null — and the daemon receives project=null (clear).
    let env = setup();
    script(
        &env.table,
        "set_channel_project",
        json!({"ok": true, "data": {"channel": "general", "project": null}}),
    )
    .await;

    let (status, _body) = send_json(
        env.router,
        "PATCH",
        "/workspaces/test-ws/im/channels/general/project",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn set_channel_project_daemon_error_passes_through() {
    let env = setup();
    script(
        &env.table,
        "set_channel_project",
        json!({
            "ok": false,
            "error": "channel 'no-such' not found",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send_json(
        env.router,
        "PATCH",
        "/workspaces/test-ws/im/channels/no-such/project",
        json!({"project": "alpha"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.get("ok").and_then(|v| v.as_bool()).unwrap_or(true));
}

#[tokio::test]
async fn set_channel_project_unknown_workspace_returns_404() {
    let env = setup();
    let (status, _body) = send_json(
        env.router,
        "PATCH",
        "/workspaces/no-such-ws/im/channels/general/project",
        json!({"project": "alpha"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
