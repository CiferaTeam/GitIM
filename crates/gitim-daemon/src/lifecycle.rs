use std::fs;
use std::path::{Path, PathBuf};
use crate::error::DaemonError;

pub struct DaemonLifecycle {
    run_dir: PathBuf,
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
        let _ = fs::remove_file(self.run_dir.join("gitim.pid"));
        let _ = fs::remove_file(self.run_dir.join("gitim.sock"));
        let _ = fs::remove_file(self.run_dir.join("gitim.port"));
        let _ = fs::remove_file(self.run_dir.join("gitim.lock"));
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
