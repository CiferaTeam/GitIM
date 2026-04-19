#![deny(warnings)]

pub mod client;
pub mod daemon;
pub mod error;
pub mod types;

pub use client::GitimClient;
pub use daemon::{ensure_daemon, ensure_daemon_with_log, find_repo_root, is_daemon_running};
pub use error::ClientError;
pub use types::ApiResponse;
