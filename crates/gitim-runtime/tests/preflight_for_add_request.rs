//! Integration tests for `preflight_for_add_request` and
//! `classify_preflight_error_code`.
//!
//! `preflight_for_add_request` is the single entry the add-agent path calls
//! after `handler_conflict` clears and before `provision_agent` commits any
//! artifacts. These tests exercise each dispatch branch — mock short-circuit,
//! per-provider env/model threading, hermes (chat / default-profile-resolve /
//! missing-LLM / no-LLM-in-profile), unknown providers, and the outer timeout
//! seam.
//!
//! Fake-binary trick: we point the dispatcher's per-provider `*_bin` override
//! at a shell script in a tempdir (or `tests/fixtures/`). The dispatcher
//! treats it as the CLI; the script captures argv/env to a file and emits the
//! same output the real CLI would emit on success. Tests then read the
//! capture file to verify the dispatcher threaded inputs through correctly.
//!
//! `hermes_home` is also threaded through the override seam so we never touch
//! the process-global `HERMES_HOME` env var here — avoiding races with other
//! tests in the binary that also touch hermes config.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use gitim_runtime::preflight::{
    classify_preflight_error_code, preflight_for_add_request,
    preflight_for_add_request_with_overrides, ErrorKind, PreflightDispatchOverrides,
    PreflightResult, ERROR_CODE_PROVISION_PREFLIGHT_FAILED, FAILURE_CODE_HERMES_NO_LLM,
    FAILURE_CODE_MISSING_LLM_PROVIDER, FAILURE_CODE_UNKNOWN_PROVIDER,
};
use tempfile::TempDir;

/// Write a shell script that captures argv + a selection of env vars to a
/// file, then prints `stdout_body` to stdout and exits 0.
///
/// `env_probe_keys` is a list of env-var names to record. Each is emitted as
/// `KEY=value` on its own line (with `<unset>` if the var isn't set).
fn make_capture_script(
    dir: &std::path::Path,
    name: &str,
    capture_file: &std::path::Path,
    stdout_body: &str,
    env_probe_keys: &[&str],
) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut script = String::new();
    script.push_str("#!/bin/sh\n");
    script.push_str(&format!(
        "echo \"ARGV=$*\" >> \"{}\"\n",
        capture_file.display()
    ));
    for k in env_probe_keys {
        script.push_str(&format!(
            "printf '{key}=%s\\n' \"${{{key}:-<unset>}}\" >> \"{capture}\"\n",
            key = k,
            capture = capture_file.display(),
        ));
    }
    // stdout_body may contain quotes; use a heredoc-style printf to be safe.
    script.push_str(&format!("printf '%s\\n' '{}'\n", stdout_body));
    script.push_str("exit 0\n");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

// ─── mock provider ──────────────────────────────────────────────────────────

#[tokio::test]
async fn mock_provider_short_circuits_success() {
    // No fake binary needed — the dispatcher must not spawn anything for
    // provider="mock". If it did, the call would fail (no `mock` on PATH).
    let result = preflight_for_add_request("mock", None, None, None, None).await;

    assert!(result.available, "mock should short-circuit success, got {result:?}");
    assert_eq!(result.provider, "mock");
    assert!(result.failure_code.is_none());
    assert!(result.error.is_none());
    // Duration should be sub-millisecond — proves we didn't shell out.
    assert!(
        result.duration_ms < 50,
        "mock branch is too slow ({} ms); did it shell out?",
        result.duration_ms
    );
}

#[tokio::test]
async fn mock_provider_passes_classify_to_provision_preflight_failed_default() {
    // mock returns success, so classify should never be called on it in real
    // use. But verify the contract: a successful result without failure_code
    // also falls through to the generic code (since classify is only consulted
    // on failure).
    let result = preflight_for_add_request("mock", None, None, None, None).await;
    assert_eq!(
        classify_preflight_error_code(&result),
        ERROR_CODE_PROVISION_PREFLIGHT_FAILED
    );
}

