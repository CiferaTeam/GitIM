//! Integration tests for `preflight_hermes_with`.

use std::path::PathBuf;
use std::time::Duration;

use gitim_runtime::preflight::{preflight_hermes, preflight_hermes_with, ErrorKind};

fn fixture(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push(name);
    path
}

fn resolve_stdbin(name: &str) -> String {
    let a = format!("/bin/{name}");
    if std::path::Path::new(&a).is_file() {
        a
    } else {
        format!("/usr/bin/{name}")
    }
}

#[tokio::test]
async fn test_preflight_hermes_not_installed() {
    let result =
        preflight_hermes_with("/usr/bin/definitely-not-hermes-xyz", Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "hermes");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_hermes_exit_nonzero() {
    // `false` exits 1 immediately — spawn succeeds but process fails.
    let result = preflight_hermes_with(&resolve_stdbin("false"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "hermes");
}

#[tokio::test]
async fn test_preflight_hermes_empty_output() {
    // `true` exits 0 but writes nothing to stdout — ACP stream ends immediately.
    let result = preflight_hermes_with(&resolve_stdbin("true"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.provider, "hermes");
}

#[tokio::test]
async fn test_preflight_hermes_timeout() {
    let script = fixture("sleep-hermes.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result = preflight_hermes_with(script.to_str().unwrap(), Duration::from_millis(300)).await;

    assert!(
        !result.available,
        "expected unavailable on timeout, got {result:?}"
    );
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    assert_eq!(result.provider, "hermes");
}

#[tokio::test]
#[ignore = "requires real hermes CLI with ACP support; run manually with --ignored"]
async fn test_preflight_hermes_real_acp() {
    let result = preflight_hermes().await;

    assert!(
        result.available,
        "expected hermes ACP preflight to succeed, got {result:?}"
    );
    assert_eq!(result.provider, "hermes");
    assert!(result.duration_ms > 0);
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("ACP initialize OK"),
        "expected 'ACP initialize OK' in preview, got: {preview}"
    );
}
