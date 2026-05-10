//! Integration tests for `GET /hermes/llm/providers`.
//!
//! Uses `tower::ServiceExt::oneshot` — in-process, no TCP listener, no port
//! races. `HERMES_HOME` is set/unset per-test; `#[serial(hermes_home_env)]`
//! prevents races between tests that mutate the process-global env var.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serial_test::serial;
use std::fs;
use tower::ServiceExt;

use gitim_runtime::http::create_router;

async fn body_to_json(resp: axum::response::Response) -> serde_json::Value {
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("response body is valid JSON")
}

struct HermesHomeGuard {
    original: Option<std::ffi::OsString>,
    _tmp: tempfile::TempDir,
}

impl HermesHomeGuard {
    /// Point `HERMES_HOME` at a fresh empty tempdir. Returns the dir path so
    /// callers can populate fixture files before the router dispatches.
    fn install_empty() -> (Self, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().to_path_buf();
        let original = std::env::var_os("HERMES_HOME");
        std::env::set_var("HERMES_HOME", &path);
        (Self { original, _tmp: tmp }, path)
    }
}

impl Drop for HermesHomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("HERMES_HOME", val),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

// ── test 1 ────────────────────────────────────────────────────────────────────

/// Empty HERMES_HOME → providers list is empty; status is always 200.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_providers_empty_when_no_hermes_home() {
    let (_guard, _path) = HermesHomeGuard::install_empty();
    // _path is an empty tempdir — no .env, no config.yaml

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    let providers = body["providers"].as_array().expect("providers is an array");
    assert!(
        providers.is_empty(),
        "expected empty providers list for empty hermes home, got: {body}"
    );
}

// ── test 2 ────────────────────────────────────────────────────────────────────

/// `.env` containing `KIMI_API_KEY=foo` → response contains id=kimi-coding.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_providers_lists_env_configured() {
    let (_guard, path) = HermesHomeGuard::install_empty();
    fs::write(path.join(".env"), "KIMI_API_KEY=foo\n").expect("write .env");

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    let providers = body["providers"].as_array().expect("providers is an array");
    let has_kimi = providers
        .iter()
        .any(|p| p["id"].as_str() == Some("kimi-coding"));
    assert!(
        has_kimi,
        "expected kimi-coding in providers when KIMI_API_KEY is set; got: {body}"
    );
}

// ── test 3 ────────────────────────────────────────────────────────────────────

/// `config.yaml` with `custom_providers` → response contains id=`custom:foo`.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_providers_includes_custom() {
    let (_guard, path) = HermesHomeGuard::install_empty();
    fs::write(
        path.join("config.yaml"),
        "custom_providers:\n  - name: foo\n    base_url: https://custom.example.com\n",
    )
    .expect("write config.yaml");

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_to_json(response).await;
    let providers = body["providers"].as_array().expect("providers is an array");
    let has_custom = providers
        .iter()
        .any(|p| p["id"].as_str() == Some("custom:foo"));
    assert!(
        has_custom,
        "expected custom:foo in providers when config.yaml lists it; got: {body}"
    );
}

// ── test 4 ────────────────────────────────────────────────────────────────────

/// Status is always 200 — introspection failures must not produce 5xx.
/// This also verifies the response JSON has the `providers` key.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_providers_status_200() {
    // Point HERMES_HOME at a path that doesn't exist at all — worst-case for
    // the introspect logic. Still must return 200 with `{"providers": []}`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let nonexistent = tmp.path().join("does_not_exist");
    let original = std::env::var_os("HERMES_HOME");
    std::env::set_var("HERMES_HOME", &nonexistent);

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Restore before any assertion that might panic.
    match original {
        Some(val) => std::env::set_var("HERMES_HOME", val),
        None => std::env::remove_var("HERMES_HOME"),
    }

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "must always be 200 even when hermes home doesn't exist"
    );
    let body = body_to_json(response).await;
    assert!(
        body.get("providers").is_some(),
        "response must always have a 'providers' key; got: {body}"
    );
}
