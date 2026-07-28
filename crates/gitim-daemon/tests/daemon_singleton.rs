#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const CANDIDATE_COUNT: usize = 8;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

struct Candidate {
    child: Child,
    status: Option<ExitStatus>,
    stderr_path: PathBuf,
    stderr: Option<String>,
}

impl Candidate {
    fn new(child: Child, stderr_path: PathBuf) -> Self {
        Self {
            child,
            status: None,
            stderr_path,
            stderr: None,
        }
    }

    fn poll(&mut self) {
        if self.status.is_none() {
            self.status = self.child.try_wait().expect("poll daemon candidate");
        }
    }

    fn stderr(&mut self) -> &str {
        if self.stderr.is_none() {
            assert!(self.status.is_some(), "stderr is only final after exit");
            let stderr =
                std::fs::read_to_string(&self.stderr_path).expect("read captured candidate stderr");
            self.stderr = Some(stderr);
        }
        self.stderr.as_deref().expect("cached candidate stderr")
    }
}

struct ChildGuard {
    candidates: Vec<Candidate>,
}

impl ChildGuard {
    fn new() -> Self {
        Self {
            candidates: Vec::with_capacity(CANDIDATE_COUNT),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        for candidate in &mut self.candidates {
            match candidate.child.try_wait() {
                Ok(Some(status)) => candidate.status = Some(status),
                Ok(None) | Err(_) => {
                    let _ = candidate.child.kill();
                    candidate.status = candidate.child.wait().ok();
                }
            }
        }
    }
}

fn write_config(repo_root: &Path) {
    let gitim_dir = repo_root.join(".gitim");
    std::fs::create_dir_all(&gitim_dir).expect("create .gitim directory");

    let endpoint_url = "a".repeat(4 * 1024 * 1024);
    let config = format!(
        "version: 1\nendpoint: local\nendpoint_url: {endpoint_url}\ndaemon:\n  sync_interval: 3600\n  debug_http: false\nindexer:\n  enabled: false\n"
    );
    std::fs::write(gitim_dir.join("config.yaml"), config).expect("write daemon config");
}

fn init_git_repo(repo_root: &Path) {
    let home = repo_root.join("git-init-home");
    let log_dir = repo_root.join("git-init-logs");
    std::fs::create_dir_all(&home).expect("create git init HOME");
    std::fs::create_dir_all(&log_dir).expect("create git init log directory");

    let output = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "init", "--quiet"])
        .arg(repo_root)
        .env("HOME", home)
        .env("GITIM_LOG_DIR", log_dir)
        .output()
        .expect("run git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn candidate_command(repo_root: &Path, env_root: &Path, index: usize) -> (Command, PathBuf) {
    let home = env_root.join(format!("candidate-{index}/home"));
    let log_dir = env_root.join(format!("candidate-{index}/logs"));
    std::fs::create_dir_all(&home).expect("create candidate HOME");
    std::fs::create_dir_all(&log_dir).expect("create candidate log directory");
    let stderr_path = log_dir.join("stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create candidate stderr log");

    let mut command = Command::new(env!("CARGO_BIN_EXE_gitim-daemon"));
    command
        .current_dir(repo_root)
        .env("HOME", home)
        .env("GITIM_LOG_DIR", log_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));
    (command, stderr_path)
}

fn exited_diagnostics(candidates: &mut [Candidate]) -> String {
    let mut diagnostics = Vec::new();
    for candidate in candidates {
        candidate.poll();
        if let Some(status) = candidate.status {
            diagnostics.push(format!(
                "pid={} status={status} stderr={:?}",
                candidate.child.id(),
                candidate.stderr()
            ));
        } else {
            diagnostics.push(format!("pid={} status=running", candidate.child.id()));
        }
    }
    diagnostics.join("\n")
}

#[test]
fn concurrent_startup_keeps_exactly_one_daemon_owner() {
    let temp_dir = tempfile::tempdir().expect("temp repo");
    let repo_root = temp_dir.path().to_path_buf();
    init_git_repo(&repo_root);
    write_config(&repo_root);

    let env_root = temp_dir.path().join("candidate-env");
    let commands: Vec<_> = (0..CANDIDATE_COUNT)
        .map(|index| candidate_command(&repo_root, &env_root, index))
        .collect();
    let barrier = Arc::new(Barrier::new(CANDIDATE_COUNT));
    let mut threads = Vec::with_capacity(CANDIDATE_COUNT);
    for (mut command, stderr_path) in commands {
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            command
                .spawn()
                .map(|child| (child, stderr_path))
                .map_err(|error| error.to_string())
        }));
    }

    let mut guard = ChildGuard::new();
    let mut spawn_errors = Vec::new();
    for thread in threads {
        match thread.join() {
            Ok(Ok((child, stderr_path))) => {
                guard.candidates.push(Candidate::new(child, stderr_path));
            }
            Ok(Err(error)) => spawn_errors.push(error),
            Err(_) => spawn_errors.push("candidate spawn thread panicked".to_string()),
        }
    }
    assert!(
        spawn_errors.is_empty(),
        "failed to spawn daemon candidates: {spawn_errors:?}"
    );
    assert_eq!(guard.candidates.len(), CANDIDATE_COUNT);

    let socket_path: PathBuf = repo_root.join(".gitim/run/gitim.sock");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut socket_ready = false;
    loop {
        for candidate in &mut guard.candidates {
            candidate.poll();
        }
        socket_ready |= UnixStream::connect(&socket_path).is_ok();

        let exited = guard
            .candidates
            .iter()
            .filter(|candidate| candidate.status.is_some())
            .count();
        if socket_ready && exited == CANDIDATE_COUNT - 1 {
            break;
        }
        if exited == CANDIDATE_COUNT || Instant::now() >= deadline {
            let diagnostics = exited_diagnostics(&mut guard.candidates);
            panic!(
                "daemon candidates did not converge before {STARTUP_TIMEOUT:?}; socket_ready={socket_ready}, exited={exited}\n{diagnostics}"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    for candidate in &mut guard.candidates {
        candidate.poll();
    }
    let survivors: Vec<_> = guard
        .candidates
        .iter()
        .filter(|candidate| candidate.status.is_none())
        .collect();
    assert_eq!(survivors.len(), 1, "expected exactly one live daemon owner");
    let survivor_pid = survivors[0].child.id();

    let pid_path = repo_root.join(".gitim/run/gitim.pid");
    let pid_file = std::fs::read_to_string(&pid_path).expect("read daemon pid file");
    assert_eq!(
        pid_file.trim().parse::<u32>().expect("parse daemon pid"),
        survivor_pid,
        "pid file must name the live daemon owner"
    );

    for candidate in &mut guard.candidates {
        let Some(status) = candidate.status else {
            continue;
        };
        assert!(
            !status.success(),
            "loser pid {} exited successfully",
            candidate.child.id()
        );
        let candidate_pid = candidate.child.id();
        let stderr = candidate.stderr();
        assert!(
            stderr.contains("AlreadyRunningOrStarting")
                || stderr.contains("daemon already running or starting"),
            "loser pid {} did not report lifecycle lock contention; stderr: {stderr:?}",
            candidate_pid
        );
    }
}
