//! Integration tests for `preflight_claude_with` — covers the four error
//! branches against controlled fake binaries, plus an ignored end-to-end
//! test against a real Claude CLI login.

use std::collections::HashMap;
use std::time::Duration;

use gitim_runtime::preflight::{
    preflight_claude, preflight_claude_with, preflight_claude_with_config, ErrorKind,
    PreflightOverrides,
};

mod common;
use common::{fixture, resolve_stdbin};

#[tokio::test]
async fn test_preflight_claude_not_installed() {
    let result =
        preflight_claude_with("/usr/bin/definitely-not-claude-xyz", Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "claude");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_claude_exit_nonzero() {
    // `false` exits 1 immediately — should land in Other (either via
    // non-zero exit or parse failure, both are Other).
    let result = preflight_claude_with(&resolve_stdbin("false"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "claude");
}

#[tokio::test]
async fn test_preflight_claude_empty_output() {
    // `true` exits 0 with empty stdout — should land in Other via
    // parse/empty branch.
    let result = preflight_claude_with(&resolve_stdbin("true"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "claude");
}

#[tokio::test]
async fn test_preflight_claude_timeout() {
    let script = fixture("sleep-claude.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result = preflight_claude_with(script.to_str().unwrap(), Duration::from_millis(500)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    assert_eq!(result.provider, "claude");
}

#[tokio::test]
#[ignore = "requires real Claude CLI logged in; run manually with --ignored"]
async fn test_preflight_claude_real_hello() {
    let result = preflight_claude().await;

    assert!(
        result.available,
        "expected Claude CLI to be available, got {result:?}"
    );
    assert_eq!(result.provider, "claude");
    assert_eq!(result.model_used.as_deref(), Some("claude-haiku-4-5"));
    assert!(result.duration_ms > 0);
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("GITIM_OK"),
        "expected GITIM_OK in preview, got: {preview}"
    );
}

// --- Override tests for preflight_claude_with_config ---
//
// `echo-env-argv.sh` exits non-zero after echoing argv and the value of
// `MY_TEST_KEY` to stderr. The non-zero exit lands the call in the `Other`
// branch and surfaces the captured stderr inside `result.error`, which the
// assertions then grep. This is enough to prove the override fields reached
// the spawned subprocess without needing a real Claude CLI.

#[tokio::test]
async fn claude_with_config_env_override_reaches_subprocess() {
    let script = fixture("echo-env-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let mut env = HashMap::new();
    env.insert("MY_TEST_KEY".to_string(), "expected-value".to_string());

    let overrides = PreflightOverrides {
        env_override: Some(env),
        model_override: None,
    };
    let result =
        preflight_claude_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
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
async fn claude_with_config_model_override_argv() {
    let script = fixture("echo-env-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let overrides = PreflightOverrides {
        env_override: None,
        model_override: Some("test-model-xyz".to_string()),
    };
    let result =
        preflight_claude_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
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
        !err.contains("claude-haiku-4-5"),
        "default model leaked into argv despite override: {err}"
    );
}

#[tokio::test]
async fn claude_with_config_default_behavior_matches_old_function() {
    // Use the same "not installed" path the wrapper test exercises and assert
    // both code paths produce structurally equivalent failures. Comparing the
    // full PreflightResult is brittle (duration_ms varies), so we compare the
    // fields that should be stable across the two calls.
    let bin = "/usr/bin/definitely-not-claude-xyz";
    let timeout = Duration::from_secs(5);

    let via_wrapper = preflight_claude_with(bin, timeout).await;
    let via_config =
        preflight_claude_with_config(bin, timeout, PreflightOverrides::default()).await;

    assert_eq!(via_wrapper.available, via_config.available);
    assert_eq!(via_wrapper.provider, via_config.provider);
    assert_eq!(via_wrapper.error_kind, via_config.error_kind);
    assert_eq!(via_wrapper.model_used, via_config.model_used);
    assert_eq!(via_wrapper.version, via_config.version);
    // Both should report not_installed; that's the salient classification.
    assert_eq!(via_wrapper.error_kind, Some(ErrorKind::NotInstalled));
}
