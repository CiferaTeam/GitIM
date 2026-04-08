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
    #[error("api error: {message}")]
    Api { message: String },
}
