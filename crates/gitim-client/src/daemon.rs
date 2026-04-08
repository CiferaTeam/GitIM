use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::ClientError;

const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_millis(5000);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

const STALE_FILES: &[&str] = &["gitim.pid", "gitim.sock", "gitim.port", "gitim.lock"];

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
/// Waits for the socket file to appear before returning.
pub fn ensure_daemon(repo_root: &Path) -> Result<(), ClientError> {
    let sock_path = repo_root.join(".gitim/run/gitim.sock");

    if is_daemon_running(repo_root) {
        return wait_for_socket(&sock_path);
    }

    clean_stale_files(repo_root);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            Command::new("gitim-daemon")
                .current_dir(repo_root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .pre_exec(|| {
                    libc::setsid();
                    Ok(())
                })
                .spawn()
                .map_err(|e| ClientError::ConnectionFailed(format!("failed to spawn daemon: {e}")))?;
        }
    }

    #[cfg(not(unix))]
    {
        Command::new("gitim-daemon")
            .current_dir(repo_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
}
