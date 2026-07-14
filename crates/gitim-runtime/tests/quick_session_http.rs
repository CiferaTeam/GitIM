#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::quick_session_state::QuickSessionRuntimeState;
use gitim_runtime::workspace::WorkspaceContext;

const SESSION_ID: &str = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";

async fn send(
    router: axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
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
    fn spawn(repo_root: &Path, responses: HashMap<&str, Vec<Value>>) -> Self {
        let run_dir = repo_root.join(".gitim/run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let socket_path = run_dir.join("gitim.sock");
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let responses: HashMap<String, VecDeque<Value>> = responses
            .into_iter()
            .map(|(method, values)| (method.to_string(), values.into()))
            .collect();
        let responses = Arc::new(Mutex::new(responses));
        let (tx, requests) = mpsc::unbounded_channel();

        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let tx = tx.clone();
                let responses = responses.clone();
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = BufReader::new(reader);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let method = request["method"].as_str().unwrap().to_string();
                    let _ = tx.send(request);
                    let response = responses
                        .lock()
                        .unwrap()
                        .get_mut(&method)
                        .and_then(VecDeque::pop_front)
                        .unwrap_or_else(|| {
                            json!({
                                "ok": false,
                                "error": format!("unexpected method {method}"),
                                "error_code": "unexpected_method",
                            })
                        });
                    let _ = writer.write_all(format!("{response}\n").as_bytes()).await;
                });
            }
        });
        Self { requests, task }
    }

    async fn next_request(&mut self) -> Value {
        self.requests.recv().await.expect("daemon request")
    }

    fn try_next_request(&mut self) -> Option<Value> {
        self.requests.try_recv().ok()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn session_meta(status: &str) -> Value {
    json!({
        "id": SESSION_ID,
        "title": null,
        "title_source": "none",
        "agent_id": "alice",
        "created_by": "lewis",
        "status": status,
        "created_at": "2026-07-11T00:00:00Z",
        "updated_at": "2026-07-11T00:00:00Z",
        "last_message_preview": "hello",
        "revision": 1
    })
}

fn ok(data: Value) -> Value {
    json!({"ok": true, "data": data})
}

fn setup(responses: HashMap<&str, Vec<Value>>) -> (axum::Router, FakeDaemon, TempDir) {
    let tmp = tempfile::Builder::new()
        .prefix("qs")
        .tempdir_in("/tmp")
        .unwrap();
    let workspace = tmp.path().join("workspace");
    let human_repo = tmp.path().join("human");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&human_repo).unwrap();
    let daemon = FakeDaemon::spawn(&human_repo, responses);
    let (router, state) = create_router();
    inject_human_workspace(&state, "room", workspace, human_repo);
    (router, daemon, tmp)
}

#[test]
fn quick_session_state_roundtrips_atomically_with_private_permissions() {
    let tmp = TempDir::new().unwrap();
    let state = QuickSessionRuntimeState {
        estimated_tokens: 42,
        last_attempted_line: Some(7),
        ..QuickSessionRuntimeState::default()
    };

    state.save(tmp.path(), SESSION_ID).unwrap();
    assert_eq!(
        QuickSessionRuntimeState::load(tmp.path(), SESSION_ID).unwrap(),
        state
    );
    let path = QuickSessionRuntimeState::state_path(tmp.path(), SESSION_ID).unwrap();
    assert!(path.is_file());
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        1,
        "atomic save must not leave a temporary file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn quick_session_state_files_are_independent_and_generation_is_local() {
    let tmp = TempDir::new().unwrap();
    let other_id = "qs-01JYYYYYYYYYYYYYYYYYYYYYYY";
    let mut first = QuickSessionRuntimeState::default();
    assert_eq!(first.bump_context_generation(), 1);
    first.estimated_tokens = 10;
    let second = QuickSessionRuntimeState {
        estimated_tokens: 20,
        ..QuickSessionRuntimeState::default()
    };

    first.save(tmp.path(), SESSION_ID).unwrap();
    second.save(tmp.path(), other_id).unwrap();

    assert_eq!(
        QuickSessionRuntimeState::load(tmp.path(), SESSION_ID)
            .unwrap()
            .context_generation,
        1
    );
    assert_eq!(
        QuickSessionRuntimeState::load(tmp.path(), other_id)
            .unwrap()
            .estimated_tokens,
        20
    );
}

#[test]
fn quick_session_state_quarantines_corrupt_json_and_returns_default() {
    let tmp = TempDir::new().unwrap();
    let path = QuickSessionRuntimeState::state_path(tmp.path(), SESSION_ID).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{not-json").unwrap();

    assert_eq!(
        QuickSessionRuntimeState::load(tmp.path(), SESSION_ID).unwrap(),
        QuickSessionRuntimeState::default()
    );
    assert!(!path.exists());
    let corrupt_prefix = format!("{SESSION_ID}.state.json.corrupt-");
    assert!(std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with(&corrupt_prefix)));
}

