use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("daemon already running (pid: {0})")]
    AlreadyRunning(u32),
    #[error("failed to acquire lock: {0}")]
    LockFailed(#[from] std::io::Error),
    #[error("gitim repo not found at {0}")]
    RepoNotFound(String),
    #[error("invalid config: {0}")]
    ConfigError(String),
}
