use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("request timed out")]
    Timeout,
    #[error("protocol error: {0}")]
    ProtocolError(String),
    #[error("daemon not running")]
    DaemonNotRunning,
    /// Daemon responded with `ok: false`. `code` mirrors the typed
    /// `error_code` tag (e.g. `"name_conflict"`, `"not_found"`) when the
    /// daemon emits one — callers translate it into user-facing messages.
    /// `None` for legacy handlers that only set `error` without a tag.
    #[error("api error: {message}")]
    Api {
        message: String,
        code: Option<String>,
    },
}
