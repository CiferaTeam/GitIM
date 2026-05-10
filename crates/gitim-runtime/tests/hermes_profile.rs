//! Integration tests for `hermes_profile::ensure_profile` / `delete_profile`
//! / `apply_model_config`.
//!
//! These tests shell out to the real `hermes` CLI and so are marked
//! `#[ignore]` — opt-in via:
//!
//! ```bash
//! cargo test -p gitim-runtime --test hermes_profile -- --include-ignored
//! ```
//!
//! Each test uses a unique random profile suffix to avoid collisions with
//! manually-created profiles. Cleanup runs in a guard so a panic mid-test
//! still removes the profile.

use std::time::{SystemTime, UNIX_EPOCH};

use gitim_runtime::hermes_profile::{
    apply_model_config, apply_model_config_with, delete_profile, ensure_profile, profile_dir,
    EnsureOutcome, HermesProfileError,
};

/// Random suffix derived from monotonic clock — fits hermes's profile name
/// regex `[a-z0-9][a-z0-9_-]{0,63}` and is unique across parallel runs.
fn unique_handler() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("rstest-{nanos}")
}

/// Drop-safe sync cleanup — uses fs::remove_dir_all rather than the async
/// `delete_profile` so it can run from a Drop impl. The
/// `delete_profile_*` tests exercise the production deletion path
/// independently.
fn cleanup_profile(handler: &str) {
    if let Ok(dir) = profile_dir(handler) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
#[ignore = "requires hermes CLI in PATH"]
async fn ensure_profile_creates_new() {
    let handler = unique_handler();
    let _guard = scopeguard_cleanup(&handler);

    let outcome = ensure_profile(&handler)
        .await
        .expect("ensure_profile should succeed");
    assert_eq!(outcome, EnsureOutcome::Created);

    let dir = profile_dir(&handler).unwrap();
    assert!(dir.is_dir(), "profile dir not created at {}", dir.display());
    assert!(
        dir.join("config.yaml").is_file(),
        "config.yaml missing after --clone"
    );
}

#[tokio::test]
#[ignore = "requires hermes CLI in PATH"]
async fn ensure_profile_idempotent() {
    let handler = unique_handler();
    let _guard = scopeguard_cleanup(&handler);

    let first = ensure_profile(&handler).await.expect("first create");
    assert_eq!(first, EnsureOutcome::Created);

    let second = ensure_profile(&handler).await.expect("second create");
    assert_eq!(second, EnsureOutcome::AlreadyExists);
}

#[tokio::test]
#[ignore = "requires hermes CLI in PATH"]
async fn delete_profile_removes_existing() {
    let handler = unique_handler();
    let _guard = scopeguard_cleanup(&handler);

    ensure_profile(&handler).await.expect("create");
    let dir = profile_dir(&handler).unwrap();
    assert!(dir.is_dir(), "profile should exist before delete");

    delete_profile(&handler).await.expect("delete");
    assert!(!dir.exists(), "profile should be gone after delete");
}

#[tokio::test]
#[ignore = "requires hermes CLI in PATH"]
async fn delete_profile_missing_is_noop() {
    let handler = unique_handler();
    // No ensure_profile — directly delete a profile that never existed.
    delete_profile(&handler)
        .await
        .expect("delete-missing should not error");
}

// Lightweight scope-guard so cleanup runs even on test panic.
struct Cleanup<'a>(&'a str);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        cleanup_profile(self.0);
    }
}
fn scopeguard_cleanup(handler: &str) -> Cleanup<'_> {
    Cleanup(handler)
}

// ─── apply_model_config unit tests (no real hermes needed) ───────────────────

/// Test 1: non-existent binary → CliNotFound (mirrors ensure_profile pattern).
#[tokio::test]
async fn apply_model_config_with_nonexistent_binary_returns_cli_not_found() {
    let err = apply_model_config_with("alice", "openai", "gpt-4o", None, "/nonexistent/xyz")
        .await
        .expect_err("expected CliNotFound");
    assert!(
        matches!(err, HermesProfileError::CliNotFound),
        "expected CliNotFound, got: {err}"
    );
}

/// Test 2: binary that always exits non-zero → Other error.
///
/// Uses `/usr/bin/false` (macOS path) or `/bin/false` (Linux path), whichever
/// exists. The binary exits 1 immediately without printing to stderr, so we
/// get an Other error with an empty-ish message.
#[tokio::test]
async fn apply_model_config_with_failing_binary_returns_other_error() {
    // /bin/false doesn't exist on macOS; /usr/bin/false is the correct path.
    let false_bin = if std::path::Path::new("/usr/bin/false").exists() {
        "/usr/bin/false"
    } else {
        "/bin/false"
    };

    let err = apply_model_config_with("alice", "openai", "gpt-4o", None, false_bin)
        .await
        .expect_err("expected Other error");
    assert!(
        matches!(err, HermesProfileError::Other(_)),
        "expected Other(_), got: {err}"
    );
}

