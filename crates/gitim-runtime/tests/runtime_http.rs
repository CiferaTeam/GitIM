//! In-process HTTP tests for the runtime's `/preflight/{provider}` route.
//!
//! Uses `tower::ServiceExt::oneshot` to dispatch a single request through the
//! real axum router — no TCP listener, no spawned server, no port races.
//!
//! ## CLI isolation
//!
//! The handler delegates to `preflight_claude()` / `preflight_codex()`, which
//! spawn real CLIs via PATH lookup. On a developer machine with `claude` and
//! `codex` installed and logged in, a bare router-level test would burn real
//! LLM tokens every `cargo test` run (~5s, non-trivial cost).
//!
//! To keep these tests hermetic we override `PATH` to an empty tempdir around
//! the two provider-specific tests, forcing `spawn()` to return `NotFound` →
//! `ErrorKind::NotInstalled`. This exercises the exact same HTTP path the
//! WebUI hits when a CLI really is missing.
//!
//! The tests that mutate `PATH` are `#[serial(path_env)]` so they can't race
//! each other or any parallel test; the unknown-provider test doesn't touch
//! the environment and runs free.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serial_test::serial;
use std::collections::HashMap;
use tower::ServiceExt;

use gitim_runtime::http::{create_router, AgentInfo};

async fn body_to_json(resp: axum::response::Response) -> serde_json::Value {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("response body is JSON")
}

/// RAII guard that swaps `PATH` to an isolated empty directory and restores
/// the prior value on drop. Pairs with `#[serial(path_env)]` so only one
/// `PathGuard` is live at a time — avoiding the multi-threaded-set_var race.
struct PathGuard {
    original: Option<std::ffi::OsString>,
    _tmp: tempfile::TempDir,
}

impl PathGuard {
    /// Install an empty-directory PATH. Callers should drop this before any
    /// assertions that might re-enter user code depending on PATH.
    fn install_empty() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir for empty PATH");
        let original = std::env::var_os("PATH");
        std::env::set_var("PATH", tmp.path());
        Self {
            original,
            _tmp: tmp,
        }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("PATH", val),
            None => std::env::remove_var("PATH"),
        }
    }
}

