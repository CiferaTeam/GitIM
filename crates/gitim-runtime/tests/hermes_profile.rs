//! Integration tests for `hermes_profile::ensure_profile` / `delete_profile`.
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
    ensure_profile, profile_dir, EnsureOutcome,
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

/// Removes a profile directory if it exists. Used as test cleanup until
/// `delete_profile` lands in Task 2.2.
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
