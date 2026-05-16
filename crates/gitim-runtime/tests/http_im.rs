//! HTTP integration tests for the runtime's `/workspaces/{slug}/im/*` proxy
//! routes.
//!
//! These tests route requests through the real axum router and terminate the
//! daemon side with a tiny Unix-socket test double. That keeps coverage at the
//! HTTP/runtime boundary without spawning a real daemon process.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

async fn send(router: axum::Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
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

struct FakeDaemon {
    requests: mpsc::UnboundedReceiver<Value>,
    task: JoinHandle<()>,
}

impl FakeDaemon {
    fn spawn(repo_root: &Path) -> Self {
        let run_dir = repo_root.join(".gitim/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let socket_path = run_dir.join("gitim.sock");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let (tx, requests) = mpsc::unbounded_channel();

        let task = tokio::spawn(async move {
            while let Ok((stream, _addr)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = BufReader::new(reader);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }

                    let request: Value = serde_json::from_str(&line).unwrap();
                    let _ = tx.send(request);

                    let mut response = json!({
                        "ok": true,
                        "data": { "accepted": true }
                    })
                    .to_string();
                    response.push('\n');
                    let _ = writer.write_all(response.as_bytes()).await;
                });
            }
        });

        Self { requests, task }
    }

    async fn next_request(&mut self) -> Value {
        self.requests.recv().await.expect("daemon request")
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn setup() -> (axum::Router, FakeDaemon, TempDir) {
    let tmp = TempDir::new().unwrap();
    let workspace_path = tmp.path().join("workspace");
    let human_repo = tmp.path().join("human");
    std::fs::create_dir_all(&workspace_path).unwrap();
    std::fs::create_dir_all(&human_repo).unwrap();

    let daemon = FakeDaemon::spawn(&human_repo);
    let (router, state) = create_router();
    inject_human_workspace(&state, "test-ws", workspace_path, human_repo);

    (router, daemon, tmp)
}

#[tokio::test]
async fn create_channel_forwards_invitees() {
    let (router, mut daemon, _tmp) = setup();
    let (status, body) = send(
        router,
        "POST",
        "/workspaces/test-ws/im/create-channel",
        json!({
            "name": "team",
            "display_name": "Team",
            "introduction": "Project channel",
            "invitees": ["bob", "carol"],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok": true, "data": {"accepted": true}}));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "create_channel",
            "name": "team",
            "display_name": "Team",
            "introduction": "Project channel",
            "invitees": ["bob", "carol"],
        })
    );
}

#[tokio::test]
async fn create_channel_legacy_body_forwards_empty_invitees() {
    let (router, mut daemon, _tmp) = setup();
    let (status, body) = send(
        router,
        "POST",
        "/workspaces/test-ws/im/create-channel",
        json!({ "name": "legacy" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok": true, "data": {"accepted": true}}));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "create_channel",
            "name": "legacy",
            "display_name": null,
            "introduction": null,
            "invitees": [],
        })
    );
}

#[tokio::test]
async fn join_channel_forwards_targets() {
    let (router, mut daemon, _tmp) = setup();
    let (status, body) = send(
        router,
        "POST",
        "/workspaces/test-ws/im/join",
        json!({
            "channel": "team",
            "targets": ["bob", "carol"],
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok": true, "data": {"accepted": true}}));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "join_channel",
            "channel": "team",
            "targets": ["bob", "carol"],
        })
    );
}

#[tokio::test]
async fn join_channel_legacy_body_forwards_empty_targets() {
    let (router, mut daemon, _tmp) = setup();
    let (status, body) = send(
        router,
        "POST",
        "/workspaces/test-ws/im/join",
        json!({ "channel": "team" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok": true, "data": {"accepted": true}}));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "join_channel",
            "channel": "team",
            "targets": [],
        })
    );
}

#[tokio::test]
async fn list_archived_channels_forwards_pagination_query() {
    let (router, mut daemon, _tmp) = setup();
    let (status, body) = send(
        router,
        "GET",
        "/workspaces/test-ws/im/channels/archived?offset=10&limit=25",
        Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"ok": true, "data": {"accepted": true}}));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "archived_channels",
            "offset": 10,
            "limit": 25,
        })
    );
}
