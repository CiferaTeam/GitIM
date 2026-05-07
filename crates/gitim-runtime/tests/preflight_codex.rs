//! Integration tests for `preflight_codex_with` — covers the four error
//! branches against controlled fake binaries, plus an ignored end-to-end
//! test against a real Codex CLI login.

use std::time::Duration;

use gitim_runtime::preflight::{preflight_codex, preflight_codex_with, ErrorKind};

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