/// Test 3: step 1 failure stops execution — step 2 is never run.
///
/// Strategy: write a shell script that records each invocation's arguments
/// to a numbered file, and exits non-zero on every call. After
/// `apply_model_config_with` returns an error, assert that only 1 invocation
/// file was created (not 2+), confirming the sequence stopped at model.provider.
#[tokio::test]
async fn apply_model_config_step1_failure_does_not_run_step2() {
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let script_path = tmp.path().join("fake_hermes.sh");
    let invocations_dir = tmp.path().join("invocations");
    std::fs::create_dir_all(&invocations_dir).unwrap();

    // Script: write argv to a sequenced file, then exit 1.
    let script_content = format!(
        "#!/bin/sh\nn=$(ls \"{dir}\" | wc -l)\necho \"$@\" > \"{dir}/$n.txt\"\nexit 1\n",
        dir = invocations_dir.display()
    );
    std::fs::write(&script_path, script_content).unwrap();
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let result = apply_model_config_with(
        "alice",
        "openai",
        "gpt-4o",
        None,
        script_path.to_str().unwrap(),
    )
    .await;

    assert!(result.is_err(), "expected error after step 1 failure");

    let count = std::fs::read_dir(&invocations_dir).unwrap().count();
    assert_eq!(
        count, 1,
        "expected exactly 1 invocation (step 1 only), got {count}"
    );

    // Confirm the single invocation was for model.provider, not model.default.
    let first_file = std::fs::read_dir(&invocations_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let args = std::fs::read_to_string(first_file).unwrap();
    assert!(
        args.contains("model.provider"),
        "step 1 args should mention model.provider, got: {args}"
    );
}

/// Test 4: when base_url is None, only 2 shell-outs are issued (not 3).
///
/// Uses the same invocation-counter fake bin pattern as test 3, but with a
/// succeeding script so all steps run. Confirms exactly 2 files when
/// base_url=None, and exactly 3 when base_url=Some.
#[tokio::test]
async fn apply_model_config_skips_base_url_when_none() {
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // ── None case: expect 2 invocations ──────────────────────────────────────
    {
        let tmp = TempDir::new().unwrap();
        let script_path = tmp.path().join("fake_hermes.sh");
        let invocations_dir = tmp.path().join("invocations");
        std::fs::create_dir_all(&invocations_dir).unwrap();

        let script_content = format!(
            "#!/bin/sh\nn=$(ls \"{dir}\" | wc -l)\necho \"$@\" > \"{dir}/$n.txt\"\nexit 0\n",
            dir = invocations_dir.display()
        );
        std::fs::write(&script_path, script_content).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        apply_model_config_with(
            "alice",
            "openai",
            "gpt-4o",
            None,
            script_path.to_str().unwrap(),
        )
        .await
        .expect("should succeed with None base_url");

        let count = std::fs::read_dir(&invocations_dir).unwrap().count();
        assert_eq!(
            count, 2,
            "base_url=None should produce 2 shell-outs, got {count}"
        );
    }

    // ── Some case: expect 3 invocations ──────────────────────────────────────
    {
        let tmp = TempDir::new().unwrap();
        let script_path = tmp.path().join("fake_hermes.sh");
        let invocations_dir = tmp.path().join("invocations");
        std::fs::create_dir_all(&invocations_dir).unwrap();

        let script_content = format!(
            "#!/bin/sh\nn=$(ls \"{dir}\" | wc -l)\necho \"$@\" > \"{dir}/$n.txt\"\nexit 0\n",
            dir = invocations_dir.display()
        );
        std::fs::write(&script_path, script_content).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        apply_model_config_with(
            "alice",
            "openai",
            "gpt-4o",
            Some("https://api.example.com/v1"),
            script_path.to_str().unwrap(),
        )
        .await
        .expect("should succeed with Some base_url");

        let count = std::fs::read_dir(&invocations_dir).unwrap().count();
        assert_eq!(
            count, 3,
            "base_url=Some should produce 3 shell-outs, got {count}"
        );
    }
}

// ─── apply_model_config integration test (requires real hermes) ──────────────

/// Ignored integration test: verify real hermes writes model config to
/// `config.yaml`.
///
/// Run with:
/// ```bash
/// cargo test -p gitim-runtime --test hermes_profile -- --ignored apply_model_config_real
/// ```
#[tokio::test]
#[ignore = "requires hermes CLI in PATH and a configured default profile"]
async fn apply_model_config_real_writes_config_yaml() {
    let handler = unique_handler();
    let _guard = scopeguard_cleanup(&handler);

    // Create profile first (borrows from active profile).
    ensure_profile(&handler)
        .await
        .expect("ensure_profile should succeed");

    // Apply model config.
    apply_model_config(&handler, "minimax-cn", "MiniMax-M2.7-highspeed", None)
        .await
        .expect("apply_model_config should succeed");

    // Read config.yaml and verify model subtree.
    let config_path = profile_dir(&handler).unwrap().join("config.yaml");
    assert!(config_path.is_file(), "config.yaml missing after apply");
    let contents = std::fs::read_to_string(&config_path).unwrap();

    assert!(
        contents.contains("minimax-cn"),
        "model.provider not written to config.yaml; contents:\n{contents}"
    );
    assert!(
        contents.contains("MiniMax-M2.7-highspeed"),
        "model.default not written to config.yaml; contents:\n{contents}"
    );

    // Cleanup via delete_profile (production path).
    delete_profile(&handler)
        .await
        .expect("delete_profile should succeed");
}
