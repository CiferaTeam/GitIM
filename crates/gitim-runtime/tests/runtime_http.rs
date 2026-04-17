//! In-process HTTP tests for the runtime's `/preflight/{provider}` route.
//!
//! Uses `tower::ServiceExt::oneshot` to dispatch a single request through the
//! real axum router — no TCP listener, no spawned server, no port races. The
//! provider CLIs (`claude`, `codex`) are invoked by the handler through
//! `preflight_claude()` / `preflight_codex()`; when they're absent we just
//! get back a `PreflightResult` with `available: false`, which is still the
//! shape we want to assert here.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use gitim_runtime::http::create_router;

async fn body_to_json(resp: axum::response::Response) -> serde_json::Value {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("response body is JSON")
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
async fn test_preflight_claude_returns_result_shape() {
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
    // Don't assert `available` — CI may or may not have a logged-in Claude CLI.
    // The stable contract is: this returns a PreflightResult whose provider is "claude".
    assert_eq!(body["provider"], serde_json::Value::String("claude".into()));
    assert!(body.get("duration_ms").is_some(), "duration_ms missing: {body}");
}

#[tokio::test]
async fn test_preflight_codex_returns_result_shape() {
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
}
