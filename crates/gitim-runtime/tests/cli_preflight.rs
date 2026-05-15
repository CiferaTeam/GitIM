//! Integration tests for `cli::cmd_preflight::run`.
//!
//! Mirrors the spawn-real-router pattern from `cli_workspaces` / `cli_status`:
//! the runtime exposes `GET /preflight/{provider}` and we drive it through a
//! `cli::Client` pointed at a loopback `axum::serve`. Unit-level URL building
//! is covered in-module; here we cover the end-to-end wiring and a couple of
//! contract guarantees (unknown provider → 4xx → exit 2, hermes-only flag
//! gating, success → 0).
//!
//! Subprocess-level argv assertions (`gitim-runtime preflight claude` →
//! stdout match) are deferred to T14's binary-level e2e suite, same as the
//! sibling cli_* tests.

use std::net::SocketAddr;

use gitim_runtime::cli::{cmd_preflight, CliError, Client};
use gitim_runtime::http::create_router;

/// Spin up the runtime router on an ephemeral loopback port. We don't need to
/// touch state for preflight tests — the endpoint is stateless and routes
/// directly into `crate::preflight::preflight_*` functions.
async fn spawn_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let (router, _state) = create_router();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, handle)
}

fn client_for(addr: SocketAddr) -> Client {
    Client::new(format!("http://{addr}"))
}

#[tokio::test]
async fn test_preflight_claude_returns_status() {
    // `/preflight/claude` shells out to the claude CLI's --version and a real
    // hello probe. Whether the binary exists or not on the test machine, the
    // handler returns a structured `PreflightResult` with HTTP 200 — that's
    // the runtime's "preflight is informational, not fail-fast" contract.
    // Either outcome should map to exit 0; we don't whitelist outcomes here.
    let (addr, server) = spawn_server().await;
    let client = client_for(addr);

    let exit_code = cmd_preflight::run(&client, "claude".to_string(), None, None)
        .await
        .expect("preflight claude must return Ok regardless of binary presence");
    assert_eq!(exit_code, 0);

    // Sanity-check the same endpoint to assert on shape — the handler would
    // have printed this same JSON to stdout. PreflightResult guarantees
    // `provider` is set; other fields may be null on a tool-not-found host.
    let body = client
        .get("/preflight/claude")
        .await
        .expect("preflight endpoint responds");
    assert_eq!(
        body.get("provider").and_then(|v| v.as_str()),
        Some("claude"),
        "provider field must always echo back the requested provider",
    );
    assert!(
        body.get("available").is_some(),
        "available field must be present (true or false)",
    );

    server.abort();
}

#[tokio::test]
async fn test_preflight_unknown_provider_errors() {
    // Server returns 400 + `{"ok": false, "error": "unknown provider"}` —
    // crucially WITHOUT an `error_code` field. With body-first
    // classification on a 4xx, the CLI now falls through to
    // `HttpStatus(400, _)` (4xx without `error_code` → status decides),
    // which the exit-code mapper still classifies as permanent (exit 2).
    // The contract that matters is the exit code, not the variant; the
    // earlier sentinel-synthesis branch only fires for 2xx now to preserve
    // 5xx-transient semantics (see http.rs `process_response_inner`).
    let (addr, server) = spawn_server().await;
    let client = client_for(addr);

    let result = cmd_preflight::run(&client, "invalid-provider".to_string(), None, None).await;
    let err = result.expect_err("unknown provider must surface as CliError");
    match &err {
        CliError::HttpStatus(status, body) => {
            assert_eq!(*status, 400, "unknown provider must be 400");
            assert!(
                body.contains("unknown provider"),
                "body excerpt must include server's error text: {body}",
            );
        }
        other => panic!("expected HttpStatus(400, _), got: {other:?}"),
    }

    // Exit-code mapping is unit-tested in `cli::exit_code::from_cli_error`
    // but a sanity-check here pins the integration: 4xx → exit 2 (permanent).
    assert_eq!(
        gitim_runtime::cli::from_cli_error(&err),
        2,
        "unknown provider must map to exit code 2 (permanent)",
    );

    server.abort();
}

#[tokio::test]
async fn test_preflight_llm_params_without_hermes_errors() {
    // Client-side validation gate: --llm-provider / --llm-model are
    // hermes-only. Supplying them with provider=claude must fail BEFORE the
    // HTTP round-trip — exit 1 (CLI/network), not exit 2 (server-rejected).
    // No server needed; this is purely CLI-side.
    let client = Client::new("http://127.0.0.1:1".to_string());

    // Test both flags, alone and together. Each must reject independently.
    for (llm_provider, llm_model) in [
        (Some("anthropic".to_string()), None),
        (None, Some("claude-opus-4-7".to_string())),
        (
            Some("anthropic".to_string()),
            Some("claude-opus-4-7".to_string()),
        ),
    ] {
        let result = cmd_preflight::run(
            &client,
            "claude".to_string(),
            llm_provider.clone(),
            llm_model.clone(),
        )
        .await;
        let err = result.expect_err("validation gate must fire");
        match &err {
            CliError::InvalidConfig(msg) => {
                assert!(
                    msg.contains("hermes"),
                    "error must mention hermes for clarity: {msg}",
                );
            }
            other => {
                panic!("expected InvalidConfig for provider=claude with llm flags, got: {other:?}",)
            }
        }
        // Validation failures are CLI-side → exit 1, not server-side → exit 2.
        assert_eq!(
            gitim_runtime::cli::from_cli_error(&err),
            1,
            "validation failures must map to exit 1 (CLI/network)",
        );
    }
}

#[tokio::test]
async fn test_preflight_hermes_with_llm_params_routes_through() {
    // Hermes path with both flags supplied. The server's hermes branch
    // accepts the query params and forwards them to `preflight_hermes_with`.
    // The test machine almost certainly doesn't have a configured hermes
    // profile, so the preflight result will be `available:false` — but the
    // handler still returns 200 (it's informational), which the CLI maps to
    // exit 0. That's the only thing the integration test is enforcing here.
    //
    // URL building is unit-tested in cmd_preflight::tests; this is the
    // wire-format integration check that the query params actually reach
    // the server through reqwest.
    let (addr, server) = spawn_server().await;
    let client = client_for(addr);

    let exit_code = cmd_preflight::run(
        &client,
        "hermes".to_string(),
        Some("custom:test".to_string()),
        Some("test-model".to_string()),
    )
    .await
    .expect("hermes preflight with llm params returns Ok regardless of profile state");
    assert_eq!(exit_code, 0);

    server.abort();
}

#[tokio::test]
async fn test_preflight_runtime_not_running() {
    // Connection-refused parity with the sibling cli_* tests: ephemeral
    // bind + immediate drop, then attempt to call. Tiny race window where
    // another test could grab the port; same caveat as cli_status, fine in
    // practice today.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let client = Client::new(format!("http://{addr}"));
    let result = cmd_preflight::run(&client, "claude".to_string(), None, None).await;
    let err = result.expect_err("preflight against dead port must error");
    assert!(
        matches!(err, CliError::Transport(_)),
        "expected Transport error, got: {err:?}",
    );
}
