#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
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

fn write_sleep_script(path: &Path) {
    std::fs::write(
        path,
        "#!/bin/sh\nmarker=\"$1\"\necho $$ > \"$marker\"\nsleep 30\n",
    )
    .expect("write helper script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod helper script");
    }
}

fn kill_force(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGKILL);
        }
        let _ = child.wait();
    }
}

#[cfg(unix)]
#[test]
fn spawn_detached_makes_child_session_leader() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let script = tmp.path().join("sleep.sh");
    let marker = tmp.path().join("marker.pid");
    write_sleep_script(&script);

    let mut cmd = Command::new(&script);
    cmd.arg(&marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child =
        gitim_runtime::background::spawn_detached(&mut cmd).expect("spawn detached child");

    wait_until(Duration::from_secs(5), || marker.exists());

    let pid = child.id() as i32;
    let child_sid = unsafe { libc::getsid(pid) };
    assert_eq!(
        child_sid, pid,
        "detached child should become session leader"
    );

    let parent_sid = unsafe { libc::getsid(0) };
    assert_ne!(
        child_sid, parent_sid,
        "detached child should not share the parent's session"
    );

    kill_force(&mut child);
}
