use fs2::FileExt;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::ClientError;

const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_RESPAWN_BACKOFF: Duration = Duration::from_secs(2);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonStartupState {
    Ready,
    WaitForOwner,
    SpawnCandidate,
}

static DAEMON_REAPER: OnceLock<Result<mpsc::Sender<Child>, String>> = OnceLock::new();

fn daemon_reaper_sender() -> Result<mpsc::Sender<Child>, ClientError> {
    match DAEMON_REAPER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("gitim-daemon-reaper".to_string())
            .spawn(move || daemon_reaper_loop(receiver))
            .map(|_| sender)
            .map_err(|error| error.to_string())
    }) {
        Ok(sender) => Ok(sender.clone()),
        Err(error) => Err(ClientError::ConnectionFailed(format!(
            "failed to start daemon reaper: {error}"
        ))),
    }
}

fn daemon_reaper_loop(receiver: mpsc::Receiver<Child>) {
    let mut children = Vec::new();
    let mut disconnected = false;

    loop {
        if disconnected {
            thread::sleep(POLL_INTERVAL);
        } else {
            match receiver.recv_timeout(POLL_INTERVAL) {
                Ok(child) => children.push(child),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
            }
            while let Ok(child) = receiver.try_recv() {
                children.push(child);
            }
        }

        let mut index = 0;
        while index < children.len() {
            match children[index].try_wait() {
                Ok(Some(status)) => {
                    tracing::debug!(pid = children[index].id(), %status, "reaped daemon child");
                    let mut child = children.swap_remove(index);
                    let _ = child.wait();
                }
                Ok(None) => index += 1,
                Err(error) => {
                    tracing::warn!(
                        pid = children[index].id(),
                        %error,
                        "failed to poll daemon child from reaper"
                    );
                    index += 1;
                }
            }
        }

        if disconnected && children.is_empty() {
            return;
        }
    }
}

struct RespawnSchedule {
    delay: Duration,
    next_spawn_at: Instant,
}

impl RespawnSchedule {
    fn new(now: Instant) -> Self {
        Self {
            delay: POLL_INTERVAL,
            next_spawn_at: now,
        }
    }

    fn candidate_failed(&mut self, now: Instant) {
        self.next_spawn_at = now + self.delay;
        self.delay = (self.delay * 2).min(MAX_RESPAWN_BACKOFF);
    }

    fn can_spawn(&self, now: Instant) -> bool {
        now >= self.next_spawn_at
    }
}

fn update_respawn_schedule(
    schedule: &mut RespawnSchedule,
    state: DaemonStartupState,
    candidate_exited: bool,
    now: Instant,
) {
    if state != DaemonStartupState::Ready && candidate_exited {
        schedule.candidate_failed(now);
    }
}

struct DaemonCandidate {
    child: Option<Child>,
    reaper: mpsc::Sender<Child>,
}

impl DaemonCandidate {
    fn new(reaper: mpsc::Sender<Child>) -> Self {
        Self {
            child: None,
            reaper,
        }
    }

    fn is_empty(&self) -> bool {
        self.child.is_none()
    }

    fn replace(&mut self, child: Child) {
        self.child = Some(child);
    }

    fn reap_if_exited(&mut self) -> Result<bool, ClientError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(false);
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::debug!(pid = child.id(), %status, "daemon candidate exited");
                self.child = None;
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(error) => Err(ClientError::ConnectionFailed(format!(
                "failed to poll daemon candidate: {error}"
            ))),
        }
    }
}

