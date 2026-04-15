use async_trait::async_trait;

use crate::{ExecOptions, ProviderConfig, ProviderError, Session};

/// Unified interface for executing prompts via headless coding agents.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Execute a prompt and return a Session for streaming results.
    ///
    /// The caller should read from `session.events` (optional) and await
    /// `session.result` for the final outcome.
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError>;
}

/// Create a provider for the given type.
///
/// Supported types: "claude", "codex", "cursor", "opencode".
pub fn create(
    provider_type: &str,
    config: ProviderConfig,
) -> Result<Box<dyn Provider>, ProviderError> {
    match provider_type {
        "claude" => Ok(Box::new(crate::claude::ClaudeProvider::new(config))),
        "codex" => Ok(Box::new(crate::codex::CodexProvider::new(config))),
        "gemini" => Ok(Box::new(crate::gemini::GeminiProvider::new(config))),
        "openclaw" => Ok(Box::new(crate::openclaw::OpenclawProvider::new(config))),
        "mock" => Ok(Box::new(crate::mock::MockProvider::new(config))),
        "cursor" => Ok(Box::new(crate::stubs::CursorProvider::new(config))),
        "opencode" => Ok(Box::new(crate::stubs::OpencodeProvider::new(config))),
        _ => Err(ProviderError::UnknownProvider(provider_type.to_string())),
    }
}
