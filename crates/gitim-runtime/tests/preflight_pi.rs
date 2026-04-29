//! Integration tests for `preflight_pi_with` — covers the four error
//! branches against controlled fake binaries, plus a real end-to-end test
//! against the live pi CLI.

use std::path::PathBuf;
use std::time::Duration;

use gitim_runtime::preflight::{preflight_pi, preflight_pi_with, ErrorKind};

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
async fn test_preflight_pi_not_installed() {
    let result = preflight_pi_with("/usr/bin/definitely-not-pi-xyz", Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "pi");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_pi_exit_nonzero() {
    let result = preflight_pi_with(&resolve_stdbin("false"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "pi");
}

#[tokio::test]
async fn test_preflight_pi_empty_output() {
    let result = preflight_pi_with(&resolve_stdbin("true"), Duration::from_secs(5)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "pi");
}

#[tokio::test]
async fn test_preflight_pi_timeout() {
    let script = fixture("sleep-pi.sh");
    assert!(
        script.is_file(),
        "fixture missing: {script:?} — did you chmod +x?"
    );

    let result = preflight_pi_with(script.to_str().unwrap(), Duration::from_millis(500)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    assert_eq!(result.provider, "pi");
}

#[tokio::test]
async fn test_preflight_pi_uses_text_rpc_field() {
    let script = fixture("pi-rpc-echo.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let result = preflight_pi_with(script.to_str().unwrap(), Duration::from_secs(5)).await;

    assert!(result.available, "expected available, got {result:?}");
    assert_eq!(result.provider, "pi");
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(preview.contains("GITIM_OK"), "preview: {preview}");
}

#[tokio::test]
#[ignore = "requires real pi CLI; run manually with --ignored"]
async fn test_preflight_pi_real_hello() {
    let result = preflight_pi().await;

    assert!(
        result.available,
        "expected pi CLI to be available, got {result:?}"
    );
    assert_eq!(result.provider, "pi");
    assert!(result.duration_ms > 0);
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("GITIM_OK"),
        "expected GITIM_OK in preview, got: {preview}"
    );
}
