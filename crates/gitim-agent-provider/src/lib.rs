pub mod claude;
pub mod codex;
mod error;
pub mod gemini;
pub mod hermes;
pub mod mock;
pub mod openclaw;
pub mod opencode;
pub(crate) mod prompts;
mod provider;
mod stubs;
mod types;
pub(crate) mod util;

pub use error::ProviderError;
pub use provider::{create, Provider};
pub use types::{
    Event, ExecOptions, ExecResult, ExecStatus, PromptContext, ProviderConfig, ProviderUsage,
    Session,
};
