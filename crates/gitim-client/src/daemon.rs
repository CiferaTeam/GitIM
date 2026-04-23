use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::ClientError;

const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_millis(5000);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

const STALE_FILES: &[&str] = &["gitim.pid", "gitim.sock", "gitim.port", "gitim.lock"];

/// Resolve the `gitim-daemon` binary to spawn.
///
/// Prefers a sibling binary next to the currently running executable
/// (e.g. `~/.gitim/bin/gitim-daemon` when the runtime itself lives in
/// `~/.gitim/bin/`). Falls back to a bare `"gitim-daemon"` — letting the
/// OS resolve via `PATH` — if any step fails: no `current_exe`, no
/// parent dir, no sibling file, or canonicalize error.
///
/// The fallback matters for `cargo test` and dev builds where
/// `target/debug/` has no sibling daemon binary.
pub(crate) fn resolve_daemon_binary() -> PathBuf {
    resolve_daemon_binary_from(std::env::current_exe().ok())
}

/// Pure core of [`resolve_daemon_binary`] — takes the `current_exe` as a
/// parameter so tests can inject fake paths without mocking global state.
/// Canonicalize errors are absorbed into the PATH fallback (defensible:
/// a non-canonicalizable exe path is abnormal, and spawning via PATH is
/// the historical behavior).
///
/// Sibling-existence check uses `is_file()` only — not exec-bit. The
/// binary ships via `install.sh` / `replace_binaries` which both chmod
/// 0o755, so in practice a sibling `gitim-daemon` is always executable.
/// A broken file will surface as a spawn error with a useful message.
pub(crate) fn resolve_daemon_binary_from(current_exe: Option<PathBuf>) -> PathBuf {
    let fallback = PathBuf::from("gitim-daemon");
    let Some(exe) = current_exe else {
        return fallback;
    };
    let Ok(canonical) = exe.canonicalize() else {
        tracing::warn!(exe = %exe.display(), "cannot canonicalize current_exe; falling back to PATH for gitim-daemon");
        return fallback;
    };
    let Some(parent) = canonical.parent() else {
        return fallback;
    };
    let candidate = parent.join("gitim-daemon");
    if candidate.is_file() {
        candidate
    } else {
        fallback
    }
}

/// Traverse upward from `from`, return the first ancestor containing `.gitim/`.
pub fn find_repo_root(from: &Path) -> Option<PathBuf> {
    let mut dir = from.to_path_buf();
    loop {
        if dir.join(".gitim").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Check if a daemon process is alive by reading `.gitim/run/gitim.pid`.
pub fn is_daemon_running(repo_root: &Path) -> bool {
    let pid_file = repo_root.join(".gitim/run/gitim.pid");
    let contents = match std::fs::read_to_string(&pid_file) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pid: i32 = match contents.trim().parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    // signal 0 tests whether the process exists without actually sending a signal
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Ensure the daemon is running, spawning it if necessary.
///
/// Daemon stdout/stderr are discarded. For setups that need to capture daemon
/// logs (e.g. runtime-managed daemons), use
/// [`ensure_daemon_with_log`] instead.
pub fn ensure_daemon(repo_root: &Path) -> Result<(), ClientError> {
    spawn_daemon(repo_root, DaemonStdio::Null)
}

/// Ensure the daemon is running, redirecting its stdout and stderr to
/// `log_path`. Appends — existing content is preserved.
///
/// The caller is responsible for choosing a stable path (e.g. runtime names
/// each daemon's log after `<workspace>-<handler>`). The parent directory is
/// created if missing.
pub fn ensure_daemon_with_log(repo_root: &Path, log_path: &Path) -> Result<(), ClientError> {
    spawn_daemon(repo_root, DaemonStdio::LogFile(log_path.to_path_buf()))
}

enum DaemonStdio {
    Null,
    LogFile(PathBuf),
}

fn spawn_daemon(repo_root: &Path, stdio: DaemonStdio) -> Result<(), ClientError> {
    let sock_path = repo_root.join(".gitim/run/gitim.sock");

    if is_daemon_running(repo_root) {
        return wait_for_socket(&sock_path);
    }

    clean_stale_files(repo_root);

    let (stdout, stderr) = match &stdio {
        DaemonStdio::Null => (Stdio::null(), Stdio::null()),
        DaemonStdio::LogFile(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ClientError::ConnectionFailed(format!(
                        "failed to create daemon log dir {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| {
                    ClientError::ConnectionFailed(format!(
                        "failed to open daemon log {}: {e}",
                        path.display()
                    ))
                })?;
            let clone = file.try_clone().map_err(|e| {
                ClientError::ConnectionFailed(format!("failed to clone daemon log fd: {e}"))
            })?;
            (Stdio::from(file), Stdio::from(clone))
        }
    };

    let daemon_bin = resolve_daemon_binary();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            Command::new(&daemon_bin)
                .current_dir(repo_root)
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(stderr)
                .pre_exec(|| {
                    libc::setsid();
                    Ok(())
                })
                .spawn()
                .map_err(|e| {
                    ClientError::ConnectionFailed(format!("failed to spawn daemon: {e}"))
                })?;
        }
    }

    #[cfg(not(unix))]
    {
        Command::new(&daemon_bin)
            .current_dir(repo_root)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .map_err(|e| ClientError::ConnectionFailed(format!("failed to spawn daemon: {e}")))?;
    }

    wait_for_socket(&sock_path)
}

fn wait_for_socket(sock_path: &Path) -> Result<(), ClientError> {
    if sock_path.exists() {
        return Ok(());
    }
    let deadline = Instant::now() + DAEMON_STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
        if sock_path.exists() {
            return Ok(());
        }
    }
    Err(ClientError::Timeout)
}

