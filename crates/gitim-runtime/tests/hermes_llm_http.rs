//! Integration tests for `GET /hermes/llm/providers` and
//! `GET /hermes/llm/providers/{id}/models`.
//!
//! Uses `tower::ServiceExt::oneshot` — in-process, no TCP listener, no port
//! races. `HERMES_HOME` is set/unset per-test; `#[serial(hermes_home_env)]`
//! prevents races between tests that mutate the process-global env var.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mockito::Server;
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

// ── test 5 ────────────────────────────────────────────────────────────────────

/// GET /hermes/llm/providers/{id}/models with a completely unknown provider id
/// returns 400 (not 200).
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_models_unknown_provider_400() {
    let (_guard, _path) = HermesHomeGuard::install_empty();

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers/totally-fake/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "unknown provider id must return 400"
    );
    let body = body_to_json(response).await;
    assert!(
        body.get("error").is_some(),
        "400 response must have 'error' key; got: {body}"
    );
}

// ── test 6 ────────────────────────────────────────────────────────────────────

/// GET /hermes/llm/providers/{builtin-id}/models with no API key configured
/// returns 200 + error field (missing api key error).
///
/// Status is 200 — the provider is *known* (builtin), so 400 is wrong.
/// The error comes from `fetch_models` finding no key in `.env`.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_models_builtin_returns_shape() {
    let (_guard, _path) = HermesHomeGuard::install_empty();
    // _path is empty — no .env, so kimi-coding has no API key.

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers/kimi-coding/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "known builtin provider must return 200 even without api key"
    );
    let body = body_to_json(response).await;
    // error field must be present and non-null (missing api key)
    let error = body.get("error").expect("response must have 'error' key");
    assert!(
        !error.is_null(),
        "error must be non-null when api key is missing; got: {body}"
    );
    let err_str = error.as_str().expect("error must be a string");
    assert!(
        err_str.contains("missing api key"),
        "error must mention 'missing api key'; got: {err_str}"
    );
    // models must be present (empty array)
    assert!(
        body.get("models").is_some(),
        "response must have 'models' key; got: {body}"
    );
    assert!(
        body.get("custom_allowed").is_some(),
        "response must have 'custom_allowed' key; got: {body}"
    );
}

// ── test 7 ────────────────────────────────────────────────────────────────────

/// GET /hermes/llm/providers/custom:{name}/models with a custom provider
/// that has a base_url pointing at a mock server returns 200 + populated
/// models list + null error.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_models_custom_provider_returns_shape() {
    let (_guard, path) = HermesHomeGuard::install_empty();

    // Spin up a mock HTTP server that returns an OpenAI-compatible model list.
    let mut mock_server = Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/models")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"id":"custom-model-1"},{"id":"custom-model-2"}]}"#)
        .create_async()
        .await;

    // Write config.yaml with a custom_providers entry pointing at the mock.
    let yaml = format!(
        "custom_providers:\n  - name: myfoo\n    base_url: {url}\n    api_key: test-key-xyz\n",
        url = mock_server.url()
    );
    fs::write(path.join("config.yaml"), &yaml).expect("write config.yaml");

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers/custom:myfoo/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "custom provider must return 200 on success"
    );
    let body = body_to_json(response).await;
    let error = &body["error"];
    assert!(
        error.is_null(),
        "error must be null on successful custom provider fetch; got: {body}"
    );
    let models = body["models"].as_array().expect("models must be an array");
    assert_eq!(
        models.len(),
        2,
        "expected 2 models from mock, got: {body}"
    );
    assert_eq!(models[0]["id"].as_str(), Some("custom-model-1"));
    assert_eq!(models[1]["id"].as_str(), Some("custom-model-2"));
}

// ── test 8 ────────────────────────────────────────────────────────────────────

/// GET /hermes/llm/providers/{id}/models returns HTTP 200 even when the
/// upstream server returns a 500.  The HTTP status is always 200; the upstream
/// error is surfaced in the `error` field.
#[tokio::test]
#[serial(hermes_home_env)]
async fn get_models_status_always_200_even_on_upstream_failure() {
    let (_guard, path) = HermesHomeGuard::install_empty();

    // Upstream mock returns 500.
    let mut mock_server = Server::new_async().await;
    let _mock = mock_server
        .mock("GET", "/models")
        .with_status(500)
        .with_body("Internal Server Error")
        .create_async()
        .await;

    // Use a custom provider so we can point its base_url at the mock.
    let yaml = format!(
        "custom_providers:\n  - name: failprovider\n    base_url: {url}\n    api_key: test-key\n",
        url = mock_server.url()
    );
    fs::write(path.join("config.yaml"), &yaml).expect("write config.yaml");

    let (router, _state) = create_router();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/hermes/llm/providers/custom:failprovider/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "status must always be 200 even when upstream returns 500"
    );
    let body = body_to_json(response).await;
    let error = body.get("error").expect("response must have 'error' key");
    assert!(
        !error.is_null(),
        "error must be non-null when upstream fails; got: {body}"
    );
    let err_str = error.as_str().expect("error must be a string");
    assert!(
        err_str.contains("upstream HTTP 500") || err_str.contains("500"),
        "error must mention upstream failure; got: {err_str}"
    );
}