// ─── unknown provider ───────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_provider_returns_unknown_failure_code() {
    let result =
        preflight_for_add_request("madeup-provider-xyz", None, None, None, None).await;

    assert!(!result.available);
    assert_eq!(result.provider, "madeup-provider-xyz");
    assert_eq!(
        result.failure_code.as_deref(),
        Some(FAILURE_CODE_UNKNOWN_PROVIDER)
    );
    assert_eq!(result.error_kind, Some(ErrorKind::Other));
    assert_eq!(
        classify_preflight_error_code(&result),
        FAILURE_CODE_UNKNOWN_PROVIDER
    );
}

// ─── claude dispatch ────────────────────────────────────────────────────────

#[tokio::test]
async fn claude_dispatch_passes_env_and_model_overrides() {
    let tmp = TempDir::new().unwrap();
    let capture = tmp.path().join("argv.txt");
    // Claude's `_with_config` parses stdout as JSON looking for the result entry.
    // Emit a minimal valid JSON object to keep the path on success.
    let stdout_body =
        r#"{"type":"result","result":"GITIM_OK should appear in output","is_error":false}"#;
    let script = make_capture_script(
        tmp.path(),
        "fake_claude.sh",
        &capture,
        stdout_body,
        &["MY_TEST_KEY"],
    );

    let mut env = HashMap::new();
    env.insert("MY_TEST_KEY".to_string(), "claude-env-here".to_string());

    let overrides = PreflightDispatchOverrides {
        claude_bin: Some(script.to_string_lossy().into_owned()),
        ..Default::default()
    };

    let result = preflight_for_add_request_with_overrides(
        "claude",
        Some(&env),
        Some("test-model-abc"),
        None,
        None,
        overrides,
    )
    .await;

    // Should have succeeded (script printed JSON with GITIM_OK).
    assert!(result.available, "preflight should succeed, got {result:?}");
    assert_eq!(result.provider, "claude");
    assert_eq!(result.model_used.as_deref(), Some("test-model-abc"));

    // Verify env + model reached the fake binary.
    let captured = std::fs::read_to_string(&capture).unwrap();
    assert!(
        captured.contains("test-model-abc"),
        "expected model override in argv, got: {captured}"
    );
    assert!(
        captured.contains("MY_TEST_KEY=claude-env-here"),
        "expected env override at child env, got: {captured}"
    );
}

// ─── hermes dispatch ────────────────────────────────────────────────────────

#[tokio::test]
async fn hermes_dual_llm_calls_preflight_hermes_with() {
    // Both llm_provider and llm_model supplied → should call chat-mode hermes
    // preflight with the explicit pair.
    let tmp = TempDir::new().unwrap();
    let capture = tmp.path().join("argv.txt");
    // Hermes chat path: emit GITIM_OK so the success branch fires.
    let script = make_capture_script(
        tmp.path(),
        "fake_hermes.sh",
        &capture,
        "GITIM_OK",
        &["HERMES_HOME"],
    );

    let overrides = PreflightDispatchOverrides {
        hermes_bin: Some(script.to_string_lossy().into_owned()),
        ..Default::default()
    };

    let result = preflight_for_add_request_with_overrides(
        "hermes",
        None,
        None,
        Some("minimax-cn"),
        Some("MiniMax-M2.7-highspeed"),
        overrides,
    )
    .await;

    assert!(result.available, "hermes chat should succeed, got {result:?}");
    assert_eq!(result.provider, "hermes");

    let captured = std::fs::read_to_string(&capture).unwrap();
    assert!(
        captured.contains("--provider"),
        "expected --provider in argv: {captured}"
    );
    assert!(
        captured.contains("minimax-cn"),
        "expected minimax-cn in argv: {captured}"
    );
    assert!(
        captured.contains("--model"),
        "expected --model in argv: {captured}"
    );
    assert!(
        captured.contains("MiniMax-M2.7-highspeed"),
        "expected MiniMax-M2.7-highspeed in argv: {captured}"
    );
}

#[tokio::test]
async fn hermes_missing_one_llm_returns_missing_llm_provider_code() {
    // Only llm_provider supplied → setup-level failure, no spawn.
    let result =
        preflight_for_add_request("hermes", None, None, Some("anthropic"), None).await;

    assert!(!result.available);
    assert_eq!(result.provider, "hermes");
    assert_eq!(
        result.failure_code.as_deref(),
        Some(FAILURE_CODE_MISSING_LLM_PROVIDER)
    );
    assert_eq!(
        classify_preflight_error_code(&result),
        FAILURE_CODE_MISSING_LLM_PROVIDER
    );

    // And the mirror case: only llm_model supplied.
    let result2 = preflight_for_add_request(
        "hermes",
        None,
        None,
        None,
        Some("claude-opus-4-7"),
    )
    .await;
    assert_eq!(
        result2.failure_code.as_deref(),
        Some(FAILURE_CODE_MISSING_LLM_PROVIDER)
    );
}