#[tokio::test]
async fn quick_session_routes_map_typed_daemon_requests_and_responses() {
    let detail = json!({"meta": session_meta("needs_title"), "entries": [], "archived": false});
    let mut responses = HashMap::new();
    responses.insert("users", vec![ok(json!({"users": ["alice", "lewis"]}))]);
    responses.insert(
        "create_quick_session",
        vec![ok(json!({
            "session": detail.clone(),
            "line_number": 1,
            "ref": format!("session:{SESSION_ID}")
        }))],
    );
    responses.insert(
        "list_quick_sessions",
        vec![ok(json!({"sessions": [{
            "id": SESSION_ID,
            "title": null,
            "agent_id": "alice",
            "created_by": "lewis",
            "status": "needs_title",
            "updated_at": "2026-07-11T00:00:00Z",
            "last_message_preview": "hello",
            "revision": 1,
            "archived": false,
            "ref": format!("session:{SESSION_ID}")
        }]}))],
    );
    responses.insert("read_quick_session", vec![ok(json!({"session": detail}))]);
    responses.insert(
        "send_quick_session_message",
        vec![ok(json!({
            "session_id": SESSION_ID,
            "line_number": 2,
            "status": "needs_title",
            "revision": 2,
            "ref": format!("session:{SESSION_ID}:L000002")
        }))],
    );
    responses.insert(
        "archive_quick_session",
        vec![ok(json!({
            "session_id": SESSION_ID,
            "status": "archived",
            "revision": 3,
            "archived_at": "2026-07-11T00:01:00Z"
        }))],
    );
    responses.insert(
        "unarchive_quick_session",
        vec![ok(json!({
            "session_id": SESSION_ID,
            "status": "needs_title",
            "revision": 4
        }))],
    );
    let (router, mut daemon, _tmp) = setup(responses);

    let (status, create) = send(
        router.clone(),
        Method::POST,
        "/workspaces/room/im/quick-sessions",
        Some(json!({
            "session_id": SESSION_ID,
            "agent_id": "alice",
            "first_message": "hello"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(create["ok"], true);
    assert_eq!(create["data"]["session"]["meta"]["id"], SESSION_ID);

    let (status, list) = send(
        router.clone(),
        Method::GET,
        "/workspaces/room/im/quick-sessions?archived=false&agent_id=alice&actionable=true&limit=25",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["data"]["sessions"][0]["id"], SESSION_ID);

    let (status, read) = send(
        router.clone(),
        Method::GET,
        &format!("/workspaces/room/im/quick-sessions/{SESSION_ID}?limit=20&since=1"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["data"]["session"]["meta"]["agent_id"], "alice");

    let (status, sent) = send(
        router.clone(),
        Method::POST,
        &format!("/workspaces/room/im/quick-sessions/{SESSION_ID}/messages"),
        Some(json!({"body": "follow-up", "request_id": "req-1"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sent["data"]["line_number"], 2);

    let (status, archived) = send(
        router.clone(),
        Method::POST,
        &format!("/workspaces/room/im/quick-sessions/{SESSION_ID}/archive"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived["data"]["status"], "archived");

    let (status, unarchived) = send(
        router,
        Method::POST,
        &format!("/workspaces/room/im/quick-sessions/{SESSION_ID}/unarchive"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unarchived["data"]["status"], "needs_title");

    assert_eq!(daemon.next_request().await, json!({"method": "users"}));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "create_quick_session",
            "session_id": SESSION_ID,
            "agent_id": "alice",
            "first_message": "hello"
        })
    );
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "list_quick_sessions",
            "archived": false,
            "agent_id": "alice",
            "actionable": true,
            "limit": 25
        })
    );
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "read_quick_session",
            "session_id": SESSION_ID,
            "limit": 20,
            "since": 1
        })
    );
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "send_quick_session_message",
            "session_id": SESSION_ID,
            "body": "follow-up",
            "request_id": "req-1"
        })
    );
    assert_eq!(
        daemon.next_request().await,
        json!({"method": "archive_quick_session", "session_id": SESSION_ID})
    );
    assert_eq!(
        daemon.next_request().await,
        json!({"method": "unarchive_quick_session", "session_id": SESSION_ID})
    );
}

