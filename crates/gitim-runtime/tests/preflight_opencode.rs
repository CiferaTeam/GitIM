//! Integration tests for `preflight_opencode_with` — covers the four error
//! branches against controlled fake binaries, plus an ignored end-to-end
//! test against a real opencode CLI login.

use std::time::Duration;

use gitim_runtime::preflight::{preflight_opencode, preflight_opencode_with, ErrorKind};

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
