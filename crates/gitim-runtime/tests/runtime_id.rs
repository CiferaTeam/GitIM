#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for runtime_id end-to-end:
//! ensure_runtime_id → RuntimeState → /health response.
//!
//! Unlike the unit tests in user_config.rs (which exercise only the
//! file-IO layer) and http.rs (which exercises only the handler with a
//! pre-injected ID), these tests cover the wiring that bin/runtime.rs
//! does at startup.
//!
//! Pattern mirrors tests/http_workspaces.rs: oneshot through the real
//! router, no TCP listener.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gitim_runtime::http::create_router;
use gitim_runtime::user_config;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

async fn fetch_health(router: axum::Router) -> Value {
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_returns_runtime_id_after_ensure() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime.json");

    // 模拟 run_shell 启动序列:ensure → 注入 state
    let id = user_config::ensure_runtime_id_at(&path);
    let (router, state) = create_router();
    state.lock().unwrap().runtime_id = id.clone();

    let json = fetch_health(router).await;
    assert_eq!(
        json.get("runtime_id").and_then(|v| v.as_str()),
        Some(id.as_str())
    );
}

#[tokio::test]
async fn restart_preserves_runtime_id() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime.json");

    // 第一次"启动"
    let first_id = user_config::ensure_runtime_id_at(&path);
    let (router1, state1) = create_router();
    state1.lock().unwrap().runtime_id = first_id.clone();
    let json1 = fetch_health(router1).await;
    let observed1 = json1
        .get("runtime_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    // 模拟重启:state 全部丢失,只有磁盘上的 runtime.json 留下
    drop(state1);

    let second_id = user_config::ensure_runtime_id_at(&path);
    let (router2, state2) = create_router();
    state2.lock().unwrap().runtime_id = second_id.clone();
    let json2 = fetch_health(router2).await;
    let observed2 = json2
        .get("runtime_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    assert_eq!(first_id, second_id, "ensure_runtime_id_at should be stable");
    assert_eq!(
        observed1, observed2,
        "/health should return the same ID across restarts"
    );
}