#[tokio::test]
async fn hermes_no_llm_default_profile_present_uses_resolved_llm() {
    // Both llm params omitted but default-profile config.yaml has model →
    // should resolve from yaml and dispatch chat-mode with that pair.
    let tmp = TempDir::new().unwrap();
    let hermes_home = tmp.path().join("hermes_home");
    std::fs::create_dir_all(&hermes_home).unwrap();
    let yaml = "model:\n  default: claude-haiku-4-5\n  provider: anthropic\n";
    std::fs::write(hermes_home.join("config.yaml"), yaml).unwrap();

    let capture = tmp.path().join("argv.txt");
    let script = make_capture_script(
        tmp.path(),
        "fake_hermes.sh",
        &capture,
        "GITIM_OK",
        &["HERMES_HOME"],
    );

    let overrides = PreflightDispatchOverrides {
        hermes_bin: Some(script.to_string_lossy().into_owned()),
        hermes_home: Some(hermes_home.clone()),
        ..Default::default()
    };

    let result = preflight_for_add_request_with_overrides(
        "hermes", None, None, None, None, overrides,
    )
    .await;

    assert!(
        result.available,
        "hermes default-profile-resolve should succeed, got {result:?}"
    );

    let captured = std::fs::read_to_string(&capture).unwrap();
    // The resolved (provider, model) pair should appear in argv.
    assert!(
        captured.contains("anthropic"),
        "expected resolved provider in argv: {captured}"
    );
    assert!(
        captured.contains("claude-haiku-4-5"),
        "expected resolved model in argv: {captured}"
    );
    // HERMES_HOME should have been propagated to the child process.
    assert!(
        captured.contains(&format!("HERMES_HOME={}", hermes_home.display())),
        "expected HERMES_HOME propagated to child env: {captured}"
    );
}

#[tokio::test]
async fn hermes_no_llm_default_profile_missing_llm_returns_default_profile_no_llm_code() {
    // Both llm params omitted AND default-profile config.yaml absent → setup
    // failure tagged with hermes_default_profile_no_llm. No spawn.
    let tmp = TempDir::new().unwrap();
    let hermes_home = tmp.path().join("hermes_home_empty");
    std::fs::create_dir_all(&hermes_home).unwrap();
    // Intentionally do NOT write config.yaml.

    let overrides = PreflightDispatchOverrides {
        // We still set a hermes_bin in case the dispatcher mistakenly spawned;
        // we want a clear assertion failure, not a "binary not found" error
        // that masks the real bug.
        hermes_bin: Some("/usr/bin/false".to_string()),
        hermes_home: Some(hermes_home),
        ..Default::default()
    };

    let result = preflight_for_add_request_with_overrides(
        "hermes", None, None, None, None, overrides,
    )
    .await;

    assert!(!result.available, "expected failure, got {result:?}");
    assert_eq!(result.provider, "hermes");
    assert_eq!(
        result.failure_code.as_deref(),
        Some(FAILURE_CODE_HERMES_NO_LLM)
    );
    assert_eq!(
        classify_preflight_error_code(&result),
        FAILURE_CODE_HERMES_NO_LLM
    );
    // No spawn → duration should be near-zero.
    assert!(
        result.duration_ms < 50,
        "hermes_default_profile_no_llm path shelled out unexpectedly ({} ms)",
        result.duration_ms
    );
}

