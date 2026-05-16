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
    /// receiving a response. The carried string already names the failing
    /// method / message; `Display` is passthrough so it can flow straight
    /// into `ExecResult.error` without a redundant `"protocol error: "`
    /// prefix.
    #[error("{0}")]
    Protocol(String),
}
