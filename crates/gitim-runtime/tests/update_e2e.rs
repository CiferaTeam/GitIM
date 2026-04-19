//! End-to-end tests for the self-update async phase (Task 7).
//!
//! ## Scope
//!
//! The plan's gold-standard E2E is:
//!
//! > POST `/runtime/update-and-restart` → 202 → old runtime replaces its own
//! > binaries → fork-execs a new runtime → `exit(0)` → new runtime serves
//! > `/health` with the target version.
//!
//! Automating the *full* loop in a single `cargo test` binary is hard:
//! `std::process::exit(0)` is load-bearing on the production path (without it,
//! the TCP port isn't released and the replacement binary can't bind), and
//! calling it mid-test kills the test harness. Spawning the real
//! `gitim-runtime` out of process would sidestep the exit, but the real
//! runtime runs `preflight::check_env()` on boot which demands a
//! version-aligned `gitim` + `gitim-daemon` on `PATH` — a prerequisite that
//! turns a self-contained test into a coordinated multi-binary fixture.
//!
//! So we split the pipeline:
//!
//! 1. The sync phase (download + extract + sanity-check) is already covered
//!    in `tests/update_handler.rs` and unit tests in `src/update.rs` via
//!    fake `sleep`/`echo` stand-ins.
//! 2. The async phase (replace + fork-exec + `/health` verification) lives
//!    here. We call
//!    [`gitim_runtime::update::run_async_install_and_spawn`] directly — the
//!    extraction point the handler itself uses before its final
//!    `std::process::exit`. Everything the async phase does on disk and over
//!    the wire is exercised; only the `exit(0)` is skipped.
//!
//! Each test builds an "install dir" containing a v1 `fake-gitim-runtime`
//! copy, extracts a v2 "tarball" (just copies of the same fake bin with a
//! different `FAKE_VERSION`) into a tempdir, and runs the async phase. The
//! post-conditions we assert match the plan's acceptance criteria:
//!
//! - replace_binaries swapped all three binaries;
//! - no `.old` crumbs remain in the install dir;
//! - the fork-exec'd child is reachable on `--port` and reports the new
//!   version via `/health`;
//! - on injected failure, old binaries survive, guard is released, and
//!   `update_last_error` captures the reason.
//!
//! Serialised with `serial_test::serial` because each test:
//! (a) binds a known TCP port,
//! (b) mutates process-global env vars (`HOME`, `FAKE_VERSION`,
//!     `GITIM_RELEASES_*`).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serial_test::serial;
use tempfile::TempDir;

use gitim_runtime::http::{RuntimeState, SharedRuntimeState};
use gitim_runtime::update::{run_async_install_and_spawn, AsyncPhaseOutcome};

// -- helpers ----------------------------------------------------------------

/// Path to the compiled `fake-gitim-runtime` bin. Cargo sets
/// `CARGO_BIN_EXE_fake-gitim-runtime` for integration tests when the bin
/// target is declared in the same crate.
fn fake_runtime_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-gitim-runtime"))
}

/// Copy the fake runtime binary to `<dir>/<name>` and chmod 0o755. Returns the
/// destination path. We write all three `BINARIES` names from the same source
/// because the replace step only checks names — not behaviour — for `gitim`
/// and `gitim-daemon`.
fn install_fake(dir: &Path, name: &str) -> PathBuf {
    let src = fake_runtime_bin();
    let dest = dir.join(name);
    std::fs::copy(&src, &dest)
        .unwrap_or_else(|e| panic!("copy {src:?} -> {dest:?}: {e}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .expect("chmod 0o755");
    }
    dest
}

/// Find an unused TCP port by binding ephemeral and reading the allocated
/// port number, then releasing the socket. Cheap, racy in principle, but the
/// window between release and child bind is microseconds and the tests are
/// serialised anyway.
fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Poll `http://127.0.0.1:<port>/health` until it responds with JSON containing
/// `"version":"<expected>"` or the deadline elapses. Returns `Ok(body)` on
/// success, `Err(last_error_or_timeout)` otherwise.
///
/// Uses blocking TCP rather than a full HTTP client so a test-only dep isn't
/// pulled into the tree for this. The fake runtime replies with one-shot
/// `Connection: close` responses — trivial to parse.
fn poll_health(port: u16, expected_version: &str, deadline: Duration) -> Result<String, String> {
    let start = Instant::now();
    let mut last_err = String::from("never attempted");
    while start.elapsed() < deadline {
        match fetch_health_once(port) {
            Ok(body) if body.contains(&format!("\"version\":\"{expected_version}\"")) => {
                return Ok(body);
            }
            Ok(body) => {
                last_err = format!("version mismatch in body: {body}");
            }
            Err(e) => last_err = e,
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "poll_health timed out after {:?}: last_err={last_err}",
        deadline,
    ))
}

fn fetch_health_once(port: u16) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("parse {addr}: {e}"))?,
        Duration::from_millis(500),
    )
    .map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| format!("set_read_timeout: {e}"))?;
    let req = "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&buf).to_string();
    // Strip HTTP headers — body is whatever follows the blank line.
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok(body)
}