#[tokio::test]
async fn hermes_no_llm_default_profile_yaml_without_model_returns_default_profile_no_llm_code() {
    // Default profile exists but yaml lacks the model.* keys → still
    // failure_code = hermes_default_profile_no_llm.
    let tmp = TempDir::new().unwrap();
    let hermes_home = tmp.path().join("hermes_home_partial");
    std::fs::create_dir_all(&hermes_home).unwrap();
    let yaml = "auth:\n  provider: anthropic\n"; // no `model:` key at all
    std::fs::write(hermes_home.join("config.yaml"), yaml).unwrap();

    let overrides = PreflightDispatchOverrides {
        hermes_bin: Some("/usr/bin/false".to_string()),
        hermes_home: Some(hermes_home),
        ..Default::default()
    };

    let result = preflight_for_add_request_with_overrides(
        "hermes", None, None, None, None, overrides,
    )
    .await;

    assert_eq!(
        result.failure_code.as_deref(),
        Some(FAILURE_CODE_HERMES_NO_LLM)
    );
}

// ─── classifier ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn classify_unknown_failure_code_returns_provision_preflight_failed() {
    // A PreflightResult with no failure_code (the typical spawn-fail case) →
    // falls through to the generic top-level code.
    let pf = PreflightResult::failure(
        "claude",
        ErrorKind::NotInstalled,
        "claude CLI not found",
        12,
    );
    assert!(pf.failure_code.is_none());
    assert_eq!(
        classify_preflight_error_code(&pf),
        ERROR_CODE_PROVISION_PREFLIGHT_FAILED
    );
}

#[tokio::test]
async fn classify_recognizes_each_setup_level_code() {
    // Round-trip each known tag through classify to lock the mapping in tests.
    let pf_unknown = PreflightResult::failure_with_code(
        "x",
        ErrorKind::Other,
        "x",
        0,
        FAILURE_CODE_UNKNOWN_PROVIDER,
    );
    assert_eq!(
        classify_preflight_error_code(&pf_unknown),
        FAILURE_CODE_UNKNOWN_PROVIDER
    );

    let pf_missing = PreflightResult::failure_with_code(
        "hermes",
        ErrorKind::Other,
        "x",
        0,
        FAILURE_CODE_MISSING_LLM_PROVIDER,
    );
    assert_eq!(
        classify_preflight_error_code(&pf_missing),
        FAILURE_CODE_MISSING_LLM_PROVIDER
    );

    let pf_no_llm = PreflightResult::failure_with_code(
        "hermes",
        ErrorKind::Other,
        "x",
        0,
        FAILURE_CODE_HERMES_NO_LLM,
    );
    assert_eq!(
        classify_preflight_error_code(&pf_no_llm),
        FAILURE_CODE_HERMES_NO_LLM
    );

    // Unknown failure_code value (e.g. a typo) — fall through to generic.
    let pf_garbage = PreflightResult::failure_with_code(
        "x",
        ErrorKind::Other,
        "x",
        0,
        "not-a-known-tag",
    );
    assert_eq!(
        classify_preflight_error_code(&pf_garbage),
        ERROR_CODE_PROVISION_PREFLIGHT_FAILED
    );
}

// ─── outer timeout ──────────────────────────────────────────────────────────

#[tokio::test]
async fn outer_timeout_fires_with_slow_binary() {
    // Slow fake binary + tight outer_timeout override → the outer
    // `tokio::time::timeout` wrap should trip before the inner per-provider
    // timeout (60s) does. Verifies the outer cap is wired correctly.
    let tmp = TempDir::new().unwrap();
    let script = tmp.path().join("slow_claude.sh");
    std::fs::write(&script, "#!/bin/sh\nsleep 60\nexit 0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let overrides = PreflightDispatchOverrides {
        claude_bin: Some(script.to_string_lossy().into_owned()),
        outer_timeout: Some(Duration::from_millis(400)),
        ..Default::default()
    };

    let started = std::time::Instant::now();
    let result = preflight_for_add_request_with_overrides(
        "claude", None, None, None, None, overrides,
    )
    .await;
    let elapsed = started.elapsed();

    assert!(!result.available);
    assert_eq!(result.provider, "claude");
    assert_eq!(result.error_kind, Some(ErrorKind::Timeout));
    // No setup-level tag — top-level should fall through to generic code.
    assert!(result.failure_code.is_none());
    assert_eq!(
        classify_preflight_error_code(&result),
        ERROR_CODE_PROVISION_PREFLIGHT_FAILED
    );
    // We should have returned in well under the inner 60s default.
    assert!(
        elapsed < Duration::from_secs(5),
        "outer timeout didn't fire promptly: elapsed = {elapsed:?}"
    );
}
