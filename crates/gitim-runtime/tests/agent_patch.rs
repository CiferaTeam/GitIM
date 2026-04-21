//! Integration tests for PATCH /workspaces/{slug}/agents/{id}
//!
//! Follows the `tests/http_workspaces.rs` pattern: tower::ServiceExt::oneshot
//! + create_router + direct WorkspaceContext injection.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;

async fn send(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn inject_workspace(state: &SharedRuntimeState, slug_str: &str) {
    use std::path::PathBuf;
    let mut ctx = WorkspaceContext::new(
        slug_str.to_string(),
        slug_str.to_string(),
        PathBuf::from("/tmp/test-ws"),
    );
    ctx.git_config = Some(WorkspaceConfig {
        workspace: "/tmp/test-ws".to_string(),
        created_at: "2026-04-21T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    });
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug_str.to_string(), ctx);
}

// -- 1. PATCH on nonexistent agent returns 404 --------------------------------

#[tokio::test]
async fn patch_nonexistent_agent_returns_404() {
    let (router, state) = create_router();
    inject_workspace(&state, "ws1");

    let (status, body) = send(
        router,
        "PATCH",
        "/workspaces/ws1/agents/nonexistent",
        Some(json!({ "system_prompt": "hi" })),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], json!(false));
}