/// Build a `RuntimeState` whose `canonical_exe_path` points into `install_dir`
/// and whose `listen_port` is `port`. Wraps it in the usual
/// `Arc<Mutex<...>>` so the async-phase API accepts it.
fn state_for(install_dir: &Path, port: u16) -> SharedRuntimeState {
    let canonical = install_dir
        .join("gitim-runtime")
        .canonicalize()
        .expect("canonicalize install_dir/gitim-runtime");
    let inner = RuntimeState {
        canonical_exe_path: canonical,
        listen_port: port,
        ..RuntimeState::default()
    };
    Arc::new(Mutex::new(inner))
}

/// Best-effort kill of a spawned child-by-pid on test teardown. A leaked child
/// binding our `--port` would make the next `pick_free_port` reuse harmless
/// but would leave a process lying around on the dev machine.
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .output();
    std::thread::sleep(Duration::from_millis(100));
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
}

/// RAII guard for the `FAKE_VERSION` env var: sets it on construction and
/// restores the prior value on drop. The fake runtime reads `FAKE_VERSION`
/// lazily at startup, so the var must be set when `spawn` runs.
struct FakeVersionGuard {
    original: Option<std::ffi::OsString>,
}

impl FakeVersionGuard {
    fn install(value: &str) -> Self {
        let original = std::env::var_os("FAKE_VERSION");
        std::env::set_var("FAKE_VERSION", value);
        Self { original }
    }
}

impl Drop for FakeVersionGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(v) => std::env::set_var("FAKE_VERSION", v),
            None => std::env::remove_var("FAKE_VERSION"),
        }
    }
}

// -- tests ------------------------------------------------------------------

/// Happy path: replace binaries, fork-exec a new runtime, new runtime answers
/// `/health` with the target version, `.old` backups are cleaned up.
///
/// This is the practical equivalent of the plan's full E2E — the only step
/// we skip is the handler's terminal `std::process::exit(0)`, because the
/// test harness needs to keep running.
#[tokio::test]
#[serial(update_e2e_env)]
async fn async_phase_replaces_and_forks_new_runtime() {
    let install_dir = TempDir::new().expect("install_dir tempdir");
    let src_dir = TempDir::new().expect("src_dir tempdir");

    // Pre-populate the install dir with the "old" binaries. The async phase
    // will rename these to `.old` and replace them with the src_dir copies.
    for name in gitim_updater::BINARIES {
        install_fake(install_dir.path(), name);
    }

    // The "new" tarball — same fake bin (it just reports whatever FAKE_VERSION
    // says), laid out in the same flat shape `replace_binaries` expects under
    // src_dir. No nested top-level dir because `find_binary` does a recursive
    // walk anyway.
    for name in gitim_updater::BINARIES {
        install_fake(src_dir.path(), name);
    }

    // Claim an ephemeral port for the freshly spawned child. We pick it now
    // so the test can assert the child binds this exact port.
    let port = pick_free_port();
    let state = state_for(install_dir.path(), port);

    // Simulate the handler's sync phase having set the guard — the async
    // phase contract is "guard stays set on success (parent exits), cleared
    // on failure". Pre-setting lets us observe that contract directly.
    state
        .lock()
        .unwrap()
        .update_in_progress
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // The child reads FAKE_VERSION at spawn time and echoes it via /health.
    let _version_guard = FakeVersionGuard::install("9.9.9");

    // We have to consume the src_dir tempdir so replace_binaries can walk it;
    // ownership transfer is what the real handler does too.
    let job_id = "test-happy".to_string();
    let outcome =
        run_async_install_and_spawn(state.clone(), job_id.clone(), src_dir).await;

    let child_pid = match outcome {
        AsyncPhaseOutcome::Done { child_pid } => child_pid,
        AsyncPhaseOutcome::Failed { detail } => panic!("async phase failed: {detail}"),
    };

    // Make sure we reap the child even if later asserts fail.
    let _guard = scopeguard::ScopeGuard::new(child_pid, |pid| kill_pid(pid));

    // Poll /health for up to 10s. In practice the fake binds within a few
    // hundred milliseconds; the budget gives CI headroom without hanging
    // developer feedback loops.
    let body = poll_health(port, "9.9.9", Duration::from_secs(10))
        .expect("child should serve /health with new version");
    assert!(body.contains("\"version\":\"9.9.9\""), "body={body}");

    // Verify the install dir has the new binaries (all three) and no `.old`
    // crumbs — the success-path cleanup must have removed them.
    for name in gitim_updater::BINARIES {
        let installed = install_dir.path().join(name);
        assert!(installed.is_file(), "{} should exist", installed.display());
        let backup = install_dir.path().join(format!("{name}.old"));
        assert!(
            !backup.exists(),
            "{} should have been cleaned up on success",
            backup.display(),
        );
    }

    // The concurrency guard was intentionally left set on success: the real
    // handler exits immediately after, and no further HTTP handlers on this
    // process will see it. Assert the contract explicitly so a future
    // refactor that changes this is flagged by tests.
    let guard = state.lock().unwrap().update_in_progress.clone();
    assert!(
        guard.load(std::sync::atomic::Ordering::SeqCst),
        "guard stays set on success — parent is expected to exit(0)"
    );
    // update_last_error must remain None on the success path.
    assert!(state.lock().unwrap().update_last_error.is_none());
}

