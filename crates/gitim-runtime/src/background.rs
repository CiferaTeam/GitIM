use std::io;
use std::process::{Child, Command};

/// Spawn a child detached from the caller's controlling terminal/session.
///
/// On Unix we create a new session in the child and ignore SIGHUP so closing
/// the launching terminal does not tear down the background runtime.
pub fn spawn_detached(cmd: &mut Command) -> io::Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // Safety: `pre_exec` runs in the forked child immediately before
        // `exec`. We only call async-signal-safe libc functions.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::signal(libc::SIGHUP, libc::SIG_IGN) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    cmd.spawn()
}
