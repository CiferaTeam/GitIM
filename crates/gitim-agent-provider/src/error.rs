use std::io;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("executable not found: {path}")]
    ExecutableNotFound { path: String },

    #[error("failed to start process: {0}")]
    SpawnFailed(#[from] io::Error),

    #[error("unknown provider type: {0}")]
    UnknownProvider(String),
}
