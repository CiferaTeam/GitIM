use crate::git::GitError;
use fs2::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::warn;

const LOCK_FILE: &str = "remote-git.lock";
const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);

struct RemoteSlotLock {
    file: std::fs::File,
}

impl Drop for RemoteSlotLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

enum LockAttempt {
    Acquired(RemoteSlotLock),
    Busy,
    Failed(std::io::Error),
}

pub(crate) fn with_remote_slot<T>(
    clone_root: &Path,
    op: impl FnOnce() -> Result<T, GitError>,
) -> Result<T, GitError> {
    let Some(lock_path) = discover_lock_path(clone_root) else {
        return op();
    };
    let _lock = match acquire_lock(&lock_path) {
        LockAttempt::Acquired(lock) => lock,
        LockAttempt::Busy => return Err(GitError::RemoteSlotBusy),
        LockAttempt::Failed(error) => {
            warn!(
                path = %lock_path.display(),
                error = %error,
                "remote git slot lock failed"
            );
            return Err(GitError::RemoteSlotUnavailable(error.to_string()));
        }
    };
    op()
}

fn discover_lock_path(clone_root: &Path) -> Option<PathBuf> {
    let clone_root = clone_root.canonicalize().ok()?;
    let workspace = clone_root
        .ancestors()
        .find(|ancestor| ancestor.join(".gitim-runtime/config.json").is_file())?;
    let runtime_dir = workspace.join(".gitim-runtime");
    if !std::fs::symlink_metadata(&runtime_dir)
        .ok()?
        .file_type()
        .is_dir()
        || runtime_dir.canonicalize().ok()? != runtime_dir
    {
        return None;
    }
    Some(runtime_dir.join(LOCK_FILE))
}

fn acquire_lock(path: &Path) -> LockAttempt {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) => return LockAttempt::Failed(error),
    };
    let started = Instant::now();
    loop {
        if started.elapsed() >= LOCK_WAIT_TIMEOUT {
            return LockAttempt::Busy;
        }
        match file.try_lock_exclusive() {
            Ok(()) => return LockAttempt::Acquired(RemoteSlotLock { file }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let elapsed = started.elapsed();
                if elapsed >= LOCK_WAIT_TIMEOUT {
                    return LockAttempt::Busy;
                }
                std::thread::sleep(LOCK_POLL_INTERVAL.min(LOCK_WAIT_TIMEOUT - elapsed));
            }
            Err(error) => return LockAttempt::Failed(error),
        }
    }
}
