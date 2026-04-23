use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn wait_until(deadline: Duration, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("condition not met within {deadline:?}");
}

fn run_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".gitim").join("run")
}

fn spawn_daemon(repo_root: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_gitim-daemon"))
        .current_dir(repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn gitim-daemon")
}

fn kill_force(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGKILL);
        }
        let _ = child.wait();
    }
}

#[test]
fn sigterm_removes_pid_and_socket_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_root = tmp.path();
    let run_dir = run_dir(repo_root);
    let pid_file = run_dir.join("gitim.pid");
    let sock_file = run_dir.join("gitim.sock");

    let mut child = spawn_daemon(repo_root);

    wait_until(Duration::from_secs(5), || {
        pid_file.exists() && sock_file.exists()
    });

    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }

    wait_until(Duration::from_secs(5), || {
        child.try_wait().expect("try_wait").is_some()
    });

    assert!(
        !pid_file.exists(),
        "pid file should be removed on SIGTERM: {}",
        pid_file.display()
    );
    assert!(
        !sock_file.exists(),
        "socket file should be removed on SIGTERM: {}",
        sock_file.display()
    );

    kill_force(&mut child);
}