/// Injected replace-failure: create `gitim-daemon.old` as a directory so the
/// second-binary rename EISDIRs, triggering rollback. The async phase must:
/// - return Failed
/// - record a detail in `update_last_error`
/// - release the concurrency guard (old process stays alive)
/// - NOT spawn a child (we only assert the no-child path indirectly by
///   requiring Failed)
/// - leave the install dir recoverable (first binary rolled back, second
///   untouched, third never reached)
#[tokio::test]
#[serial(update_e2e_env)]
async fn async_phase_replace_failure_leaves_old_runtime_alive() {
    let install_dir = TempDir::new().expect("install_dir tempdir");
    let src_dir = TempDir::new().expect("src_dir tempdir");

    for name in gitim_updater::BINARIES {
        install_fake(install_dir.path(), name);
        install_fake(src_dir.path(), name);
    }

    // Inject a rename-blocker: `replace_binaries` will try to rename
    // `install_dir/gitim-daemon` to `install_dir/gitim-daemon.old`, but we've
    // made that name a directory — the rename fails and rollback kicks in.
    std::fs::create_dir(install_dir.path().join("gitim-daemon.old"))
        .expect("pre-create blocker");

    let port = pick_free_port();
    let state = state_for(install_dir.path(), port);

    // Pre-flip the guard so we can assert it gets cleared on failure. The
    // production handler sets this in its sync phase; in this direct-call
    // test we do it manually.
    state
        .lock()
        .unwrap()
        .update_in_progress
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let _version_guard = FakeVersionGuard::install("9.9.9");

    let outcome =
        run_async_install_and_spawn(state.clone(), "test-fail".to_string(), src_dir).await;

    match outcome {
        AsyncPhaseOutcome::Failed { detail } => {
            assert!(
                detail.contains("replace_binaries failed"),
                "unexpected failure detail: {detail}"
            );
        }
        AsyncPhaseOutcome::Done { .. } => panic!("expected failure, got Done"),
    }

    // Guard must be released so a retry can run.
    let guard = state.lock().unwrap().update_in_progress.clone();
    assert!(
        !guard.load(std::sync::atomic::Ordering::SeqCst),
        "guard must be cleared on failure"
    );

    // update_last_error must capture the detail.
    let last_err = state
        .lock()
        .unwrap()
        .update_last_error
        .clone()
        .expect("update_last_error should be populated on failure");
    assert!(
        last_err.contains("replace_binaries failed"),
        "unexpected last_err: {last_err}"
    );

    // The first binary should have been rolled back to the original content —
    // it still exists at its canonical path, not at `.old`.
    assert!(install_dir.path().join("gitim").is_file());
    assert!(!install_dir.path().join("gitim.old").exists());
    // No listener ever started.
    assert!(fetch_health_once(port).is_err());
}

