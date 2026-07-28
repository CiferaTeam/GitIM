use crate::error::DaemonError;
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub struct DaemonLifecycle {
    run_dir: PathBuf,
}

#[derive(Debug)]
pub struct DaemonLease {
    _lock_file: File,
}

impl DaemonLifecycle {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            run_dir: repo_root.join(".gitim").join("run"),
        }
    }

    pub fn ensure_run_dir(&self) -> Result<(), DaemonError> {
        fs::create_dir_all(&self.run_dir)?;
        Ok(())
    }

    pub fn acquire(&self) -> Result<DaemonLease, DaemonError> {
        self.ensure_run_dir()?;
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.run_dir.join("gitim.lock"))?;

        match FileExt::try_lock_exclusive(&lock_file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                return Err(DaemonError::AlreadyRunningOrStarting);
            }
            Err(error) => return Err(error.into()),
        }

        self.prepare_stale_artifacts()?;
        self.write_pid()?;
        Ok(DaemonLease {
            _lock_file: lock_file,
        })
    }

    pub fn is_running(&self) -> Option<u32> {
        let pid_file = self.run_dir.join("gitim.pid");
        let pid_str = fs::read_to_string(&pid_file).ok()?;
        let pid: u32 = pid_str.trim().parse().ok()?;
        if process_exists(pid) {
            Some(pid)
        } else {
            let _ = fs::remove_file(&pid_file);
            None
        }
    }

    pub fn write_pid(&self) -> Result<(), DaemonError> {
        let pid = std::process::id();
        fs::write(self.run_dir.join("gitim.pid"), pid.to_string())?;
        Ok(())
    }

    pub fn socket_path(&self) -> PathBuf {
        self.run_dir.join("gitim.sock")
    }

    pub fn write_port(&self, port: u16) -> Result<(), DaemonError> {
        fs::write(self.run_dir.join("gitim.port"), port.to_string())?;
        Ok(())
    }

    pub fn cleanup(&self) {
        if !self.runtime_artifacts_belong_to_current_process() {
            return;
        }

        let _ = fs::remove_file(self.run_dir.join("gitim.pid"));
        let _ = fs::remove_file(self.run_dir.join("gitim.sock"));
        let _ = fs::remove_file(self.run_dir.join("gitim.port"));
    }

    fn prepare_stale_artifacts(&self) -> Result<(), DaemonError> {
        for name in ["gitim.pid", "gitim.sock", "gitim.port"] {
            match fs::remove_file(self.run_dir.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn runtime_artifacts_belong_to_current_process(&self) -> bool {
        fs::read_to_string(self.run_dir.join("gitim.pid"))
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            == Some(std::process::id())
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn runtime_path(temp_dir: &TempDir, name: &str) -> PathBuf {
        temp_dir.path().join(".gitim").join("run").join(name)
    }

    #[test]
    fn acquired_lease_rejects_second_acquisition() {
        let temp_dir = TempDir::new().expect("temp dir");
        let lifecycle = DaemonLifecycle::new(temp_dir.path());
        let _lease = lifecycle.acquire().expect("first lease");
        let pid_path = runtime_path(&temp_dir, "gitim.pid");
        let socket_path = runtime_path(&temp_dir, "gitim.sock");
        let port_path = runtime_path(&temp_dir, "gitim.port");
        let pid = fs::read_to_string(&pid_path).expect("owner pid");
        fs::write(&socket_path, "active socket").expect("active socket");
        fs::write(&port_path, "16868").expect("active port");

        let error = lifecycle.acquire().expect_err("second lease must fail");

        assert!(matches!(error, DaemonError::AlreadyRunningOrStarting));
        assert_eq!(fs::read_to_string(pid_path).expect("owner pid"), pid);
        assert_eq!(
            fs::read_to_string(socket_path).expect("active socket"),
            "active socket"
        );
        assert_eq!(fs::read_to_string(port_path).expect("active port"), "16868");
    }

    #[test]
    fn lease_can_be_reacquired_after_drop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let lifecycle = DaemonLifecycle::new(temp_dir.path());
        let lease = lifecycle.acquire().expect("first lease");
        drop(lease);

        let _lease = lifecycle.acquire().expect("reacquired lease");
    }

    #[test]
    fn lock_file_remains_after_cleanup() {
        let temp_dir = TempDir::new().expect("temp dir");
        let lifecycle = DaemonLifecycle::new(temp_dir.path());
        let lease = lifecycle.acquire().expect("lease");
        let lock_path = runtime_path(&temp_dir, "gitim.lock");

        lifecycle.cleanup();
        drop(lease);

        assert!(lock_path.exists());
    }

    #[test]
    fn cleanup_removes_runtime_artifacts_owned_by_current_process() {
        let temp_dir = TempDir::new().expect("temp dir");
        let lifecycle = DaemonLifecycle::new(temp_dir.path());
        let _lease = lifecycle.acquire().expect("lease");
        let pid_path = runtime_path(&temp_dir, "gitim.pid");
        let socket_path = runtime_path(&temp_dir, "gitim.sock");
        let port_path = runtime_path(&temp_dir, "gitim.port");
        fs::write(&socket_path, "socket").expect("socket artifact");
        fs::write(&port_path, "16868").expect("port artifact");

        lifecycle.cleanup();

        assert!(!pid_path.exists());
        assert!(!socket_path.exists());
        assert!(!port_path.exists());
        assert!(runtime_path(&temp_dir, "gitim.lock").exists());
    }

    #[test]
    fn cleanup_preserves_runtime_artifacts_owned_by_another_process() {
        let temp_dir = TempDir::new().expect("temp dir");
        let lifecycle = DaemonLifecycle::new(temp_dir.path());
        lifecycle.ensure_run_dir().expect("run dir");
        let pid_path = runtime_path(&temp_dir, "gitim.pid");
        let socket_path = runtime_path(&temp_dir, "gitim.sock");
        let port_path = runtime_path(&temp_dir, "gitim.port");
        let lock_path = runtime_path(&temp_dir, "gitim.lock");
        fs::write(&pid_path, std::process::id().wrapping_add(1).to_string()).expect("foreign pid");
        fs::write(&socket_path, "socket").expect("socket artifact");
        fs::write(&port_path, "16868").expect("port artifact");
        fs::write(&lock_path, "").expect("lock artifact");

        lifecycle.cleanup();

        assert!(pid_path.exists());
        assert!(socket_path.exists());
        assert!(port_path.exists());
        assert!(lock_path.exists());
    }
}
