use async_trait::async_trait;

use crate::{ExecOptions, PromptContext, ProviderConfig, ProviderError, Session};

/// Unified interface for executing prompts via headless coding agents.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Execute a prompt and return a Session for streaming results.
    ///
    /// The caller should read from `session.events` (optional) and await
    /// `session.result` for the final outcome.
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError>;

    fn prompt_identity(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_identity(ctx)
    }
    fn prompt_communication_style(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_communication_style(ctx)
    }
    fn prompt_cognitive_loop(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cognitive_loop(ctx)
    }
    fn prompt_collaboration(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_collaboration(ctx)
    }
    fn prompt_memory(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_memory(ctx)
    }
    fn prompt_cold_start(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cold_start(ctx)
    }
    fn prompt_gitim_api(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_gitim_api(ctx)
    }
    fn build_system_prompt(&self, ctx: &PromptContext) -> String {
        [
            self.prompt_identity(ctx),
            self.prompt_communication_style(ctx),
            self.prompt_cognitive_loop(ctx),
            self.prompt_collaboration(ctx),
            self.prompt_memory(ctx),
            self.prompt_cold_start(ctx),
            self.prompt_gitim_api(ctx),
        ]
        .join("\n\n")
    }
}

/// Create a provider for the given type.
///
/// Supported types: "claude", "codex", "gemini", "hermes", "openclaw", "opencode", "cursor", "mock".
pub fn create(
    provider_type: &str,
    config: ProviderConfig,
) -> Result<Box<dyn Provider>, ProviderError> {
    match provider_type {
        "claude" => Ok(Box::new(crate::claude::ClaudeProvider::new(config))),
        "codex" => Ok(Box::new(crate::codex::CodexProvider::new(config))),
        "gemini" => Ok(Box::new(crate::gemini::GeminiProvider::new(config))),
        "hermes" => Ok(Box::new(crate::hermes::HermesProvider::new(config))),
        "openclaw" => Ok(Box::new(crate::openclaw::OpenclawProvider::new(config))),
        "mock" => Ok(Box::new(crate::mock::MockProvider::new(config))),
        "cursor" => Ok(Box::new(crate::stubs::CursorProvider::new(config))),
        "opencode" => Ok(Box::new(crate::opencode::OpencodeProvider::new(config))),
        _ => Err(ProviderError::UnknownProvider(provider_type.to_string())),
    }
}