#[tokio::test]
async fn quick_session_create_rejects_non_active_handler_before_mutation() {
    let mut responses = HashMap::new();
    responses.insert("users", vec![ok(json!({"users": ["lewis"]}))]);
    let (router, mut daemon, _tmp) = setup(responses);

    let (status, body) = send(
        router,
        Method::POST,
        "/workspaces/room/im/quick-sessions",
        Some(json!({
            "session_id": SESSION_ID,
            "agent_id": "departed-agent",
            "first_message": "hello"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "quick_session_agent_not_found");
    assert_eq!(daemon.next_request().await, json!({"method": "users"}));
    assert!(daemon.try_next_request().is_none());
}

#[tokio::test]
async fn quick_session_routes_preserve_daemon_guest_error() {
    let mut responses = HashMap::new();
    responses.insert("users", vec![ok(json!({"users": ["alice", "lewis"]}))]);
    responses.insert(
        "create_quick_session",
        vec![json!({
            "ok": false,
            "error": "guest mode: write operations are not allowed"
        })],
    );
    let (router, _daemon, _tmp) = setup(responses);

    let (status, body) = send(
        router,
        Method::POST,
        "/workspaces/room/im/quick-sessions",
        Some(json!({
            "session_id": SESSION_ID,
            "agent_id": "alice",
            "first_message": "hello"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "ok": false,
            "error": "guest mode: write operations are not allowed"
        })
    );
}

#[tokio::test]
async fn quick_session_send_preserves_typed_forbidden_error() {
    let mut responses = HashMap::new();
    responses.insert(
        "send_quick_session_message",
        vec![json!({
            "ok": false,
            "error": "quick session actor is not authorized for this transition",
            "error_code": "quick_session_forbidden"
        })],
    );
    let (router, _daemon, _tmp) = setup(responses);

    let (status, body) = send(
        router,
        Method::POST,
        &format!("/workspaces/room/im/quick-sessions/{SESSION_ID}/messages"),
        Some(json!({"body": "follow-up", "request_id": "req-1"})),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error_code"], "quick_session_forbidden");
    assert_eq!(
        body["error"],
        "quick session actor is not authorized for this transition"
    );
}

#[tokio::test]
async fn quick_session_send_requires_non_empty_request_id_before_daemon_call() {
    for body in [
        json!({"body": "follow-up"}),
        json!({
            "body": "follow-up",
            "request_id": "  "
        }),
    ] {
        let (router, mut daemon, _tmp) = setup(HashMap::new());
        let (status, response) = send(
            router,
            Method::POST,
            &format!("/workspaces/room/im/quick-sessions/{SESSION_ID}/messages"),
            Some(body),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(response["error_code"], "invalid_quick_session_request_id");
        assert!(daemon.try_next_request().is_none());
    }
}

#[tokio::test]
async fn quick_session_path_routes_reject_invalid_workspace_slug() {
    let (router, mut daemon, _tmp) = setup(HashMap::new());
    let cases = [
        (
            Method::GET,
            format!("/workspaces/UPPER/im/quick-sessions/{SESSION_ID}"),
            None,
        ),
        (
            Method::POST,
            format!("/workspaces/UPPER/im/quick-sessions/{SESSION_ID}/messages"),
            Some(json!({"body": "follow-up", "request_id": "req-1"})),
        ),
        (
            Method::POST,
            format!("/workspaces/UPPER/im/quick-sessions/{SESSION_ID}/archive"),
            None,
        ),
        (
            Method::POST,
            format!("/workspaces/UPPER/im/quick-sessions/{SESSION_ID}/unarchive"),
            None,
        ),
    ];

    for (method, uri, body) in cases {
        let (status, response) = send(router.clone(), method, &uri, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert!(
            response["error"]
                .as_str()
                .is_some_and(|error| error.contains("invalid slug")),
            "{response}"
        );
    }
    assert!(daemon.try_next_request().is_none());
}

#[tokio::test]
async fn quick_session_routes_resolve_persisted_human_repo() {
    let tmp = tempfile::Builder::new()
        .prefix("qs-persisted")
        .tempdir_in("/tmp")
        .unwrap();
    let workspace = tmp.path().join("workspace");
    let human_repo = workspace.join(".gitim-runtime/human");
    std::fs::create_dir_all(human_repo.join(".git")).unwrap();
    std::fs::create_dir_all(human_repo.join(".gitim")).unwrap();
    std::fs::write(human_repo.join(".gitim/me.json"), r#"{"handler":"lewis"}"#).unwrap();
    let mut responses = HashMap::new();
    responses.insert("list_quick_sessions", vec![ok(json!({"sessions": []}))]);
    let mut daemon = FakeDaemon::spawn(&human_repo, responses);
    let (router, state) = create_router();
    state.lock().unwrap().workspaces.insert(
        "room".to_string(),
        WorkspaceContext::new("room".to_string(), "room".to_string(), workspace),
    );

    let (status, body) = send(
        router,
        Method::GET,
        "/workspaces/room/im/quick-sessions",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, ok(json!({"sessions": []})));
    assert_eq!(
        daemon.next_request().await,
        json!({
            "method": "list_quick_sessions",
            "archived": false,
            "actionable": false
        })
    );
}
