//! Integration tests for `preflight_claude_with` — covers the four error
//! branches against controlled fake binaries, plus an ignored end-to-end
//! test against a real Claude CLI login.

use std::path::PathBuf;
use std::time::Duration;

use gitim_runtime::preflight::{preflight_claude, preflight_claude_with, ErrorKind};

fn fixture(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push(name);
    path
}

/// Resolve `/bin/false` → `/usr/bin/false` fallback for macOS, where
/// `/bin/false` doesn't exist but `/usr/bin/false` does.
fn resolve_stdbin(name: &str) -> &'static str {
    let a = format!("/bin/{name}");
    if std::path::Path::new(&a).is_file() {
        Box::leak(a.into_boxed_str())
    } else {
        Box::leak(format!("/usr/bin/{name}").into_boxed_str())
    }
}

#[tokio::test]
async fn test_preflight_claude_not_installed() {
    let result = preflight_claude_with(
        "/usr/bin/definitely-not-claude-xyz",
        Duration::from_secs(5),
    )
    .await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "claude");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_claude_exit_nonzero() {
    // `false` exits 1 immediately — should land in Other (either via
    // non-zero exit or parse failure, both are Other).
    let result =
        preflight_claude_with(resolve_stdbin("false"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "claude");
}

#[tokio::test]
async fn test_preflight_claude_empty_output() {
    // `true` exits 0 with empty stdout — should land in Other via
    // parse/empty branch.
    let result =
        preflight_claude_with(resolve_stdbin("true"), Duration::from_secs(5)).await;

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

    let result = preflight_claude_with(
        script.to_str().unwrap(),
        Duration::from_millis(500),
    )
    .await;

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
