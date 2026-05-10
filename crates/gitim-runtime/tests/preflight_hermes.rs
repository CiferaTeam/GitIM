//! Integration tests for `preflight_hermes_with`.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use gitim_runtime::preflight::{preflight_hermes, preflight_hermes_with, ErrorKind};
use tempfile::TempDir;

mod common;
use common::{fixture, resolve_stdbin};

#[tokio::test]
async fn test_preflight_hermes_not_installed() {
    let result = preflight_hermes_with(
        "/usr/bin/definitely-not-hermes-xyz",
        Duration::from_secs(5),
        None,
        None,
        None,
    )
    .await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::NotInstalled));
    assert_eq!(result.provider, "hermes");
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_preflight_hermes_exit_nonzero() {
    // `false` exits 1 immediately — spawn succeeds but process fails.
    let result = preflight_hermes_with(
        &resolve_stdbin("false"),
        Duration::from_secs(5),
        None,
        None,
        None,
    )
    .await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(result.provider, "hermes");
}

#[tokio::test]
async fn test_preflight_hermes_empty_output() {
    // `true` exits 0 but writes nothing to stdout — ACP stream ends immediately.
    let result = preflight_hermes_with(
        &resolve_stdbin("true"),
        Duration::from_secs(5),
        None,
        None,
        None,
    )
    .await;

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

    let result = preflight_hermes_with(
        script.to_str().unwrap(),
        Duration::from_millis(300),
        None,
        None,
        None,
    )
    .await;

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

// ─── New tests for llm_provider / llm_model params ───────────────────────────

/// Test 1: when llm_provider + llm_model are both Some, the fake binary's argv
/// must contain "--provider" and "--model" flags.
///
/// Strategy: write a shell script that appends its argv to a capture file and
/// echoes "GITIM_OK" to stdout (so the chat path sees a valid response), then
/// assert the capture file contains the expected flags.
#[tokio::test]
async fn preflight_hermes_with_llm_overrides_passes_args() {
    let tmp = TempDir::new().unwrap();
    let script_path = tmp.path().join("fake_hermes.sh");
    let capture_file = tmp.path().join("argv.txt");

    // Script: append $@ to capture file, print GITIM_OK (so chat path succeeds), exit 0.
    let script_content = format!(
        "#!/bin/sh\necho \"$@\" >> \"{capture}\"\necho 'GITIM_OK'\nexit 0\n",
        capture = capture_file.display(),
    );
    std::fs::write(&script_path, &script_content).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let _result = preflight_hermes_with(
        script_path.to_str().unwrap(),
        Duration::from_secs(5),
        None,
        Some("minimax-cn"),
        Some("MiniMax-M2.7-highspeed"),
    )
    .await;

    // The fake binary should have been called — verify the capture file exists
    // and contains the expected flags.
    assert!(
        capture_file.exists(),
        "capture file not written — fake binary was not called"
    );
    let captured_argv = std::fs::read_to_string(&capture_file).unwrap();
    assert!(
        captured_argv.contains("--provider"),
        "expected --provider in argv, got: {captured_argv}"
    );
    assert!(
        captured_argv.contains("minimax-cn"),
        "expected minimax-cn in argv, got: {captured_argv}"
    );
    assert!(
        captured_argv.contains("--model"),
        "expected --model in argv, got: {captured_argv}"
    );
    assert!(
        captured_argv.contains("MiniMax-M2.7-highspeed"),
        "expected MiniMax-M2.7-highspeed in argv, got: {captured_argv}"
    );
}

/// Test 2: when llm_provider and llm_model are both None (no overrides), the
/// fake binary's argv must NOT contain "--provider".
///
/// The None path uses the ACP protocol, so argv will be just "acp".
#[tokio::test]
async fn preflight_hermes_with_no_llm_overrides_omits_args() {
    let tmp = TempDir::new().unwrap();
    let script_path = tmp.path().join("fake_hermes.sh");
    let capture_file = tmp.path().join("argv.txt");

    // Script: append $@ to capture file and exit 0. Output doesn't matter for
    // this test — we only inspect whether --provider appears in argv.
    let script_content = format!(
        "#!/bin/sh\necho \"$@\" >> \"{capture}\"\nexit 0\n",
        capture = capture_file.display(),
    );
    std::fs::write(&script_path, &script_content).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let _result = preflight_hermes_with(
        script_path.to_str().unwrap(),
        Duration::from_secs(5),
        None,
        None,
        None,
    )
    .await;

    // The binary may have been called once (hermes --version) or more times.
    // What matters: "--provider" must not appear in any captured argv line.
    if capture_file.exists() {
        let captured_argv = std::fs::read_to_string(&capture_file).unwrap();
        assert!(
            !captured_argv.contains("--provider"),
            "expected --provider to be absent when llm params are None, got: {captured_argv}"
        );
    }
    // If capture file doesn't exist, the script was never called with args we
    // care about (e.g. --version failed before reaching acp) — that's still OK
    // for this assertion.
}

/// Test 3 (ignored): real hermes + minimax-cn key → preflight succeeds.
///
/// Requires hermes CLI in PATH with minimax-cn configured in the default profile.
/// Run manually:
/// ```bash
/// cargo test -p gitim-runtime --test preflight_hermes -- \
///   --ignored preflight_hermes_with_real_minimax_succeeds
/// ```
#[tokio::test]
#[ignore = "requires real hermes CLI with minimax-cn key configured; run manually with --ignored"]
async fn preflight_hermes_with_real_minimax_succeeds() {
    let result = preflight_hermes_with(
        "hermes",
        Duration::from_secs(60),
        None,
        Some("minimax-cn"),
        Some("MiniMax-M2.7-highspeed"),
    )
    .await;

    assert!(
        result.available,
        "expected hermes chat minimax-cn preflight to succeed, got {result:?}"
    );
    assert_eq!(result.provider, "hermes");
    assert!(result.duration_ms > 0);
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("GITIM_OK"),
        "expected GITIM_OK in preview, got: {preview}"
    );
}
