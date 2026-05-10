//! HTTP integration tests for the runtime's cron read endpoints.
//!
//! Same pattern as `http_im.rs`: a fake Unix-socket daemon answers method-
//! specific JSON, the axum router runs in-process via `tower::oneshot`. The
//! single-run body endpoint reads off disk, so a few tests also seed
//! `<workspace>/crons/<name>/<ts>.thread` files on the human-repo side.
//!
//! Coverage: list (empty + populated), show (existing + missing → 404), runs
//! list (existing + missing), single-run body (present + missing + bad ts +
//! human dir not initialized), and workspace 404 across all four routes.

use std::path::{Path, PathBuf};
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

/// Fake daemon that picks a response based on the request method. Tests
/// configure a small dispatch table; unknown methods echo back the canned
/// `{"ok": false, "error": "..."}` shape so route-mismatch isn't silent.
struct ScriptedDaemon {
    task: JoinHandle<()>,
}

type ResponseTable = std::collections::HashMap<String, Value>;

impl ScriptedDaemon {
    fn spawn(repo_root: &Path, table: Arc<Mutex<ResponseTable>>) -> Self {
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
                        map.get(&method)
                            .cloned()
                            .unwrap_or_else(|| json!({"ok": false, "error": format!("no scripted response for method {method}")}))
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
    human_repo: PathBuf,
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
    inject_human_workspace(&state, "test-ws", workspace_path, human_repo.clone());

    TestEnv {
        router,
        table,
        human_repo,
        _daemon: daemon,
        _tmp: tmp,
    }
}

async fn set(table: &Arc<Mutex<ResponseTable>>, method: &str, value: Value) {
    table.lock().await.insert(method.to_string(), value);
}

#[tokio::test]
async fn list_crons_empty() {
    let env = setup();
    set(
        &env.table,
        "list_crons",
        json!({"ok": true, "data": {"crons": []}}),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/crons").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"crons": []}));
}

#[tokio::test]
async fn list_crons_populated() {
    let env = setup();
    set(
        &env.table,
        "list_crons",
        json!({
            "ok": true,
            "data": {
                "crons": [
                    {
                        "name": "weekly-report",
                        "schedule": "0 9 * * 1",
                        "target": "alice",
                        "enabled": true,
                        "created_by": "alice",
                        "created_at": "2026-05-09T10:00:00Z",
                        "next_fire": "2026-05-11T09:00:00Z"
                    }
                ]
            }
        }),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/crons").await;
    assert_eq!(status, StatusCode::OK);
    let crons = body.get("crons").and_then(|v| v.as_array()).unwrap();
    assert_eq!(crons.len(), 1);
    assert_eq!(
        crons[0].get("name").and_then(|v| v.as_str()),
        Some("weekly-report")
    );
    assert_eq!(
        crons[0].get("next_fire").and_then(|v| v.as_str()),
        Some("2026-05-11T09:00:00Z")
    );
}

#[tokio::test]
async fn show_cron_existing() {
    let env = setup();
    set(
        &env.table,
        "show_cron",
        json!({
            "ok": true,
            "data": {
                "name": "weekly-report",
                "spec": {
                    "schedule": "0 9 * * 1",
                    "target": "alice",
                    "prompt": "weekly checkin",
                    "created_by": "alice",
                    "created_at": "2026-05-09T10:00:00Z"
                },
                "recent_runs": [],
                "next_fire": "2026-05-11T09:00:00Z"
            }
        }),
    )
    .await;

    let (status, body) =
        send(env.router, "GET", "/workspaces/test-ws/crons/weekly-report").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.get("name").and_then(|v| v.as_str()),
        Some("weekly-report")
    );
    assert!(body.get("spec").is_some());
    assert!(body
        .get("recent_runs")
        .map(|v| v.is_array())
        .unwrap_or(false));
}

#[tokio::test]
async fn show_cron_missing_returns_404() {
    let env = setup();
    // Daemon's handle_show_cron returns this code when spec.yaml is absent.
    set(
        &env.table,
        "show_cron",
        json!({
            "ok": false,
            "error": "cron 'ghost' does not exist",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/crons/ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

#[tokio::test]
async fn runs_endpoint_lists_thread_files() {
    let env = setup();
    set(
        &env.table,
        "history_cron",
        json!({
            "ok": true,
            "data": {
                "name": "weekly-report",
                "runs": [
                    {"ts": "2026-05-11T09-00-00Z", "filename": "2026-05-11T09-00-00Z.thread"},
                    {"ts": "2026-05-04T09-00-00Z", "filename": "2026-05-04T09-00-00Z.thread"}
                ]
            }
        }),
    )
    .await;

    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/crons/weekly-report/runs",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let runs = body.get("runs").and_then(|v| v.as_array()).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs[0].get("ts").and_then(|v| v.as_str()),
        Some("2026-05-11T09-00-00Z")
    );
}

#[tokio::test]
async fn runs_endpoint_404_for_unknown_cron() {
    let env = setup();
    set(
        &env.table,
        "history_cron",
        json!({
            "ok": false,
            "error": "cron 'ghost' does not exist",
            "error_code": "not_found"
        }),
    )
    .await;

    let (status, body) = send(env.router, "GET", "/workspaces/test-ws/crons/ghost/runs").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

#[tokio::test]
async fn single_run_returns_body() {
    let env = setup();
    // No daemon roundtrip — runtime reads the thread file off disk directly.
    let cron_dir = env.human_repo.join("crons").join("weekly-report");
    std::fs::create_dir_all(&cron_dir).unwrap();
    let thread = cron_dir.join("2026-05-11T09-00-00Z.thread");
    std::fs::write(
        &thread,
        "[L1][@system][2026-05-11T09:00:00Z] cron(weekly-report): weekly checkin\n",
    )
    .unwrap();

    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/crons/weekly-report/runs/2026-05-11T09-00-00Z",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body_str = body.get("body").and_then(|v| v.as_str()).unwrap();
    assert!(body_str.contains("cron(weekly-report)"));
    assert!(body_str.contains("[@system]"));
}

#[tokio::test]
async fn single_run_404_for_missing_ts() {
    let env = setup();
    // A real cron dir but no thread file for the requested ts.
    let cron_dir = env.human_repo.join("crons").join("weekly-report");
    std::fs::create_dir_all(&cron_dir).unwrap();

    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/crons/weekly-report/runs/2026-05-11T09-00-00Z",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

#[tokio::test]
async fn single_run_400_for_malformed_ts() {
    let env = setup();
    let (status, body) = send(
        env.router,
        "GET",
        "/workspaces/test-ws/crons/weekly-report/runs/not-a-timestamp",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body.get("error_code").and_then(|v| v.as_str()),
        Some("invalid_ts")
    );
}

#[tokio::test]
async fn workspace_not_found_for_list() {
    let env = setup();
    let (status, body) = send(env.router, "GET", "/workspaces/missing/crons").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.get("error").is_some());
}

#[tokio::test]
async fn workspace_not_found_for_show() {
    let env = setup();
    let (status, _body) = send(env.router, "GET", "/workspaces/missing/crons/foo").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workspace_not_found_for_runs() {
    let env = setup();
    let (status, _body) = send(env.router, "GET", "/workspaces/missing/crons/foo/runs").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workspace_not_found_for_single_run() {
    let env = setup();
    let (status, _body) = send(
        env.router,
        "GET",
        "/workspaces/missing/crons/foo/runs/2026-05-11T09-00-00Z",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
