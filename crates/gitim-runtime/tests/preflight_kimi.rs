#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `preflight_kimi_with_config`.
//!
//! Driven by fake `kimi` shell scripts under `tests/fixtures/`:
//! - `mock-kimi-acp.sh` — happy path: acks initialize / session/new /
//!   set_model and replies to `session/prompt` with a text chunk.
//! - `mock-kimi-acp-set-model-fails.sh` — set_model rejection path.
//! - `sleep-kimi.sh` — stalls past any reasonable timeout.
//!
//! Modelled on `preflight_hermes.rs`. The kimi preflight is the most
//! end-to-end of any provider (initialize → new → optional set_model
//! → prompt → text-walk), so the integration test catches regressions
//! that the inline parse-only unit tests can't reach.

use std::time::Duration;

use gitim_runtime::preflight::{preflight_kimi_with_config, ErrorKind, PreflightOverrides};

mod common;
use common::fixture;

#[tokio::test]
async fn test_preflight_kimi_not_installed() {
    let result = preflight_kimi_with_config(
        "/usr/bin/definitely-not-kimi-xyz",
        Duration::from_secs(5),
        PreflightOverrides::default(),
    )
    .await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "kimi");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_kimi_happy_path() {
    let script = fixture("mock-kimi-acp.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result = preflight_kimi_with_config(
        script.to_str().unwrap(),
        Duration::from_secs(5),
        PreflightOverrides::default(),
    )
    .await;

    assert!(
        result.available,
        "expected available on happy path, got {result:?}"
    );
    assert_eq!(result.provider, "kimi");
    assert_eq!(
        result.version.as_deref(),
        Some("0.99.0-mock"),
        "version should round-trip from `kimi --version`'s last token"
    );
    let preview = result
        .output_preview
        .expect("output_preview should be set on success");
    assert!(
        preview.contains("\"text\":\"hi\"") || preview.contains("\"hi\""),
        "expected text chunk in preview, got: {preview}"
    );
}

/// Plan §"preflight_kimi_with_config" / behaviour symmetry with the
/// runtime driver: when set_model errors mid-handshake, preflight must
/// fail (no silent fallback to whatever default kimi picked).
#[tokio::test]
async fn test_preflight_kimi_set_model_failure_fails() {
    let script = fixture("mock-kimi-acp-set-model-fails.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let overrides = PreflightOverrides {
        model_override: Some("bogus-model".to_string()),
        ..PreflightOverrides::default()
    };

    let result =
        preflight_kimi_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
            .await;

    assert!(
        !result.available,
        "expected unavailable when set_model fails, got {result:?}"
    );
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "kimi");
    let err = result
        .error
        .as_deref()
        .expect("error must be set on set_model failure");
    assert!(
        err.contains("model not available"),
        "error must surface the upstream JSON-RPC message, got: {err}"
    );
}

#[tokio::test]
async fn test_preflight_kimi_timeout() {
    let script = fixture("sleep-kimi.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result = preflight_kimi_with_config(
        script.to_str().unwrap(),
        Duration::from_millis(300),
        PreflightOverrides::default(),
    )
    .await;

    assert!(
        !result.available,
        "expected unavailable on timeout, got {result:?}"
    );
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    assert_eq!(result.provider, "kimi");
}

/// Run against a real `kimi` CLI in PATH. Skipped by default —
/// exercises the production happy path when developing locally.
#[tokio::test]
#[ignore = "requires real kimi CLI with ACP support; run manually with --ignored"]
async fn test_preflight_kimi_real_acp() {
    let result = preflight_kimi_with_config(
        "kimi",
        Duration::from_secs(60),
        PreflightOverrides::default(),
    )
    .await;

    assert!(
        result.available,
        "expected real kimi ACP preflight to succeed, got {result:?}"
    );
    assert_eq!(result.provider, "kimi");
    assert!(result.duration_ms > 0);
}
