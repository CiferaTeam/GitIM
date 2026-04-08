#![deny(warnings)]

pub mod client;
pub mod error;
pub mod types;

pub use client::GitimClient;
pub use error::ClientError;
pub use types::ApiResponse;
