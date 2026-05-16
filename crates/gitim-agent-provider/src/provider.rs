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

    /// Whether this provider populates `ExecResult.usage` with token counts.
    ///
    /// Providers backed by SDKs that don't surface usage data (e.g., Gemini
    /// CLI, openclaw) return `false` so the runtime can skip token-bucket
    /// accumulation and only count turns.
    fn reports_usage(&self) -> bool {
        true
    }

    /// Whether `ExecResult.usage` reports cumulative token counts for the
    /// resumed session, or per-turn deltas.
    ///
    /// - `false` (default): each turn's `usage` reflects only that turn.
    ///   The runtime adds it directly to the daily bucket.
    /// - `true`: each turn's `usage` is the running session total. The
    ///   runtime computes the delta against `AgentState.last_session_usage`
    ///   before accumulating, and resets the baseline whenever the session
    ///   id changes.
    ///
    /// This matters only when `reports_usage()` is `true`.
    fn usage_is_cumulative(&self) -> bool {
        false
    }

    /// Whether the provider self-manages its conversation context.
    ///
    /// - `false` (default): the runtime is responsible for keeping context
    ///   pressure under control — it computes occupancy (via provider usage
    ///   or cl100k estimate), emits a `[系统通知]` preamble when the agent
    ///   crosses `WARN_AT_PERCENT`, and the agent uses the `[[RESET]]`
    ///   sentinel to hand off to a fresh session. This is the design built
    ///   for Claude / Codex, which only do emergency compaction near the
    ///   model's hard limit.
    /// - `true`: the provider performs proactive in-loop compression
    ///   (e.g. hermes-agent's `compression.threshold: 0.5`). The runtime's
    ///   estimator has no visibility into what the provider compressed, so
    ///   occupancy computations are unreliable. Self-managed providers opt
    ///   out of the entire pressure-relief mechanism: no occupancy gauge,
    ///   no threshold preamble, no forced `[[RESET]]`. Provider identity /
    ///   persistent memory must survive its own compression — typically by
    ///   living in files the provider auto-reloads after compression
    ///   (e.g. hermes SOUL.md / MEMORY.md), not by being re-injected as the
    ///   first user message of a fresh runtime-side session.
    fn self_managed_context(&self) -> bool {
        false
    }

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

    fn prompt_reset_protocol(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_reset_protocol(ctx)
    }

    fn prompt_cold_start(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cold_start(ctx)
    }

    fn prompt_gitim_api(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_gitim_api(ctx)
    }

    fn prompt_host_safety(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_host_safety(ctx)
    }
    fn build_system_prompt(&self, ctx: &PromptContext) -> String {
        [
            self.prompt_identity(ctx),
            self.prompt_communication_style(ctx),
            self.prompt_cognitive_loop(ctx),
            self.prompt_collaboration(ctx),
            self.prompt_memory(ctx),
            self.prompt_reset_protocol(ctx),
            self.prompt_cold_start(ctx),
            self.prompt_gitim_api(ctx),
            self.prompt_host_safety(ctx),
        ]
        .join("\n\n")
    }
}

/// Create a provider for the given type — see the match below for
/// the supported `provider_type` strings.
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
        "cursor" => Ok(Box::new(crate::cursor::CursorProvider::new(config))),
        "opencode" => Ok(Box::new(crate::opencode::OpencodeProvider::new(config))),
        "pi" => Ok(Box::new(crate::pi::PiProvider::new(config))),
        _ => Err(ProviderError::UnknownProvider(provider_type.to_string())),
    }
}
