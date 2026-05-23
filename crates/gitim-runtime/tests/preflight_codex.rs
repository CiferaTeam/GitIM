#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `preflight_codex_with` — covers the four error
//! branches against controlled fake binaries, plus an ignored end-to-end
//! test against a real Codex CLI login.

use std::collections::HashMap;
use std::time::Duration;

use gitim_runtime::preflight::{
    preflight_codex, preflight_codex_with, preflight_codex_with_config, ErrorKind,
    PreflightOverrides,
};

mod common;
use common::{fixture, resolve_stdbin};

#[tokio::test]
async fn test_preflight_codex_not_installed() {
    let result =
        preflight_codex_with("/usr/bin/definitely-not-codex-xyz", Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "codex");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_codex_exit_nonzero() {
    // `false` exits 1 immediately — should land in Other (non-zero exit).
    let result = preflight_codex_with(&resolve_stdbin("false"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "codex");
}

#[tokio::test]
async fn test_preflight_codex_empty_output() {
    // `true` exits 0 with empty stdout — no `turn.completed`, so Other.
    let result = preflight_codex_with(&resolve_stdbin("true"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "codex");
}

#[tokio::test]
async fn test_preflight_codex_timeout() {
    // `sleep-claude.sh` is generic (just `sleep 10`) — reuse it for codex too.
    let script = fixture("sleep-claude.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result = preflight_codex_with(script.to_str().unwrap(), Duration::from_millis(500)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    assert_eq!(result.provider, "codex");
}

#[tokio::test]
#[ignore = "requires real Codex CLI logged in; run manually with --ignored"]
async fn test_preflight_codex_real_hello() {
    let result = preflight_codex().await;

    assert!(
        result.available,
        "expected Codex CLI to be available, got {result:?}"
    );
    assert_eq!(result.provider, "codex");
    assert_eq!(result.model_used.as_deref(), Some("gpt-5.4-mini"));
    assert!(result.duration_ms > 0);
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("GITIM_OK"),
        "expected GITIM_OK in preview, got: {preview}"
    );
}

// --- Override tests for preflight_codex_with_config ---
//
// Same fake-binary pattern as the claude override tests: `echo-env-argv.sh`
// exits non-zero after echoing argv and `MY_TEST_KEY` to stderr, which
// `preflight_codex_with_config` captures into `result.error`.

#[tokio::test]
async fn codex_with_config_env_override_reaches_subprocess() {
    let script = fixture("echo-env-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let mut env = HashMap::new();
    env.insert("MY_TEST_KEY".to_string(), "expected-value".to_string());

    let overrides = PreflightOverrides {
        env_override: Some(env),
        model_override: None,
    };
    let result =
        preflight_codex_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
            .await;

    assert!(!result.available);
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    let err = result.error.expect("error should be set");
    assert!(
        err.contains("MY_TEST_KEY=expected-value"),
        "env override not reflected in subprocess stderr: {err}"
    );
}

#[tokio::test]
async fn codex_with_config_model_override_argv() {
    let script = fixture("echo-env-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let overrides = PreflightOverrides {
        env_override: None,
        model_override: Some("test-model-xyz".to_string()),
    };
    let result =
        preflight_codex_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
            .await;

    assert!(!result.available);
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    let err = result.error.expect("error should be set");
    assert!(
        err.contains("test-model-xyz"),
        "model override not passed via --model argv: {err}"
    );
    // The default preflight model must NOT appear when an override is supplied —
    // proves we replaced rather than appended.
    assert!(
        !err.contains("gpt-5.4-mini"),
        "default model leaked into argv despite override: {err}"
    );
}

#[tokio::test]
async fn codex_with_config_default_behavior_matches_old_function() {
    // Compare the stable fields between the legacy wrapper and the new
    // _with_config entry called with `Default::default()` — they must agree
    // on classification, provider, and version fields. `duration_ms` is
    // excluded because it's a wall-clock measurement.
    let bin = "/usr/bin/definitely-not-codex-xyz";
    let timeout = Duration::from_secs(5);

    let via_wrapper = preflight_codex_with(bin, timeout).await;
    let via_config = preflight_codex_with_config(bin, timeout, PreflightOverrides::default()).await;

    assert_eq!(via_wrapper.available, via_config.available);
    assert_eq!(via_wrapper.provider, via_config.provider);
    assert_eq!(via_wrapper.error_kind, via_config.error_kind);
    assert_eq!(via_wrapper.model_used, via_config.model_used);
    assert_eq!(via_wrapper.version, via_config.version);
    assert_eq!(via_wrapper.error_kind, Some(ErrorKind::NotInstalled));
}
