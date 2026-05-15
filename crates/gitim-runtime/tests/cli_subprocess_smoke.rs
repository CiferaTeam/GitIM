//! Binary-level subprocess smoke tests for `gitim-runtime`.
//!
//! Counterpart to T13's in-process clap parse catalog (`argv_subcommand_tests`
//! in `src/bin/runtime.rs`). These tests fork the *compiled* binary as a
//! subprocess so anything that depends on real argv handling, tracing init,
//! `std::process::exit` mapping, or stderr framing is exercised end-to-end.
//!
//! Scope is deliberately minimal — per-subcommand happy paths live in the
//! `cli_*` integration tests at handler granularity. We only add subprocess
//! coverage for behaviors that *only* manifest in a real process:
//!   - `--version` / `--help` global flags (clap built-ins),
//!   - unknown subcommand → clap exit 2,
//!   - legacy positional form rejection (verifies T1 didn't leave a back door),
//!   - subcommand-level `--help` exists,
//!   - status against an unreachable runtime maps to exit 1.
//!
//! The dual-mode E2E smoke (`test_server_mode_starts_then_status_succeeds`)
//! is marked `#[ignore]` because server-mode startup runs
//! `preflight::check_env()`, which requires version-aligned `gitim` and
//! `gitim-daemon` binaries to exist as siblings of `gitim-runtime` in
//! `target/debug/`. A plain `cargo test -p gitim-runtime --test
//! cli_subprocess_smoke` does not guarantee they're built. See the test's
//! own doc comment for the run command.
//!
//! Every test is `#[serial_test::serial]` because they mutate process-wide
//! `GITIM_LOG_DIR` (set once via `ensure_daemon_in_path`) and some also bind
//! a TCP port or override `HOME`. Cargo's default multi-thread runner would
//! race on those without serialisation.

use std::path::PathBuf;
use std::process::Command;

use serial_test::serial;
use tempfile::TempDir;

mod common;
use common::ensure_daemon_in_path;

/// Path to the compiled `gitim-runtime` binary. Cargo sets this env var for
/// integration tests in the same crate that declares the bin target.
fn runtime_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gitim-runtime"))
}

/// Build a `Command` that won't leak daemon logs into the developer's
/// `~/.gitim/logs/`. Honours the same `GITIM_LOG_DIR` env that
/// `ensure_daemon_in_path()` set on the parent process.
fn base_command() -> Command {
    ensure_daemon_in_path();
    let mut cmd = Command::new(runtime_bin());
    if let Ok(log_dir) = std::env::var("GITIM_LOG_DIR") {
        cmd.env("GITIM_LOG_DIR", log_dir);
    }
    cmd
}

// ---------------------------------------------------------------------------
// --version / --help
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_version_flag_prints_and_exits_zero() {
    let output = base_command()
        .arg("--version")
        .output()
        .expect("spawn runtime");
    assert!(
        output.status.success(),
        "expected exit 0, got status {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("gitim-runtime "),
        "version stdout should start with 'gitim-runtime ', got: {stdout:?}",
    );
    // The remainder must contain at least one digit so a typo like
    // `gitim-runtime  ` won't slip through.
    assert!(
        stdout.chars().any(|c| c.is_ascii_digit()),
        "version stdout must contain a digit, got: {stdout:?}",
    );
}

#[test]
#[serial]
fn test_help_flag_lists_subcommands() {
    let output = base_command()
        .arg("--help")
        .output()
        .expect("spawn runtime");
    assert!(
        output.status.success(),
        "expected exit 0, got status {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // All 8 documented subcommands must be listed. If a future PR adds a
    // subcommand it should also extend this expectation explicitly — clap's
    // auto-help is the user-facing contract.
    for sub in [
        "status",
        "runtime-id",
        "workspaces",
        "list-agents",
        "add-agent",
        "burn-agent",
        "update-agent",
        "preflight",
    ] {
        assert!(
            stdout.contains(sub),
            "--help output is missing subcommand `{sub}`. Full output:\n{stdout}",
        );
    }
}

// ---------------------------------------------------------------------------
// Unknown / malformed argv → clap exit 2
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_unknown_subcommand_exits_nonzero() {
    let output = base_command()
        .arg("fly-to-mars")
        .output()
        .expect("spawn runtime");
    assert!(
        !output.status.success(),
        "expected non-zero exit for unknown subcommand, got success",
    );
    // Spec §4 maps argv parse errors to exit 1 (CLI internal error).
    // clap's own default is 2; we override in `main` so the agent's
    // exit-code mapper sees a uniform 1 = CLI / argv class.
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        stderr.contains("unrecognized subcommand"),
        "stderr should mention 'unrecognized subcommand', got: {stderr:?}",
    );
}