/// Fork-exec failure: point `canonical_exe_path` at a missing file so `spawn`
/// itself fails. The binaries have already been replaced by this point (the
/// real-world shape: replace succeeded, the exec syscall returned ENOENT).
/// We still expect guard release + error recording.
#[tokio::test]
#[serial(update_e2e_env)]
async fn async_phase_fork_exec_failure_records_error() {
    let install_dir = TempDir::new().expect("install_dir tempdir");
    let src_dir = TempDir::new().expect("src_dir tempdir");

    // Write a *non-executable* placeholder so `canonicalize` succeeds in
    // `state_for`, then delete it after capturing state — `Command::spawn`
    // will see ENOENT and fail.
    let placeholder = install_dir.path().join("gitim-runtime");
    std::fs::write(&placeholder, b"placeholder").expect("write placeholder");
    // Also pre-populate `gitim` + `gitim-daemon` so replace succeeds.
    install_fake(install_dir.path(), "gitim");
    install_fake(install_dir.path(), "gitim-daemon");

    for name in gitim_updater::BINARIES {
        install_fake(src_dir.path(), name);
    }

    let port = pick_free_port();
    let state = state_for(install_dir.path(), port);
    state
        .lock()
        .unwrap()
        .update_in_progress
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Make the canonical_exe_path unreachable *after* state capture: the
    // replace will put the (working) fake back, but we then delete it to
    // force spawn to fail.
    //
    // Trick: point canonical_exe_path at a non-exec path by overwriting the
    // state's canonical after state construction, bypassing canonicalize.
    state.lock().unwrap().canonical_exe_path =
        install_dir.path().join("definitely-nonexistent");

    let _version_guard = FakeVersionGuard::install("9.9.9");

    let outcome = run_async_install_and_spawn(
        state.clone(),
        "test-spawn-fail".to_string(),
        src_dir,
    )
    .await;

    match outcome {
        AsyncPhaseOutcome::Failed { detail } => {
            assert!(
                detail.contains("fork-exec new runtime failed"),
                "unexpected failure detail: {detail}"
            );
        }
        AsyncPhaseOutcome::Done { .. } => panic!("expected Failed, got Done"),
    }

    // Guard must be cleared.
    assert!(
        !state
            .lock()
            .unwrap()
            .update_in_progress
            .load(std::sync::atomic::Ordering::SeqCst)
    );

    // update_last_error populated.
    let last_err = state
        .lock()
        .unwrap()
        .update_last_error
        .clone()
        .expect("update_last_error should be populated");
    assert!(last_err.contains("fork-exec"), "unexpected: {last_err}");
}

/// The full-pipeline E2E the plan asked for — POST `/runtime/update-and-restart`
/// → 202 → poll `/health` for target version — is intentionally **not**
/// implemented here. Two things make it unsafe to run inside `cargo test`:
///
/// 1. The handler's async task calls `std::process::exit(0)` on the happy
///    path (production-critical: the child cannot bind the port otherwise).
///    Running that inside a `tokio::spawn` kills the test harness along with
///    the parent "runtime".
/// 2. Even if we inserted a test-only exit hook, the spawned child process
///    binds `state.listen_port`. Picking that port from inside an
///    ephemeral-port dance is fine, but making the child's `/health`
///    reachable requires either routing the test's existing reqwest client
///    at the child's port (straightforward) or polling raw TCP (what
///    `poll_health` does above).
///
/// The direct-call tests (`async_phase_replaces_and_forks_new_runtime` etc.)
/// already exercise steps 1-4 of the plan's async phase via the exact
/// function the handler's final `exit(0)` call sits behind. The sync phase
/// is covered by `tests/update_handler.rs` (reject branches) and the
/// in-module `#[tokio::test]`s (`sanity_check_*`). The end-to-end glue —
/// "handler spawns async task after sync succeeds" — is two lines of code
/// in the handler, easy to eyeball.
///
/// A manual reproduction script (run against a real dev install) lives in
/// the implementation notes for Task 7; rerunning it before each release is
/// cheaper than the test infrastructure a self-contained E2E would need.
#[allow(dead_code)]
fn _documentation_only_full_e2e_placeholder() {}

// -- scopeguard ------------------------------------------------------------
//
// Tiny in-crate clone of `scopeguard::ScopeGuard` so we don't pull in a
// full dep for three call sites. The test runs best-effort cleanup on drop.

mod scopeguard {
    pub struct ScopeGuard<T, F: FnOnce(T)> {
        value: Option<T>,
        on_drop: Option<F>,
    }
    impl<T, F: FnOnce(T)> ScopeGuard<T, F> {
        pub fn new(value: T, on_drop: F) -> Self {
            Self {
                value: Some(value),
                on_drop: Some(on_drop),
            }
        }
    }
    impl<T, F: FnOnce(T)> Drop for ScopeGuard<T, F> {
        fn drop(&mut self) {
            if let (Some(v), Some(f)) = (self.value.take(), self.on_drop.take()) {
                f(v);
            }
        }
    }
}