#[tokio::test]
async fn test_preflight_unknown_provider_returns_400() {
    let (router, _state) = create_router();

    let response = router
        .oneshot(
            Request::builder()
                .uri("/preflight/fake")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    assert_eq!(
        body["error"],
        serde_json::Value::String("unknown provider".into())
    );
}

#[tokio::test]
#[serial(path_env)]
async fn test_preflight_claude_returns_result_shape() {
    let _path_guard = PathGuard::install_empty();
    let (router, _state) = create_router();

    let response = router
        .oneshot(
            Request::builder()
                .uri("/preflight/claude")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert_eq!(body["provider"], serde_json::Value::String("claude".into()));
    assert!(
        body.get("duration_ms").is_some(),
        "duration_ms missing: {body}"
    );
    // With PATH stripped, spawn must fail with NotFound → NotInstalled. This
    // is the same JSON shape the WebUI sees when a user hasn't installed the
    // CLI, so asserting on it gives us the stable contract for that branch.
    assert_eq!(
        body["available"],
        serde_json::Value::Bool(false),
        "body: {body}"
    );
    assert_eq!(
        body["error_kind"],
        serde_json::Value::String("not_installed".into()),
        "body: {body}",
    );
}

#[tokio::test]
#[serial(path_env)]
async fn test_preflight_codex_returns_result_shape() {
    let _path_guard = PathGuard::install_empty();
    let (router, _state) = create_router();

    let response = router
        .oneshot(
            Request::builder()
                .uri("/preflight/codex")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert_eq!(body["provider"], serde_json::Value::String("codex".into()));
    assert!(
        body.get("duration_ms").is_some(),
        "duration_ms missing: {body}"
    );
    assert_eq!(
        body["available"],
        serde_json::Value::Bool(false),
        "body: {body}"
    );
    assert_eq!(
        body["error_kind"],
        serde_json::Value::String("not_installed".into()),
        "body: {body}",
    );
}

// -- /agents/add provider-field guardrails --
//
// These tests rely on the fact that provider validation runs *before* any
// workspace or state check — so they don't need a provisioned workspace or
// human daemon. The happy-path "valid provider succeeds" case requires a real
// workspace + human daemon and belongs in E2E (Task 16).

fn agents_add_request(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/workspaces/test-ws/agents/add")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Inject a placeholder workspace so the /workspaces/{slug}/agents/add route
/// resolves. Provider validation runs before workspace state is read, so these
/// tests exercise the validation branch alone.
fn inject_ws(state: &gitim_runtime::http::SharedRuntimeState) {
    use std::path::PathBuf;
    inject_ws_at(state, PathBuf::from("/tmp/test-ws"));
}

fn inject_ws_at(state: &gitim_runtime::http::SharedRuntimeState, path: std::path::PathBuf) {
    let mut s = state.lock().unwrap();
    let ctx = gitim_runtime::workspace::WorkspaceContext::new(
        "test-ws".to_string(),
        "test-ws".to_string(),
        path,
    );
    s.workspaces.insert("test-ws".to_string(), ctx);
}

fn insert_agent(
    state: &gitim_runtime::http::SharedRuntimeState,
    id: &str,
    repo_path: &std::path::Path,
) {
    let mut s = state.lock().unwrap();
    let ctx = s.workspaces.get_mut("test-ws").expect("workspace exists");
    ctx.agents.insert(
        id.to_string(),
        AgentInfo {
            id: id.to_string(),
            handler: id.to_string(),
            display_name: id.to_string(),
            status: "idle".to_string(),
            last_activity: None,
            messages_processed: 0,
            repo_path: repo_path.display().to_string(),
            provider: Some("mock".to_string()),
            model: None,
            system_prompt: None,
            env: HashMap::new(),
            error_message: None,
            session_usage: None,
            loop_handle: None,
        },
    );
}

fn agents_remove_request(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/workspaces/test-ws/agents/remove")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn test_agents_add_missing_provider_returns_400() {
    let (router, state) = create_router();
    inject_ws(&state);

    // Body deliberately omits `provider`. serde's "missing field" error surfaces
    // as a 4xx from axum's Json extractor — we accept any 4xx to stay resilient
    // to axum version drift (some versions use 400, others 422).
    let response = router
        .oneshot(agents_add_request(serde_json::json!({
            "handler": "bot",
            "display_name": "Bot",
        })))
        .await
        .unwrap();

    assert!(
        response.status().is_client_error(),
        "expected 4xx for missing provider, got {}",
        response.status()
    );
}

#[tokio::test]
async fn test_agents_add_unsupported_provider_returns_400() {
    let (router, state) = create_router();
    inject_ws(&state);

    let response = router
        .oneshot(agents_add_request(serde_json::json!({
            "handler": "bot",
            "display_name": "Bot",
            "provider": "unknown_xyz",
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_to_json(response).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    let error = body["error"].as_str().unwrap_or("");
    assert!(
        error.contains("unsupported provider"),
        "error should mention 'unsupported provider', got: {error}"
    );
    assert!(
        error.contains("unknown_xyz"),
        "error should echo the rejected provider name, got: {error}"
    );
}

#[tokio::test]
async fn test_agents_remove_soft_delete_keeps_agent_directory() {
    let tmp = tempfile::tempdir().expect("workspace tempdir");
    let agent_dir = tmp.path().join("soft-bot");
    std::fs::create_dir_all(agent_dir.join(".gitim/run")).expect("agent dir");
    let (router, state) = create_router();
    inject_ws_at(&state, tmp.path().to_path_buf());
    insert_agent(&state, "soft-bot", &agent_dir);

    let response = router
        .oneshot(agents_remove_request(serde_json::json!({
            "id": "soft-bot",
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(true));
    assert!(
        agent_dir.exists(),
        "soft delete should leave the local agent directory"
    );
    let s = state.lock().unwrap();
    assert!(
        !s.workspaces["test-ws"].agents.contains_key("soft-bot"),
        "soft delete should remove the agent from runtime state"
    );
}

#[tokio::test]
async fn test_agents_remove_hard_delete_removes_agent_directory() {
    let tmp = tempfile::tempdir().expect("workspace tempdir");
    let agent_dir = tmp.path().join("hard-bot");
    std::fs::create_dir_all(agent_dir.join(".gitim/run")).expect("agent dir");
    std::fs::write(agent_dir.join("state.txt"), "local state").expect("agent file");
    let (router, state) = create_router();
    inject_ws_at(&state, tmp.path().to_path_buf());
    insert_agent(&state, "hard-bot", &agent_dir);

    let response = router
        .oneshot(agents_remove_request(serde_json::json!({
            "id": "hard-bot",
            "hard_delete": true,
        })))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    assert_eq!(body["ok"], serde_json::Value::Bool(true));
    assert!(
        !agent_dir.exists(),
        "hard delete should remove the local agent directory"
    );
    let s = state.lock().unwrap();
    assert!(
        !s.workspaces["test-ws"].agents.contains_key("hard-bot"),
        "hard delete should remove the agent from runtime state"
    );
}

// -- Archive / unarchive route dispatch --
//
// These tests don't spin up a daemon — the router hits `human_client()` /
// `human_handler()` with an empty state and short-circuits with a structured
// "human daemon not initialized" JSON error. The value is proving the route
// is wired and reachable: a 404 would show a missing or misordered route,
// and a 5xx would show a panic / handler signature mismatch. Deeper behaviour
// (success path, permission checks) belongs in E2E where a workspace exists.

async fn assert_route_reachable(router: axum::Router, req: Request<Body>) {
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "archive routes return 200 with JSON body even on uninitialised state"
    );
    let body = body_to_json(response).await;
    assert!(
        body.get("ok").is_some(),
        "response should be a well-formed gitim envelope, got: {body}"
    );
}

#[tokio::test]
async fn test_card_archive_route_reachable() {
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/cards/general/abc123/archive")
        .method("POST")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}

#[tokio::test]
async fn test_card_unarchive_route_reachable() {
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/cards/general/abc123/unarchive")
        .method("POST")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}

#[tokio::test]
async fn test_list_archived_cards_route_reachable() {
    // No query param.
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/cards/archived")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}

#[tokio::test]
async fn test_list_archived_cards_with_channel_query_route_reachable() {
    // With `?channel=foo` — verifies the Query extractor accepts the optional
    // filter and the route matches `archived` rather than trying to fall
    // through to `/im/cards/{channel}/{card_id}`.
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/cards/archived?channel=general")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}

#[tokio::test]
async fn test_channel_archive_route_reachable() {
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/channels/general/archive")
        .method("POST")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}

#[tokio::test]
async fn test_channel_unarchive_route_reachable() {
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/channels/general/unarchive")
        .method("POST")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}

#[tokio::test]
async fn test_list_archived_channels_route_reachable() {
    let (router, state) = create_router();
    inject_ws(&state);
    let req = Request::builder()
        .uri("/workspaces/test-ws/im/channels/archived")
        .body(Body::empty())
        .unwrap();
    assert_route_reachable(router, req).await;
}
