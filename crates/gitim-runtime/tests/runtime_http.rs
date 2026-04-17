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
use tower::ServiceExt;

use gitim_runtime::http::create_router;

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
        Self { original, _tmp: tmp }
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
    assert_eq!(body["error"], serde_json::Value::String("unknown provider".into()));
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
    assert!(body.get("duration_ms").is_some(), "duration_ms missing: {body}");
    // With PATH stripped, spawn must fail with NotFound → NotInstalled. This
    // is the same JSON shape the WebUI sees when a user hasn't installed the
    // CLI, so asserting on it gives us the stable contract for that branch.
    assert_eq!(body["available"], serde_json::Value::Bool(false), "body: {body}");
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
    assert!(body.get("duration_ms").is_some(), "duration_ms missing: {body}");
    assert_eq!(body["available"], serde_json::Value::Bool(false), "body: {body}");
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
        .uri("/agents/add")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn test_agents_add_missing_provider_returns_400() {
    let (router, _state) = create_router();

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
    let (router, _state) = create_router();

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
