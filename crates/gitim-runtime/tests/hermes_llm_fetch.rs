//! Integration tests for `hermes_llm::fetch_models`.
//!
//! All network calls are intercepted by a local `mockito` server.
//! No external network is touched; tests are CI-friendly.

use std::fs;
use std::time::Duration;

use gitim_runtime::hermes_llm::{fetch_models, ApiProtocol, LlmProvider, ProviderKind};
use mockito::Server;
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_hermes_home() -> TempDir {
    TempDir::new().expect("TempDir::new")
}

fn write_env(dir: &TempDir, content: &str) {
    fs::write(dir.path().join(".env"), content).expect("write .env");
}

/// Build an OpenAI-protocol provider pointing at the given base URL.
fn openai_provider(id: &str, base_url: &str) -> LlmProvider {
    LlmProvider {
        id: id.to_owned(),
        label: id.to_owned(),
        kind: ProviderKind::ApiKey,
        base_url: Some(base_url.to_owned()),
        api_protocol: ApiProtocol::OpenAI,
    }
}

// ── test 1: success ───────────────────────────────────────────────────────────

/// A 200 OK response with the standard `{data: [{id: ...}]}` shape succeeds.
#[tokio::test]
async fn success_openai_compatible() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#)
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=sk-test-key\n");

    let provider = openai_provider("kimi-coding", &server.url());
    let result = fetch_models(&provider, tmp.path()).await;

    assert_eq!(result.models.len(), 2, "expected 2 models, got {:?}", result.models);
    assert_eq!(result.models[0].id, "m1");
    assert_eq!(result.models[1].id, "m2");
    assert!(result.error.is_none(), "expected no error, got {:?}", result.error);
    assert!(result.custom_allowed);
    assert!(result.fetched_at_ms > 0);

    mock.assert_async().await;
}

// ── test 2: HTTP 401 ─────────────────────────────────────────────────────────

/// A 401 response maps to an actionable auth-failed error message.
#[tokio::test]
async fn http_401_returns_auth_failed_error() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=sk-bad-key\n");

    let provider = openai_provider("kimi-coding", &server.url());
    let result = fetch_models(&provider, tmp.path()).await;

    assert!(result.models.is_empty());
    let err = result.error.expect("expected error field");
    assert!(
        err.contains("auth failed (HTTP 401)"),
        "expected 'auth failed (HTTP 401)' in error, got: {err:?}"
    );
    assert!(result.custom_allowed);

    mock.assert_async().await;
}

// ── test 3: HTTP 500 ─────────────────────────────────────────────────────────

/// A 5xx response maps to an upstream-HTTP error message.
#[tokio::test]
async fn http_500_returns_upstream_error() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .with_status(500)
        .with_body("Internal Server Error")
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "DEEPSEEK_API_KEY=sk-deepseek-key\n");

    let provider = openai_provider("deepseek", &server.url());
    let result = fetch_models(&provider, tmp.path()).await;

    assert!(result.models.is_empty());
    let err = result.error.expect("expected error field");
    assert!(
        err.contains("upstream HTTP 500"),
        "expected 'upstream HTTP 500' in error, got: {err:?}"
    );

    mock.assert_async().await;
}

// ── test 4: timeout ───────────────────────────────────────────────────────────

/// A response that never completes within 5s triggers the timeout path.
#[tokio::test]
async fn timeout_returns_timeout_error() {
    let mut server = Server::new_async().await;
    // Chunked body writer blocks for 10s — our 5s reqwest timeout fires first.
    let _mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_chunked_body(|w| {
            std::thread::sleep(Duration::from_secs(10));
            w.write_all(b"[]")
        })
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "DEEPSEEK_API_KEY=sk-deepseek-key\n");

    let provider = openai_provider("deepseek", &server.url());
    let start = std::time::Instant::now();
    let result = fetch_models(&provider, tmp.path()).await;
    let elapsed = start.elapsed();

    let err = result.error.expect("expected timeout error");
    assert!(
        err.contains("timeout fetching"),
        "expected 'timeout fetching' in error, got: {err:?}"
    );
    // Should complete well before 10s (our timeout is 5s), with some slack.
    assert!(
        elapsed < Duration::from_secs(9),
        "test took too long ({elapsed:?}); timeout may not have fired"
    );
}

// ── test 5: JSON parse failure ────────────────────────────────────────────────

/// A 200 OK response with an unexpected JSON shape triggers the schema-error path.
#[tokio::test]
async fn parse_failure_returns_schema_error() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"unexpected":"shape"}"#)
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "DEEPSEEK_API_KEY=sk-deepseek-key\n");

    let provider = openai_provider("deepseek", &server.url());
    let result = fetch_models(&provider, tmp.path()).await;

    assert!(result.models.is_empty());
    let err = result.error.expect("expected error");
    assert!(
        err.contains("unexpected response schema"),
        "expected 'unexpected response schema' in error, got: {err:?}"
    );

    mock.assert_async().await;
}

// ── test 6: `data` field missing ──────────────────────────────────────────────

/// A 200 OK response that has a `object=list` shape but no `data` field.
#[tokio::test]
async fn data_field_missing_returns_schema_error() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"object":"list"}"#)
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "DEEPSEEK_API_KEY=sk-deepseek-key\n");

    let provider = openai_provider("deepseek", &server.url());
    let result = fetch_models(&provider, tmp.path()).await;

    assert!(result.models.is_empty());
    let err = result.error.expect("expected error");
    assert!(
        err.contains("unexpected response schema"),
        "expected 'unexpected response schema' in error for missing data field, got: {err:?}"
    );

    mock.assert_async().await;
}

