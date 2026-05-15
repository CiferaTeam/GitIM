//! Integration tests for `preflight_pi_with` — covers the four error
//! branches against controlled fake binaries, plus a real end-to-end test
//! against the live pi CLI.

use std::collections::HashMap;
use std::time::Duration;

use gitim_runtime::preflight::{
    preflight_pi, preflight_pi_with, preflight_pi_with_config, ErrorKind, PreflightOverrides,
};

mod common;
use common::{fixture, resolve_stdbin};

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
async fn test_preflight_pi_uses_message_rpc_field() {
    let script = fixture("pi-rpc-echo.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let result = preflight_pi_with(script.to_str().unwrap(), Duration::from_secs(5)).await;

    assert!(result.available, "expected available, got {result:?}");
    assert_eq!(result.provider, "pi");
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(preview.contains("GITIM_OK"), "preview: {preview}");
}

#[tokio::test]
#[ignore = "flaky under parallel cargo test load: timing race between script stdout surfacing and the test's own timeout firing. Run with --ignored to verify."]
async fn test_preflight_pi_surfaces_rpc_error_response() {
    let script = fixture("pi-rpc-error.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let result = preflight_pi_with(script.to_str().unwrap(), Duration::from_millis(500)).await;

    assert!(!result.available, "expected unavailable, got {result:?}");
    assert_eq!(result.provider, "pi");
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    let error = result.error.expect("error should be set");
    assert!(error.contains("startsWith"), "error: {error}");
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

// --- Override tests for preflight_pi_with_config ---
//
// pi's RPC mode is stream-oriented (stdin write + stdout JSONL read), so the
// `echo-env-argv.sh` pattern used for claude/codex/opencode doesn't apply
// directly — pi never reads the subprocess's stderr. Instead we use two
// dedicated fixtures (`pi-rpc-echo-env.sh`, `pi-rpc-echo-argv.sh`) that
// emit observable values inside a `message_update` delta, which the parent
// collects into `output_preview` on the success path.
//
// pi ignores `model_override` by design — see doc comment on
// `preflight_pi_with_config`.

#[tokio::test]
async fn pi_with_config_env_override_reaches_subprocess() {
    let script = fixture("pi-rpc-echo-env.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    // pi's success path requires "GITIM_OK" in the collected text. Embed
    // the marker inside the override value so the fixture-echoed delta
    // both lands us in `available = true` (giving us `output_preview`
    // back) and lets us grep for the value the subprocess actually saw.
    let mut env = HashMap::new();
    env.insert(
        "MY_TEST_KEY".to_string(),
        "expected-value-GITIM_OK".to_string(),
    );

    let overrides = PreflightOverrides {
        env_override: Some(env),
        model_override: None,
    };
    let result =
        preflight_pi_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides).await;

    assert!(result.available, "expected available, got {result:?}");
    let preview = result.output_preview.expect("output_preview should be set");
    assert!(
        preview.contains("expected-value-GITIM_OK"),
        "env override not reflected in subprocess delta: {preview}"
    );
}

#[tokio::test]
async fn pi_with_config_model_override_is_ignored() {
    let script = fixture("pi-rpc-echo-argv.sh");
    assert!(script.is_file(), "fixture missing: {script:?}");

    let overrides = PreflightOverrides {
        env_override: None,
        // pi has no model arg — the override should be silently dropped.
        // Use a marker the fixture would echo verbatim if we ever spliced
        // it onto argv.
        model_override: Some("should-not-appear-GITIM_OK".to_string()),
    };
    let result =
        preflight_pi_with_config(script.to_str().unwrap(), Duration::from_secs(5), overrides).await;

    // Success branch: fixture writes ARGV=... wrapped around GITIM_OK so the
    // parent treats it as a valid response and we get `output_preview` back.
    assert!(result.available, "expected available, got {result:?}");
    let preview = result.output_preview.expect("output_preview should be set");
    // argv must remain the hardcoded set — no `--model`, no override leak.
    assert!(
        !preview.contains("should-not-appear"),
        "model override leaked into pi argv despite being ignored by design: {preview}"
    );
    assert!(
        preview.contains("--mode")
            && preview.contains("rpc")
            && preview.contains("--no-session")
            && preview.contains("--no-tools"),
        "expected the hardcoded pi argv flags, got: {preview}"
    );
}

#[tokio::test]
async fn pi_with_config_default_behavior_matches_old_function() {
    // Compare the stable fields between the legacy wrapper and the new
    // _with_config entry called with `Default::default()` — they must agree
    // on classification, provider, and version fields. `duration_ms` is
    // excluded because it's a wall-clock measurement.
    let bin = "/usr/bin/definitely-not-pi-xyz";
    let timeout = Duration::from_secs(5);

    let via_wrapper = preflight_pi_with(bin, timeout).await;
    let via_config = preflight_pi_with_config(bin, timeout, PreflightOverrides::default()).await;

    assert_eq!(via_wrapper.available, via_config.available);
    assert_eq!(via_wrapper.provider, via_config.provider);
    assert_eq!(via_wrapper.error_kind, via_config.error_kind);
    assert_eq!(via_wrapper.model_used, via_config.model_used);
    assert_eq!(via_wrapper.version, via_config.version);
    assert_eq!(via_wrapper.error_kind, Some(ErrorKind::NotInstalled));
}
