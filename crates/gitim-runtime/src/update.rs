//! Self-update endpoint: `POST /runtime/update-and-restart`.
//!
//! Two-phase flow. This file implements the **synchronous** phase: validate
//! install dir, resolve target version, download + extract + sanity-check the
//! new tarball into a tempdir. If every step passes we spawn the async phase
//! (Task 7 fills that in) and return `202 Accepted` with a job id.
//!
//! Any failure in the sync phase returns a structured error body with one of
//! the codes in [`error_codes`] and an appropriate HTTP status. The sync phase
//! is pure preflight — nothing on disk outside a tempdir is mutated, so
//! rolling back a sync failure costs nothing.
//!
//! The `update_in_progress` atomic on [`crate::http::RuntimeState`] guards
//! against two concurrent updates colliding mid-replace. Clients that hit this
//! endpoint while one is already running get `409 concurrent_update`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::http::SharedRuntimeState;

/// Hard cap for the new-runtime `--version` sanity check. Long enough for
/// cold-start binary load on a slow disk, short enough that a wedged binary
/// can't stall the endpoint.
const SANITY_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// 202 body when all sync validation passes and the async phase has been
/// spawned. The caller polls (Task 7) or just waits for the runtime HTTP
/// server to restart.
#[derive(Debug, Serialize)]
pub struct UpdateJob {
    pub job_id: String,
    pub target_version: String,
    pub started_at: DateTime<Utc>,
}

/// Structured error body. `error_code` is the stable machine-readable field;
/// `detail` is a human sentence safe to show in a log or UI toast.
#[derive(Debug, Serialize)]
pub struct UpdateError {
    pub error_code: String,
    pub detail: String,
}

/// Error code constants. Keep in sync with the WebUI switch statement and the
/// Task 6 plan doc — these strings are part of the HTTP contract.
pub mod error_codes {
    pub const RUNTIME_NOT_INSTALLED: &str = "runtime_not_installed";
    pub const UNSUPPORTED_PLATFORM: &str = "unsupported_platform";
    pub const NETWORK: &str = "network";
    pub const ALREADY_LATEST: &str = "already_latest";
    pub const DOWNLOAD_FAILED: &str = "download_failed";
    pub const EXTRACT_FAILED: &str = "extract_failed";
    pub const ARCHIVE_MISSING_BINARIES: &str = "archive_missing_binaries";
    pub const SANITY_CHECK_FAILED: &str = "sanity_check_failed";
    pub const CONCURRENT_UPDATE: &str = "concurrent_update";
}