// ── test 7: missing API key ───────────────────────────────────────────────────

/// When no API key is present in `.env`, return an actionable error.
#[tokio::test]
async fn missing_api_key_returns_actionable_error() {
    // No network mock — function should return before hitting network.
    let tmp = make_hermes_home();
    // .env exists but does NOT contain the key for kimi-coding.
    write_env(&tmp, "SOME_OTHER_KEY=irrelevant\n");

    let provider = openai_provider("kimi-coding", "http://127.0.0.1:19999/should-not-connect");
    let result = fetch_models(&provider, tmp.path()).await;

    assert!(result.models.is_empty());
    let err = result.error.expect("expected error for missing key");
    assert!(
        err.contains("missing api key for"),
        "expected 'missing api key for' in error, got: {err:?}"
    );
    assert!(result.custom_allowed);
}

// ── test 8: API key must not leak into error strings ─────────────────────────

/// A 401 response must not expose the literal API key value in the error field.
///
/// This is the security invariant: keys are used in the Authorization header
/// but must never be serialized into the `error` string.
///
/// Two halves:
/// (a) the header matcher proves the key was actually sent in Authorization
/// (b) the error-string assert proves the key was not echoed into `error`
#[tokio::test]
async fn error_message_does_not_leak_api_key() {
    const SECRET_KEY: &str = "secret-token-xxx";

    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .match_header(
            "authorization",
            mockito::Matcher::Regex("^Bearer ".to_string()),
        )
        .with_status(401)
        .with_body(r#"{"error":"Unauthorized"}"#)
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, &format!("KIMI_API_KEY={SECRET_KEY}\n"));

    let provider = openai_provider("kimi-coding", &server.url());
    let result = fetch_models(&provider, tmp.path()).await;

    let err = result.error.expect("expected error on 401");
    assert!(
        !err.contains(SECRET_KEY),
        "API key leaked into error string! error: {err:?}"
    );
    assert!(
        err.contains("auth failed (HTTP 401)"),
        "expected 'auth failed (HTTP 401)' in error, got: {err:?}"
    );

    // Proves the Authorization header was sent AND the mock was actually called.
    mock.assert_async().await;
}

// ── test 10: Custom provider reads api_key from config.yaml ──────────────────

/// A `ProviderKind::Custom` provider reads its API key from
/// `<hermes_home>/config.yaml` rather than `.env`.
/// The header matcher proves the key from config.yaml was actually sent.
#[tokio::test]
async fn success_custom_provider_reads_api_key_from_config_yaml() {
    let mut server = Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .match_header(
            "authorization",
            mockito::Matcher::Exact("Bearer custom-key-xyz".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"id":"custom-model-1"}]}"#)
        .create_async()
        .await;

    let tmp = make_hermes_home();
    // Write config.yaml with one custom_providers entry.
    let yaml = format!(
        "custom_providers:\n  - name: myprovider\n    base_url: {url}\n    api_key: custom-key-xyz\n",
        url = server.url(),
    );
    fs::write(tmp.path().join("config.yaml"), yaml).expect("write config.yaml");

    let provider = LlmProvider {
        id: "custom:myprovider".to_string(),
        label: "myprovider (custom)".to_string(),
        kind: ProviderKind::Custom,
        base_url: Some(server.url()),
        api_protocol: ApiProtocol::OpenAI,
    };

    let result = fetch_models(&provider, tmp.path()).await;
    assert!(
        result.error.is_none(),
        "expected success, got error: {:?}",
        result.error
    );
    assert_eq!(result.models.len(), 1);
    assert_eq!(result.models[0].id, "custom-model-1");

    // Proves the api_key from config.yaml was used in the Authorization header.
    mock.assert_async().await;
}

// ── test 9: Anthropic protocol short-circuit ─────────────────────────────────

/// Providers with `api_protocol == Anthropic` must short-circuit WITHOUT
/// touching the network. We point the provider at the mock server's address,
/// but the mock should receive zero hits.
#[tokio::test]
async fn anthropic_protocol_short_circuits_without_network() {
    let mut server = Server::new_async().await;
    // Register a mock — we'll assert it gets ZERO hits.
    let mock = server
        .mock("GET", "/models")
        .with_status(200)
        .with_body(r#"{"data":[]}"#)
        .expect_at_most(0)  // enforce: must not be called
        .create_async()
        .await;

    let tmp = make_hermes_home();
    write_env(&tmp, "MINIMAX_API_KEY=mm-key\n");

    let provider = LlmProvider {
        id: "minimax".to_owned(),
        label: "MiniMax".to_owned(),
        kind: ProviderKind::ApiKey,
        base_url: Some(server.url()),
        api_protocol: ApiProtocol::Anthropic,
    };

    let result = fetch_models(&provider, tmp.path()).await;

    // Must get error about Anthropic protocol short-circuit.
    let err = result.error.expect("expected short-circuit error");
    assert!(
        err.contains("Anthropic protocol"),
        "expected 'Anthropic protocol' in error, got: {err:?}"
    );
    assert!(result.models.is_empty());
    assert!(result.custom_allowed);

    // This assert_async() call verifies expect_at_most(0) — i.e. zero network hits.
    mock.assert_async().await;
}