impl Drop for DaemonCandidate {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }

        let pid = child.id();
        if let Err(error) = self.reaper.send(child) {
            tracing::error!(pid, "daemon reaper stopped unexpectedly");
            let mut child = error.0;
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_daemon(repo_root: &Path, stdio: DaemonStdio) -> Result<(), ClientError> {
    let deadline = Instant::now() + DAEMON_STARTUP_TIMEOUT;
    wait_for_daemon_until(repo_root, deadline, || {
        spawn_daemon_candidate(repo_root, &stdio)
    })
}

fn wait_for_daemon_until<F>(
    repo_root: &Path,
    deadline: Instant,
    mut spawn_candidate: F,
) -> Result<(), ClientError>
where
    F: FnMut() -> Result<Child, ClientError>,
{
    let mut candidate = DaemonCandidate::new(daemon_reaper_sender()?);
    let mut respawn_schedule = RespawnSchedule::new(Instant::now());

    loop {
        if Instant::now() >= deadline {
            return Err(ClientError::Timeout);
        }

        let candidate_exited = candidate.reap_if_exited()?;
        let now = Instant::now();

        let startup_state = daemon_startup_state(repo_root)?;
        update_respawn_schedule(&mut respawn_schedule, startup_state, candidate_exited, now);
        match startup_state {
            DaemonStartupState::Ready => return Ok(()),
            DaemonStartupState::WaitForOwner => {}
            DaemonStartupState::SpawnCandidate => {
                if candidate.is_empty() && respawn_schedule.can_spawn(now) {
                    candidate.replace(spawn_candidate()?);
                }
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ClientError::Timeout);
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

fn spawn_daemon_candidate(repo_root: &Path, stdio: &DaemonStdio) -> Result<Child, ClientError> {
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
            let file = OpenOptions::new()
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
    let mut command = Command::new(&daemon_bin);
    command
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    command
        .spawn()
        .map_err(|error| ClientError::ConnectionFailed(format!("failed to spawn daemon: {error}")))
}

fn daemon_startup_state(repo_root: &Path) -> Result<DaemonStartupState, ClientError> {
    let run_dir = repo_root.join(".gitim/run");
    if socket_is_ready(&run_dir.join("gitim.sock"))? {
        return Ok(DaemonStartupState::Ready);
    }

    if daemon_lock_is_held(&run_dir.join("gitim.lock"))? {
        Ok(DaemonStartupState::WaitForOwner)
    } else {
        Ok(DaemonStartupState::SpawnCandidate)
    }
}

fn daemon_lock_is_held(lock_path: &Path) -> Result<bool, ClientError> {
    if let Some(run_dir) = lock_path.parent() {
        std::fs::create_dir_all(run_dir).map_err(|error| {
            ClientError::ConnectionFailed(format!(
                "failed to create daemon run dir {}: {error}",
                run_dir.display()
            ))
        })?;
    }

    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| {
            ClientError::ConnectionFailed(format!(
                "failed to open daemon lock {}: {error}",
                lock_path.display()
            ))
        })?;

    match FileExt::try_lock_exclusive(&lock_file) {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(true),
        Err(error) => Err(ClientError::ConnectionFailed(format!(
            "failed to probe daemon lock {}: {error}",
            lock_path.display()
        ))),
    }
}

fn socket_is_ready(sock_path: &Path) -> Result<bool, ClientError> {
    // Poll until the daemon is actually accepting connections, not just until
    // the socket file appears. A daemon that creates its socket during startup
    // but crashes before its accept loop is ready will leave the file on disk
    // while `connect` returns `ConnectionRefused` — checking only file
    // existence would cause `ensure_daemon_running` to claim success and
    // trigger an immediate tight-loop in the caller.
    match std::os::unix::net::UnixStream::connect(sock_path) {
        Ok(_) => Ok(true),
        Err(error)
            if error.kind() == ErrorKind::NotFound
                || error.kind() == ErrorKind::ConnectionRefused =>
        {
            Ok(false)
        }
        Err(error) => Err(ClientError::ConnectionFailed(format!(
            "socket readiness check failed: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;
    use std::cell::Cell;
    use std::fs;
    use std::fs::OpenOptions;
    use std::os::unix::net::UnixListener;
    use tempfile::TempDir;

    fn create_run_dir(temp_dir: &TempDir) -> PathBuf {
        let run_dir = temp_dir.path().join(".gitim/run");
        fs::create_dir_all(&run_dir).unwrap();
        run_dir
    }

    #[test]
    fn startup_state_waits_while_daemon_lock_is_held() {
        let temp_dir = TempDir::new().unwrap();
        let run_dir = create_run_dir(&temp_dir);
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(run_dir.join("gitim.lock"))
            .unwrap();
        lock_file.lock_exclusive().unwrap();

        assert_eq!(
            daemon_startup_state(temp_dir.path()).unwrap(),
            DaemonStartupState::WaitForOwner
        );
    }

    #[test]
    fn stale_live_pid_does_not_gate_spawn_decision() {
        let temp_dir = TempDir::new().unwrap();
        let run_dir = create_run_dir(&temp_dir);
        fs::write(run_dir.join("gitim.pid"), std::process::id().to_string()).unwrap();
        assert!(is_daemon_running(temp_dir.path()));

        assert_eq!(
            daemon_startup_state(temp_dir.path()).unwrap(),
            DaemonStartupState::SpawnCandidate
        );
    }

    #[test]
    fn connectable_socket_is_ready_signal() {
        let temp_dir = TempDir::new().unwrap();
        let run_dir = create_run_dir(&temp_dir);
        let _listener = UnixListener::bind(run_dir.join("gitim.sock")).unwrap();

        assert_eq!(
            daemon_startup_state(temp_dir.path()).unwrap(),
            DaemonStartupState::Ready
        );
    }

    #[test]
    fn startup_loop_reaps_exited_candidate_before_retrying() {
        let temp_dir = TempDir::new().unwrap();
        let run_dir = create_run_dir(&temp_dir);
        let marker = temp_dir.path().join("candidate-exited");
        let attempts = Cell::new(0);
        let mut _listener = None;

        wait_for_daemon_until(
            temp_dir.path(),
            Instant::now() + Duration::from_secs(2),
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Command::new("sh")
                        .args(["-c", r#"sleep 0.2; touch "$1""#, "gitim-client-test"])
                        .arg(&marker)
                        .spawn()
                        .map_err(|error| ClientError::ConnectionFailed(error.to_string()))
                } else {
                    assert!(marker.exists(), "retry started before prior child exited");
                    _listener = Some(UnixListener::bind(run_dir.join("gitim.sock")).unwrap());
                    Command::new("true")
                        .spawn()
                        .map_err(|error| ClientError::ConnectionFailed(error.to_string()))
                }
            },
        )
        .unwrap();

        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn startup_loop_background_reaps_candidate_when_socket_becomes_ready() {
        let temp_dir = TempDir::new().unwrap();
        let run_dir = create_run_dir(&temp_dir);
        let socket_path = run_dir.join("gitim.sock");
        let listener_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let _listener = UnixListener::bind(socket_path).unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });
        let candidate_pid = Cell::new(None);

        wait_for_daemon_until(
            temp_dir.path(),
            Instant::now() + Duration::from_secs(2),
            || {
                let child = Command::new("sh")
                    .args(["-c", "sleep 0.2"])
                    .spawn()
                    .map_err(|error| ClientError::ConnectionFailed(error.to_string()))?;
                candidate_pid.set(Some(child.id()));
                Ok(child)
            },
        )
        .unwrap();
        listener_thread.join().unwrap();

        let pid = candidate_pid.get().expect("candidate spawned");
        let deadline = Instant::now() + Duration::from_secs(1);
        while unsafe { libc::kill(pid as i32, 0) == 0 } && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(pid as i32, 0) },
            0,
            "candidate should be reaped after it exits"
        );
    }

    #[test]
    fn permanent_startup_failure_uses_bounded_backoff() {
        let temp_dir = TempDir::new().unwrap();
        let attempts = Cell::new(0);

        let result = wait_for_daemon_until(
            temp_dir.path(),
            Instant::now() + Duration::from_millis(750),
            || {
                attempts.set(attempts.get() + 1);
                Command::new("false")
                    .spawn()
                    .map_err(|error| ClientError::ConnectionFailed(error.to_string()))
            },
        );

        assert!(matches!(result, Err(ClientError::Timeout)));
        assert!(
            attempts.get() <= 4,
            "permanent failure retried too aggressively: {} attempts",
            attempts.get()
        );
    }

    #[test]
    fn lock_loser_backs_off_while_waiting_for_owner() {
        let now = Instant::now();
        let mut schedule = RespawnSchedule::new(now);

        update_respawn_schedule(&mut schedule, DaemonStartupState::WaitForOwner, true, now);

        assert!(!schedule.can_spawn(now));
        assert_eq!(schedule.delay, POLL_INTERVAL * 2);
        let next_spawn_at = schedule.next_spawn_at;
        let delay = schedule.delay;

        update_respawn_schedule(
            &mut schedule,
            DaemonStartupState::WaitForOwner,
            false,
            now + Duration::from_millis(50),
        );

        assert_eq!(schedule.next_spawn_at, next_spawn_at);
        assert_eq!(schedule.delay, delay);
    }

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