/// Map an error code to its HTTP status. Centralized so response helpers and
/// tests agree on the mapping.
fn status_for(code: &str) -> StatusCode {
    match code {
        error_codes::RUNTIME_NOT_INSTALLED => StatusCode::FORBIDDEN,
        error_codes::UNSUPPORTED_PLATFORM => StatusCode::BAD_REQUEST,
        error_codes::ALREADY_LATEST => StatusCode::BAD_REQUEST,
        error_codes::CONCURRENT_UPDATE => StatusCode::CONFLICT,
        // network / download / extract / archive / sanity -> 500
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn error_response(err: UpdateError) -> Response {
    let status = status_for(&err.error_code);
    (status, Json(err)).into_response()
}

/// Strict install-dir check: `exe` must sit in a canonicalized
/// `~/.gitim/bin/`.
///
/// Both sides are canonicalized before compare — on macOS `/var/folders/...`
/// canonicalizes to `/private/var/folders/...`, and any component of the
/// runtime's install path could likewise be a symlink.
pub(crate) fn strict_install_dir_check(exe: &Path) -> Result<(), UpdateError> {
    let parent = exe.parent().ok_or_else(|| UpdateError {
        error_code: error_codes::RUNTIME_NOT_INSTALLED.into(),
        detail: "cannot determine parent of runtime exe path".into(),
    })?;
    let home = dirs::home_dir().ok_or_else(|| UpdateError {
        error_code: error_codes::RUNTIME_NOT_INSTALLED.into(),
        detail: "home directory not available".into(),
    })?;
    let expected = home
        .join(".gitim/bin")
        .canonicalize()
        .map_err(|e| UpdateError {
            error_code: error_codes::RUNTIME_NOT_INSTALLED.into(),
            detail: format!("canonicalize ~/.gitim/bin failed: {e}; is gitim installed?"),
        })?;
    let actual = parent.canonicalize().map_err(|e| UpdateError {
        error_code: error_codes::RUNTIME_NOT_INSTALLED.into(),
        detail: format!("canonicalize exe parent failed: {e}"),
    })?;
    if actual != expected {
        return Err(UpdateError {
            error_code: error_codes::RUNTIME_NOT_INSTALLED.into(),
            detail: format!(
                "runtime not launched from ~/.gitim/bin (actual: {})",
                actual.display()
            ),
        });
    }
    Ok(())
}

/// Run `<new_runtime> --version` with a hard timeout. We require the command
/// to (a) exit successfully inside the timeout and (b) print a stdout line
/// containing the target version — a freshly-built binary that reports the
/// wrong version means the tarball was mis-packed for the tag we fetched.
async fn sanity_check_new_runtime(
    new_runtime: &Path,
    target_version: &str,
) -> Result<(), UpdateError> {
    let result = tokio::time::timeout(
        SANITY_CHECK_TIMEOUT,
        tokio::process::Command::new(new_runtime)
            .arg("--version")
            .output(),
    )
    .await;

    let output = match result {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err(UpdateError {
                error_code: error_codes::SANITY_CHECK_FAILED.into(),
                detail: format!("failed to exec new runtime --version: {e}"),
            });
        }
        Err(_) => {
            return Err(UpdateError {
                error_code: error_codes::SANITY_CHECK_FAILED.into(),
                detail: format!(
                    "new runtime --version timed out after {}s",
                    SANITY_CHECK_TIMEOUT.as_secs()
                ),
            });
        }
    };

    if !output.status.success() {
        return Err(UpdateError {
            error_code: error_codes::SANITY_CHECK_FAILED.into(),
            detail: format!(
                "new runtime --version exited with status {}",
                output.status
            ),
        });
    }

    // Version comparison tolerates an optional leading `v`. The binary prints
    // a bare version (`gitim-runtime 0.4.2`) while the tag carries `v` —
    // accept either representation in the output.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let bare = target_version.strip_prefix('v').unwrap_or(target_version);
    if !stdout.contains(bare) {
        return Err(UpdateError {
            error_code: error_codes::SANITY_CHECK_FAILED.into(),
            detail: format!(
                "new runtime --version output did not mention target version {target_version}: {}",
                stdout.trim()
            ),
        });
    }

    Ok(())
}

/// Post-install-dir, post-concurrency-guard body of the handler. Returns the
/// job description on success or an [`UpdateError`] on any failure. The caller
/// is responsible for releasing the `update_in_progress` flag on error.
///
/// Split out from [`update_and_restart`] so tests can drive the core flow
/// without needing a router or state; however at present the integration
/// tests exercise it via the HTTP surface, so this function stays private.
async fn run_sync_phase() -> Result<(UpdateJob, tempfile::TempDir, PathBuf), UpdateError> {
    // 2. Platform detection.
    let platform = gitim_updater::detect_platform().map_err(|e| UpdateError {
        error_code: error_codes::UNSUPPORTED_PLATFORM.into(),
        detail: format!("{e}"),
    })?;

    // 3. Fetch latest tag from GitHub releases.
    let latest_tag = gitim_updater::fetch_latest_tag().await.map_err(|e| UpdateError {
        error_code: error_codes::NETWORK.into(),
        detail: format!("fetch latest tag failed: {e}"),
    })?;

    // 4. Already-latest short-circuit. We compare strict semver via
    //    `is_newer`; anything else (equal, older, unparsable) is "no work to
    //    do" and returns 400 so WebUI can surface a clear message instead of
    //    pretending it started an update.
    let current = env!("CARGO_PKG_VERSION");
    if !gitim_updater::is_newer(current, &latest_tag) {
        return Err(UpdateError {
            error_code: error_codes::ALREADY_LATEST.into(),
            detail: format!(
                "current version {current} is already at or newer than latest {latest_tag}"
            ),
        });
    }

    // 5. Download + extract into a tempdir. `TempDir` auto-cleans on drop so
    //    failed updates leave no crumbs on disk.
    let tmp = tempfile::Builder::new()
        .prefix("gitim-update-")
        .tempdir()
        .map_err(|e| UpdateError {
            error_code: error_codes::EXTRACT_FAILED.into(),
            detail: format!("create tempdir failed: {e}"),
        })?;
    let url = gitim_updater::download_url(&latest_tag, &platform);
    gitim_updater::download_and_extract(&url, tmp.path())
        .await
        .map_err(|e| match e {
            // `HttpStatus` means "reached GitHub, got a non-2xx" — treat as a
            // download failure (bad URL / missing asset), not a generic
            // network outage.
            gitim_updater::UpdateError::HttpStatus(_)
            | gitim_updater::UpdateError::Network(_) => UpdateError {
                error_code: error_codes::DOWNLOAD_FAILED.into(),
                detail: format!("download failed: {e}"),
            },
            _ => UpdateError {
                error_code: error_codes::EXTRACT_FAILED.into(),
                detail: format!("extract failed: {e}"),
            },
        })?;

    // 6. Verify the archive carries all three binaries. Missing any one means
    //    the tarball was packed wrong and we refuse to touch disk.
    for bin in gitim_updater::BINARIES {
        if gitim_updater::find_binary(tmp.path(), bin).is_none() {
            return Err(UpdateError {
                error_code: error_codes::ARCHIVE_MISSING_BINARIES.into(),
                detail: format!("binary missing from archive: {bin}"),
            });
        }
    }

    // 7. Sanity-check the new runtime by invoking its `--version`.
    let new_runtime = gitim_updater::find_binary(tmp.path(), "gitim-runtime").ok_or_else(|| {
        // Unreachable after the BINARIES loop above — guard anyway so the
        // unwrap doesn't live in production code.
        UpdateError {
            error_code: error_codes::ARCHIVE_MISSING_BINARIES.into(),
            detail: "gitim-runtime not found after extraction".into(),
        }
    })?;
    sanity_check_new_runtime(&new_runtime, &latest_tag).await?;

    // All sync checks passed. Build the job description. The async phase
    // (Task 7) takes the tempdir and the new_runtime path.
    let job = UpdateJob {
        job_id: uuid::Uuid::new_v4().to_string(),
        target_version: latest_tag,
        started_at: Utc::now(),
    };
    Ok((job, tmp, new_runtime))
}

/// Handler for `POST /runtime/update-and-restart`.
pub async fn update_and_restart(State(state): State<SharedRuntimeState>) -> Response {
    tracing::info!("update_and_restart: sync phase start");

    // 1. Strict install-dir check. Done first so dev-tree `cargo run`
    //    invocations of the endpoint fail loudly before we do any network IO.
    let exe_path = {
        let s = state.lock().unwrap();
        s.canonical_exe_path.clone()
    };
    if let Err(err) = strict_install_dir_check(&exe_path) {
        tracing::info!(error_code = %err.error_code, detail = %err.detail, "update_and_restart: rejected");
        return error_response(err);
    }

    // Concurrency guard. `swap(true, SeqCst)` returns the previous value —
    // if it was already true we know another update is live and bail with 409.
    let guard = {
        let s = state.lock().unwrap();
        s.update_in_progress.clone()
    };
    if guard.swap(true, Ordering::SeqCst) {
        tracing::info!("update_and_restart: concurrent_update rejected");
        return error_response(UpdateError {
            error_code: error_codes::CONCURRENT_UPDATE.into(),
            detail: "another update is already in progress".into(),
        });
    }

    // From here on, any early return must clear the flag.
    let sync_result = run_sync_phase().await;

    let (job, tmp, new_runtime) = match sync_result {
        Ok(x) => x,
        Err(err) => {
            guard.store(false, Ordering::SeqCst);
            tracing::warn!(
                error_code = %err.error_code,
                detail = %err.detail,
                "update_and_restart: sync phase failed",
            );
            return error_response(err);
        }
    };

    tracing::info!(
        job_id = %job.job_id,
        target_version = %job.target_version,
        new_runtime = %new_runtime.display(),
        "update_and_restart: sync phase ok, spawning async phase",
    );

    // --- Task 7 async phase stub. ---
    //
    // Task 7 replaces this with: kill daemons → replace_binaries → fork-exec
    // the new runtime. For Task 6 we just log and clear the flag after a
    // short delay so the endpoint's contract (202 + job id) is testable end
    // to end without touching the installed binaries.
    //
    // We keep ownership of `tmp` in the spawned task — dropping it here would
    // delete the extracted binaries before the async phase could install them.
    let guard_clone = guard.clone();
    tokio::spawn(async move {
        // Prevent the tempdir + new_runtime path from being moved away from
        // the task's scope, which would drop + delete the extracted binaries
        // the moment the spawn body returned.
        let _tmp = tmp;
        let _new_runtime = new_runtime;
        tracing::info!("update_and_restart: async phase stub (Task 7 replaces this)");
        tokio::time::sleep(Duration::from_millis(100)).await;
        guard_clone.store(false, Ordering::SeqCst);
        tracing::info!("update_and_restart: async phase stub complete, guard released");
    });

    (StatusCode::ACCEPTED, Json(job)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_for_maps_expected_codes() {
        assert_eq!(
            status_for(error_codes::RUNTIME_NOT_INSTALLED),
            StatusCode::FORBIDDEN,
        );
        assert_eq!(
            status_for(error_codes::UNSUPPORTED_PLATFORM),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            status_for(error_codes::ALREADY_LATEST),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            status_for(error_codes::CONCURRENT_UPDATE),
            StatusCode::CONFLICT,
        );
        assert_eq!(
            status_for(error_codes::NETWORK),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
        assert_eq!(
            status_for(error_codes::DOWNLOAD_FAILED),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
        assert_eq!(
            status_for(error_codes::EXTRACT_FAILED),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
        assert_eq!(
            status_for(error_codes::ARCHIVE_MISSING_BINARIES),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
        assert_eq!(
            status_for(error_codes::SANITY_CHECK_FAILED),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }

    #[test]
    fn strict_install_dir_check_rejects_tempfile_path() {
        // Any path definitely outside ~/.gitim/bin: a freshly-created tempfile.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let err = strict_install_dir_check(tmp.path()).unwrap_err();
        assert_eq!(err.error_code, error_codes::RUNTIME_NOT_INSTALLED);
    }

    #[test]
    fn strict_install_dir_check_rejects_rootless_path() {
        let err = strict_install_dir_check(Path::new("gitim-runtime")).unwrap_err();
        assert_eq!(err.error_code, error_codes::RUNTIME_NOT_INSTALLED);
    }

    #[tokio::test]
    async fn sanity_check_fails_on_timeout() {
        // `sleep` never prints a version, so we'll hit the timeout branch.
        // Available on macOS + Linux runners.
        let sleep_path = which_sleep();
        let err = sanity_check_new_runtime(&sleep_path, "v9.9.9").await.unwrap_err();
        assert_eq!(err.error_code, error_codes::SANITY_CHECK_FAILED);
    }

    #[tokio::test]
    async fn sanity_check_fails_on_version_mismatch() {
        // `echo` prints its args to stdout with exit 0 — perfect for
        // simulating a runtime whose --version output lacks the target tag.
        let echo_path = which_echo();
        let err = sanity_check_new_runtime(&echo_path, "v9.9.9")
            .await
            .unwrap_err();
        assert_eq!(err.error_code, error_codes::SANITY_CHECK_FAILED);
        assert!(err.detail.contains("did not mention target version"));
    }

    fn which_sleep() -> PathBuf {
        for candidate in ["/bin/sleep", "/usr/bin/sleep"] {
            if Path::new(candidate).exists() {
                return PathBuf::from(candidate);
            }
        }
        panic!("sleep binary not found on this platform");
    }

    fn which_echo() -> PathBuf {
        for candidate in ["/bin/echo", "/usr/bin/echo"] {
            if Path::new(candidate).exists() {
                return PathBuf::from(candidate);
            }
        }
        panic!("echo binary not found on this platform");
    }
}
