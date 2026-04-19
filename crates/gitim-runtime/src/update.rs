//! Self-update endpoint: `POST /runtime/update-and-restart`.
//!
//! Two-phase flow.
//!
//! **Sync phase** (`run_sync_phase`): validate install dir, resolve target
//! version, download + extract + sanity-check the new tarball into a tempdir.
//! On success returns `202 Accepted` with a job id and spawns the async phase.
//! Any failure returns a structured error with one of the codes in
//! [`error_codes`] and an appropriate HTTP status. Nothing on disk outside
//! the tempdir is mutated, so rolling back a sync failure costs nothing.
//!
//! **Async phase** (`run_async_phase`): kill every managed daemon, atomically
//! replace the three binaries on disk (rolling back on failure), spawn a
//! fresh runtime with the same `--port`, and `std::process::exit(0)` so the
//! replacement can bind the TCP port. The handler's caller sees the socket
//! close briefly and then comes back with the new `/health` version once the
//! child is bound — we cannot avoid that gap, only keep it short.
//!
//! Ordering is load-bearing. The sequence is: parent spawns child (child is
//! suspended in its own bind-retry loop, waiting for the port to free up),
//! parent exits — which releases the listening port — and the child's retry
//! then succeeds. If we waited for the child to be healthy before exiting,
//! we'd still hold the port and the child would never bind. The retry loop
//! in the child's `run_shell` covers the small window where the parent is
//! on its way out but hasn't released the socket yet. The frontend bridges
//! the user-visible gap with a polling loop on `/health`.
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
///
/// The child is spawned explicitly with `kill_on_drop(true)` so that a
/// timeout doesn't leak a zombie process holding file descriptors into the
/// tempdir: dropping the `wait_with_output` future drops the inner `Child`,
/// which sends SIGKILL.
async fn sanity_check_new_runtime(
    new_runtime: &Path,
    target_version: &str,
) -> Result<(), UpdateError> {
    let child = tokio::process::Command::new(new_runtime)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| UpdateError {
            error_code: error_codes::SANITY_CHECK_FAILED.into(),
            detail: format!("failed to spawn new runtime for sanity check: {e}"),
        })?;

    let output = match tokio::time::timeout(SANITY_CHECK_TIMEOUT, child.wait_with_output()).await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(UpdateError {
                error_code: error_codes::SANITY_CHECK_FAILED.into(),
                detail: format!("sanity check exec error: {e}"),
            });
        }
        Err(_) => {
            // `kill_on_drop(true)` handles the actual kill when the timeout
            // future is dropped at end of this branch — the inner
            // `wait_with_output` future owns the `Child`, and dropping it
            // drops the `Child`, which SIGKILLs the process.
            return Err(UpdateError {
                error_code: error_codes::SANITY_CHECK_FAILED.into(),
                detail: format!(
                    "sanity check timed out after {}s",
                    SANITY_CHECK_TIMEOUT.as_secs()
                ),
            });
        }
    };

    if !output.status.success() {
        return Err(UpdateError {
            error_code: error_codes::SANITY_CHECK_FAILED.into(),
            detail: format!(
                "sanity check exited with status {:?}",
                output.status.code()
            ),
        });
    }

    // Version comparison tolerates an optional leading `v`. The binary prints
    // a bare version (`gitim-runtime 0.4.2`) while the tag carries `v` —
    // accept either representation in the output.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = target_version.strip_prefix('v').unwrap_or(target_version);
    if !stdout.contains(expected) {
        return Err(UpdateError {
            error_code: error_codes::SANITY_CHECK_FAILED.into(),
            detail: format!(
                "sanity check output did not contain {expected}: stdout={:?}",
                stdout
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
    //
    // Note: no end-to-end 409 test; relies on `AtomicBool::swap` contract.
    // Reaching this branch from an integration test would require mocking
    // `$HOME` so the strict install-dir check passes — see
    // `tests/update_handler.rs::atomic_swap_supports_guard_contract` which
    // exercises the swap primitive directly instead.
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

    // --- Async phase: kill daemons → replace → fork-exec → exit. ---
    //
    // We hand the tempdir ownership into the spawned task so the extracted
    // tarball survives until `replace_binaries` has copied the files into
    // place. Any earlier drop would delete the source files out from under
    // us.
    let state_clone = state.clone();
    let job_id = job.job_id.clone();
    tokio::spawn(async move {
        run_async_phase(state_clone, job_id, tmp).await;
    });

    (StatusCode::ACCEPTED, Json(job)).into_response()
}

/// Outcome of the async phase's pre-exit work. `Done` means the child is
/// spawned and the parent is ready to exit; `Failed` means we've already
/// logged + recorded the error and released the guard. Returned by
/// [`run_async_install_and_spawn`] so callers (the handler, which exits, and
/// the integration test, which does not) share the same pre-exit code path.
///
/// Public for the `update_e2e` integration test. Production code only has
/// one caller ([`run_async_phase`]) which matches on the variant and then
/// either `exit(0)`s or logs and returns — neither branch treats the enum
/// as ergonomic API, so keep consumers at arm's length.
pub enum AsyncPhaseOutcome {
    Done { child_pid: u32 },
    Failed { detail: String },
}

/// Full async phase. Runs install + fork-exec; on success calls
/// `std::process::exit(0)` and never returns. On failure returns normally.
async fn run_async_phase(
    state: SharedRuntimeState,
    job_id: String,
    tmp: tempfile::TempDir,
) {
    let outcome = run_async_install_and_spawn(state, job_id.clone(), tmp).await;
    match outcome {
        AsyncPhaseOutcome::Done { child_pid } => {
            tracing::info!(%job_id, %child_pid, "update_and_restart: exiting parent process");
            // std::process::exit(0) here is intentional: the new runtime has just been
            // spawned and is binding its port (with retry). This process's cleanup —
            // dropping SharedRuntimeState, joining server tasks, running SIGTERM handlers —
            // is skipped on purpose; any delay risks the child failing to bind if our
            // listener is still held. kill_managed_daemons + replace_binaries + clean_old
            // all completed before this line, so no resources leak.
            std::process::exit(0);
        }
        AsyncPhaseOutcome::Failed { detail } => {
            tracing::warn!(%job_id, %detail, "update_and_restart: async phase ended in failure");
        }
    }
}

/// Everything-but-exit portion of the async phase. Kill daemons, replace
/// binaries, fork-exec the child, clean up `.old` backups. Returns the child
/// PID so the caller can decide whether to `exit(0)` (production) or just
/// observe the child (tests).
///
/// On any failure we record the detail in `state.update_last_error`, release
/// the concurrency guard, and return `Failed(detail)`; the old process stays
/// alive. On success the concurrency guard is intentionally **left set** —
/// we're about to exit, and no further handler on this process will ever run.
///
/// `tmp` must be held across `replace_binaries` so the source files aren't
/// dropped prematurely. It's released at the end of this function.
pub async fn run_async_install_and_spawn(
    state: SharedRuntimeState,
    job_id: String,
    tmp: tempfile::TempDir,
) -> AsyncPhaseOutcome {
    tracing::info!(%job_id, "update_and_restart: async phase entered");

    // Snapshot the bits the async phase needs *before* any failure branch so
    // we don't hold the mutex across blocking work.
    let (install_dir, canonical_exe, listen_port, guard) = {
        let s = state.lock().unwrap();
        let install = s
            .canonical_exe_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        (
            install,
            s.canonical_exe_path.clone(),
            s.listen_port,
            s.update_in_progress.clone(),
        )
    };

    // Step 1: kill managed daemons.
    //
    // `kill_managed_daemons` uses `std::thread::sleep` (500ms grace); wrap in
    // `spawn_blocking` so we don't park an axum worker on it.
    let state_for_kill = state.clone();
    let _ = tokio::task::spawn_blocking(move || {
        crate::workspace::kill_managed_daemons(&state_for_kill);
    })
    .await;
    tracing::info!(%job_id, "update_and_restart: managed daemons killed");

    // Step 2: atomically replace the three binaries. `keep_backup = true` so
    // the old binaries remain on disk as `.old` — defense in depth against a
    // mid-flight failure between replace and fork-exec. We clean them up at
    // the end of step 4 once the child is confirmed spawned.
    let src_dir = tmp.path().to_path_buf();
    let install_dir_for_replace = install_dir.clone();
    let replace_result = tokio::task::spawn_blocking(move || {
        gitim_updater::replace_binaries(&src_dir, &install_dir_for_replace, /* keep_backup */ true)
    })
    .await;

    let installed = match replace_result {
        Ok(Ok(installed)) => installed,
        Ok(Err(e)) => {
            let detail = format!("replace_binaries failed: {e}");
            record_async_error(&state, &guard, &job_id, detail.clone());
            return AsyncPhaseOutcome::Failed { detail };
        }
        Err(join_err) => {
            let detail = format!("replace_binaries task panicked: {join_err}");
            record_async_error(&state, &guard, &job_id, detail.clone());
            return AsyncPhaseOutcome::Failed { detail };
        }
    };
    tracing::info!(%job_id, ?installed, "update_and_restart: binaries replaced");

    // Step 3: fork-exec the new runtime with the same `--port`. We do *not*
    // wait — the child bind is what releases the port once the parent exits.
    let port_str = listen_port.to_string();
    let spawn_result = std::process::Command::new(&canonical_exe)
        .args(["--port", &port_str])
        .stdin(std::process::Stdio::null())
        // Inherit stdout/stderr so the child's logs land in the same place as
        // the parent's — daemonized runtimes already have these redirected to
        // `~/.gitim/logs/`.
        .spawn();

    let child = match spawn_result {
        Ok(c) => c,
        Err(e) => {
            let detail = format!("fork-exec new runtime failed: {e}");
            record_async_error(&state, &guard, &job_id, detail.clone());
            return AsyncPhaseOutcome::Failed { detail };
        }
    };
    let child_pid = child.id();
    // `std::process::Child::drop` is a no-op (neither waits nor kills), so
    // letting `child` go out of scope here is safe: the OS child process
    // keeps running either way. On the production path the parent's
    // subsequent `exit(0)` hands the child off to init/launchd anyway.
    drop(child);
    tracing::info!(
        %job_id,
        %child_pid,
        "update_and_restart: spawned replacement runtime"
    );

    // Step 4: cleanup `.old` backups. Best-effort — a leftover `.old` isn't
    // fatal; it just means the next update cycle will race its own rename.
    for bin in gitim_updater::BINARIES {
        let backup = backup_path(&install_dir.join(bin));
        let _ = std::fs::remove_file(backup);
    }

    // Release `tmp` explicitly. Its destructor removes the extracted tarball
    // contents now that `replace_binaries` has copied what we need.
    drop(tmp);

    AsyncPhaseOutcome::Done { child_pid }
}

/// Append `.old` to `path` in the same way `gitim-updater` does. Kept here
/// (as a small duplicate) rather than making the updater's internal helper
/// public: the updater contract is opaque on what extension it uses, and we
/// only need the literal `.old` convention to clean up after a successful
/// replace.
fn backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".old");
    std::path::PathBuf::from(s)
}

fn record_async_error(
    state: &SharedRuntimeState,
    guard: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    job_id: &str,
    detail: String,
) {
    tracing::error!(%job_id, %detail, "update_and_restart: async phase failed");
    {
        let mut s = state.lock().unwrap();
        s.update_last_error = Some(detail);
    }
    guard.store(false, Ordering::SeqCst);
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
        assert!(err.detail.contains("did not contain"));
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
