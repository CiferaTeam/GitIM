#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod acp;
pub mod claude;
pub mod codex;
pub mod cursor;
mod error;
pub mod gemini;

// System-library invariants: these helpers document and enforce invariants
// that are guaranteed by the Rust standard library / tokio.
pub(crate) mod preconditions;
pub mod hermes;
pub mod kimi;
pub mod mock;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub(crate) mod prompts;
mod provider;
mod types;
pub(crate) mod util;

pub use error::ProviderError;
pub use provider::{create, provider_reports_usage, Provider};
pub use types::{
    Event, ExecOptions, ExecResult, ExecStatus, PromptContext, ProviderConfig, ProviderUsage,
    ProviderUsageReport, Session,
};