#[test]
#[serial]
fn test_legacy_positional_form_rejected() {
    // The pre-CLI form was `gitim-runtime <url> <handler> <name>`. T1
    // retired it; this test pins the end-to-end behaviour so a regression
    // doesn't silently re-introduce the positional surface. Exit 1 per
    // spec §4 (argv / parse error class).
    let output = base_command()
        .args(["https://github.com/o/r", "handler", "displayname"])
        .output()
        .expect("spawn runtime");
    assert!(
        !output.status.success(),
        "legacy positional form must be rejected, got success",
    );
    assert_eq!(output.status.code(), Some(1));
}

// ---------------------------------------------------------------------------
// status against an unreachable runtime
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_status_against_nonexistent_runtime() {
    // Fresh HOME so `read()` returns a default `UserConfig` with
    // `listen_port: None`. We then force `GITIM_RUNTIME_PORT` to a high port
    // nothing should be listening on, so the connect attempt hits
    // connection-refused. Transport errors map to CLI exit 1.
    let tmp = TempDir::new().expect("tempdir for HOME");

    let output = base_command()
        .env("HOME", tmp.path())
        .env("GITIM_RUNTIME_PORT", "19999")
        .arg("status")
        .output()
        .expect("spawn runtime");
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit 1 for transport error; stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    // The ErrorResponse envelope flows through stderr; the inner message
    // comes from reqwest's connection-refused string. We assert on
    // 'transport' (our own classification) rather than a libc-specific
    // phrasing so the test is portable across platforms.
    assert!(
        stderr.contains("transport") || stderr.contains("connection"),
        "stderr should indicate transport / connection failure, got: {stderr:?}",
    );
}

// ---------------------------------------------------------------------------
// subcommand-level --help
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn test_subcommand_help_exists() {
    let output = base_command()
        .args(["add-agent", "--help"])
        .output()
        .expect("spawn runtime");
    assert!(
        output.status.success(),
        "add-agent --help should exit 0, got status {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in ["--handler", "--display-name", "--provider"] {
        assert!(
            stdout.contains(flag),
            "add-agent --help is missing `{flag}`. Full output:\n{stdout}",
        );
    }
}

// ---------------------------------------------------------------------------
// Server mode end-to-end
// ---------------------------------------------------------------------------

/// RAII guard that ensures the spawned server is killed even if the test
/// panics mid-way. `std::process::Child` doesn't kill on drop by default — a
/// leaked server would leave a port held and `~/.gitim/runtime.pid` written
/// for the next test run.
struct ServerGuard(Option<std::process::Child>);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// End-to-end smoke for the dual-mode binary: server boots, persists a
/// listen_port hint, then the CLI variant of the same binary can discover
/// and query it.
///
/// Marked `#[ignore]` because `run_server()` calls
/// `gitim_runtime::preflight::check_env()` on startup, which demands
/// version-aligned `gitim` and `gitim-daemon` binaries (siblings of
/// `gitim-runtime` in `target/debug/`). Plain `cargo test -p gitim-runtime
/// --test cli_subprocess_smoke` does *not* guarantee those neighbors exist,
/// so we don't enforce the test by default.
///
/// To run it explicitly:
///
/// ```text
/// cargo build -p gitim-cli --bin gitim -p gitim-daemon --bin gitim-daemon
/// cargo test -p gitim-runtime --test cli_subprocess_smoke -- --ignored \
///     test_server_mode_starts_then_status_succeeds
/// ```
#[test]
#[serial]
#[ignore = "requires sibling gitim + gitim-daemon binaries; see test doc"]
fn test_server_mode_starts_then_status_succeeds() {
    let tmp = TempDir::new().expect("tempdir for HOME");

    // Reserve an ephemeral port by binding then dropping. There's a small
    // race window where another process could grab the same port before our
    // server rebinds — acceptable for a smoke test, and the same trick the
    // sibling cli_status test uses.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let child = base_command()
        .env("HOME", tmp.path())
        .args(["--port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn server");
    let mut guard = ServerGuard(Some(child));

    // Wait until the port accepts a TCP connection or timeout. `TcpStream`
    // is the cheapest probe and avoids pulling reqwest blocking into the
    // dev-deps just for one test.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            std::time::Duration::from_millis(200),
        )
        .is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        ready,
        "server didn't accept connections on port {port} within 10s",
    );

    // Run `gitim-runtime status` as a separate subprocess pointed at the
    // same port and HOME. This exercises the full CLI dispatch path
    // (resolve_base_url → reqwest → cmd_status → println!).
    let status_out = base_command()
        .env("HOME", tmp.path())
        .env("GITIM_RUNTIME_PORT", port.to_string())
        .arg("status")
        .output()
        .expect("spawn status");
    let status_stderr = String::from_utf8_lossy(&status_out.stderr).to_string();
    assert_eq!(
        status_out.status.code(),
        Some(0),
        "status against live server should exit 0; stderr: {status_stderr}",
    );

    let stdout = String::from_utf8_lossy(&status_out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("status stdout is JSON");
    assert!(
        parsed.get("runtime_id").is_some(),
        "status JSON must carry runtime_id, got: {parsed}",
    );

    // Tear down explicitly so a failed kill surfaces in test output rather
    // than hiding in Drop.
    if let Some(mut child) = guard.0.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}
