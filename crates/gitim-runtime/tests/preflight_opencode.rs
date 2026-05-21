#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `preflight_opencode_with` — covers the four error
//! branches against controlled fake binaries, plus an ignored end-to-end
//! test against a real opencode CLI login.

use std::collections::HashMap;
use std::time::Duration;
#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

use gitim_runtime::preflight::{
    preflight_opencode, preflight_opencode_with, preflight_opencode_with_config, ErrorKind,
    PreflightOverrides,
};

mod common;
use common::{fixture, resolve_stdbin};

#[tokio::test]
async fn test_preflight_opencode_not_installed() {
    let result = preflight_opencode_with(
        "/usr/bin/definitely-not-opencode-xyz",
        Duration::from_secs(5),
    )
    .await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "opencode");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_opencode_exit_nonzero() {
    // `false` exits 1 immediately — Other branch (non-zero exit).
    let result = preflight_opencode_with(&resolve_stdbin("false"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "opencode");
}

#[tokio::test]
async fn test_preflight_opencode_empty_output() {
    // `true` exits 0 with empty stdout — Other via "did not contain GITIM_OK".
    let result = preflight_opencode_with(&resolve_stdbin("true"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "opencode");
}

#[tokio::test]
async fn test_preflight_opencode_timeout() {
    let script = fixture("sleep-opencode.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result =
        preflight_opencode_with(script.to_str().unwrap(), Duration::from_millis(500)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    assert_eq!(result.provider, "opencode");
}

#[tokio::test]
#[ignore = "requires real opencode CLI logged in; run manually with --ignored"]
async fn test_preflight_opencode_real_hello() {
    let result = preflight_opencode().await;

    assert!(
        result.available,
        "expected opencode CLI to be available, got {result:?}"
    );
    assert_eq!(result.provider, "opencode");
    assert!(result.duration_ms > 0);
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("GITIM_OK"),
        "expected GITIM_OK in preview, got: {preview}"
    );
}

// --- Override tests for preflight_opencode_with_config ---
//
// Same fake-binary pattern as the claude/codex override tests:
// `echo-env-argv.sh` exits non-zero after echoing argv and `MY_TEST_KEY` to
// stderr, which `preflight_opencode_with_config` captures into `result.error`.
// opencode supports a per-invocation `--model provider/model` override; add
// preflight must verify the same selected model the agent will run with.

#[tokio::test]
async fn opencode_with_config_env_override_reaches_subprocess() {
    let script = fixture("echo-env-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let mut env = HashMap::new();
    env.insert("MY_TEST_KEY".to_string(), "expected-value".to_string());

    let overrides = PreflightOverrides {
        env_override: Some(env),
        model_override: None,
    };
    let result =
        preflight_opencode_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
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
async fn opencode_with_config_model_override_reaches_subprocess() {
    let script = fixture("echo-env-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let overrides = PreflightOverrides {
        env_override: None,
        model_override: Some("openai/gpt-test".to_string()),
    };
    let result =
        preflight_opencode_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides)
            .await;

    assert!(!result.available);
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    let err = result.error.expect("error should be set");
    assert!(
        err.contains("--model") && err.contains("openai/gpt-test"),
        "model override not reflected in opencode argv: {err}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn opencode_with_config_runs_inside_git_worktree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let script = tmp.path().join("require-git-worktree.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "not in git worktree" >&2
  exit 7
}
printf '%s\n' '{"type":"text","part":{"text":"GITIM_OK"}}'
"#,
    )
    .expect("write script");
    let mut perms = fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod script");

    let result = preflight_opencode_with_config(
        script.to_str().unwrap(),
        Duration::from_secs(5),
        PreflightOverrides::default(),
    )
    .await;

    assert!(
        result.available,
        "preflight should run fake opencode inside a git worktree, got {result:?}"
    );
}

#[tokio::test]
async fn opencode_with_config_default_behavior_matches_old_function() {
    // Compare the stable fields between the legacy wrapper and the new
    // _with_config entry called with `Default::default()` — they must agree
    // on classification, provider, and version fields. `duration_ms` is
    // excluded because it's a wall-clock measurement.
    let bin = "/usr/bin/definitely-not-opencode-xyz";
    let timeout = Duration::from_secs(5);

    let via_wrapper = preflight_opencode_with(bin, timeout).await;
    let via_config =
        preflight_opencode_with_config(bin, timeout, PreflightOverrides::default()).await;

    assert_eq!(via_wrapper.available, via_config.available);
    assert_eq!(via_wrapper.provider, via_config.provider);
    assert_eq!(via_wrapper.error_kind, via_config.error_kind);
    assert_eq!(via_wrapper.model_used, via_config.model_used);
    assert_eq!(via_wrapper.version, via_config.version);
    assert_eq!(via_wrapper.error_kind, Some(ErrorKind::NotInstalled));
}