fn clean_stale_files(repo_root: &Path) {
    let run_dir = repo_root.join(".gitim/run");
    for name in STALE_FILES {
        let _ = std::fs::remove_file(run_dir.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn find_repo_root_from_nested_subdir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".gitim")).unwrap();
        let nested = tmp.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        let found = find_repo_root(&nested);
        assert_eq!(found, Some(tmp.path().to_path_buf()));
    }

    #[test]
    fn find_repo_root_returns_none_without_gitim() {
        let tmp = TempDir::new().unwrap();
        let found = find_repo_root(tmp.path());
        assert_eq!(found, None);
    }

    /// Simulates `~/.gitim/bin/{gitim-runtime, gitim-daemon}` install layout:
    /// when current_exe sits next to a real `gitim-daemon` file, resolution
    /// returns the absolute sibling path so PATH order cannot hijack spawn.
    #[test]
    fn resolve_prefers_sibling_when_present() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let fake_runtime = bin_dir.join("gitim-runtime");
        let fake_daemon = bin_dir.join("gitim-daemon");
        fs::write(&fake_runtime, b"#!/bin/sh\n").unwrap();
        fs::write(&fake_daemon, b"#!/bin/sh\n").unwrap();

        let resolved = resolve_daemon_binary_from(Some(fake_runtime.clone()));

        // Compare against canonicalized expectation — on macOS the tempfile
        // path is under /var/folders but canonicalize resolves to
        // /private/var/folders.
        let expected = fake_daemon.canonicalize().unwrap();
        assert_eq!(resolved, expected);
    }

    /// Dev-build / cargo-test scenario: `target/debug/gitim-client-*` has
    /// no sibling daemon → resolver falls back to PATH-resolution.
    #[test]
    fn resolve_falls_back_when_no_sibling() {
        let tmp = TempDir::new().unwrap();
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let fake_runtime = bin_dir.join("gitim-runtime");
        fs::write(&fake_runtime, b"#!/bin/sh\n").unwrap();
        // Deliberately no gitim-daemon sibling.

        let resolved = resolve_daemon_binary_from(Some(fake_runtime));
        assert_eq!(resolved, PathBuf::from("gitim-daemon"));
    }

    /// `current_exe()` errored (passed as None) → PATH fallback.
    #[test]
    fn resolve_falls_back_when_current_exe_unavailable() {
        let resolved = resolve_daemon_binary_from(None);
        assert_eq!(resolved, PathBuf::from("gitim-daemon"));
    }

    #[test]
    fn resolve_falls_back_when_current_exe_does_not_exist() {
        let bogus = std::path::PathBuf::from("/definitely/does/not/exist/gitim-runtime");
        assert_eq!(
            resolve_daemon_binary_from(Some(bogus)),
            std::path::PathBuf::from("gitim-daemon"),
        );
    }
}
