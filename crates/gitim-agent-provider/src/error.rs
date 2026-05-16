use std::io;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("executable not found: {path}")]
    ExecutableNotFound { path: String },

    #[error("failed to start process: {0}")]
    SpawnFailed(#[from] io::Error),

    #[error("unknown provider type: {0}")]
    UnknownProvider(String),

    #[error("provider not implemented: {0}")]
    NotImplemented(String),

    /// Wire-level protocol error — JSON-RPC reported an `error` object,
    /// the stream ended mid-handshake, or a request timed out before
    /// receiving a response. Carries a human-readable description shaped
    /// for `ExecResult.error`.
    #[error("protocol error: {0}")]
    Protocol(String),
}
