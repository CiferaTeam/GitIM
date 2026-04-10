use gitim_client::ClientError;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("git clone failed: {0}")]
    GitCloneFailed(String),

    #[error("daemon start failed: {0}")]
    DaemonStartFailed(#[from] ClientError),

    #[error("onboard failed: {0}")]
    OnboardFailed(String),

    #[error("poll failed: {0}")]
    PollFailed(String),

    #[error("claude failed: {0}")]
    ClaudeFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
