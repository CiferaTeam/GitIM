use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

/// Configuration for creating a provider instance.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// Path to the CLI executable. If None, uses the default for the provider.
    pub executable_path: Option<String>,
    /// Extra environment variables for the child process.
    pub env: HashMap<String, String>,
}

/// Options for a single execution.
#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    /// Working directory for the agent process.
    pub cwd: Option<PathBuf>,
    /// Model override (e.g., "claude-sonnet-4-6").
    pub model: Option<String>,
    /// System prompt to append.
    pub system_prompt: Option<String>,
    /// Maximum number of agent turns.
    pub max_turns: Option<u32>,
    /// Execution timeout. Defaults to 20 minutes if None.
    pub timeout: Option<Duration>,
    /// Resume token from a previous session.
    /// Claude: session_id, Codex: thread_id, etc.
    pub resume_token: Option<String>,
}

/// A running agent session with event streaming and final result.
pub struct Session {
    /// Stream of events emitted during execution.
    pub events: mpsc::Receiver<Event>,
    /// Final result — receives exactly one value, then closes.
    pub result: oneshot::Receiver<ExecResult>,
    abort_handle: AbortHandle,
    cancel_token: CancellationToken,
}

impl Session {
    pub fn new(
        events: mpsc::Receiver<Event>,
        result: oneshot::Receiver<ExecResult>,
        abort_handle: AbortHandle,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            events,
            result,
            abort_handle,
            cancel_token,
        }
    }

    /// Hard-kill the running execution. The tokio task is aborted immediately,
    /// so result_tx never fires — the caller gets RecvError from session.result.await.
    /// Use cancel() instead when you need a valid session_token in the ExecResult.
    pub fn abort(&self) {
        self.abort_handle.abort();
    }

    /// Gracefully cancel the running execution.
    /// Signals the provider to stop at the next clean point.
    /// The provider will send an ExecResult with status=Aborted and a valid
    /// session_token for resumption. Prefer this over abort() for steering.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }
}

/// Event emitted during agent execution.
#[derive(Debug, Clone)]
pub enum Event {
    /// Agent text output.
    Text { content: String },
    /// Agent thinking/reasoning (provider-dependent).
    Thinking { content: String },
    /// Tool invocation started.
    ToolUse {
        tool: String,
        call_id: String,
        input: serde_json::Value,
    },
    /// Tool invocation result.
    ToolResult { call_id: String, output: String },
    /// Agent status change.
    Status { status: String },
    /// Live usage snapshot emitted during execution. This is display-only:
    /// callers should use it to refresh HUD/SSE state, not to accumulate
    /// billing totals for the turn.
    Usage {
        session_id: String,
        usage: ProviderUsage,
    },
    /// Error during execution.
    Error { content: String },
    /// Log message from the agent process.
    Log { level: String, content: String },
}

/// Final result of an agent execution.
#[derive(Debug, Clone)]
pub struct ExecResult {
    /// Execution outcome.
    pub status: ExecStatus,
    /// Accumulated text output from the agent.
    pub output: String,
    /// Error message if status is Failed/Timeout/Aborted.
    pub error: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Session token for resuming (provider-specific).
    pub session_token: Option<String>,
    /// Per-turn billing usage data, when the provider reported any.
    pub usage: Option<ProviderUsage>,
    /// Split usage signals for accounting and context-window display.
    pub usage_report: ProviderUsageReport,
}

/// Execution outcome status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecStatus {
    Completed,
    Failed,
    Aborted,
    Timeout,
}

/// Usage as reported by a provider. Each provider fills the subset
/// its API exposes; missing fields stay `None`.
///
/// Context-window occupancy semantics differ by provider:
/// - Claude / opencode: sum `input_tokens + cache_read_tokens +
///   cache_creation_tokens` — cache hits are excluded from `input_tokens`
///   alone and underreport by orders of magnitude once caching kicks in.
/// - Pi: billing counters sum all assistant turns in the tool loop, while
///   `context_tokens` carries the latest assistant turn's live context.
/// - Codex: read `context_tokens` / `context_window_tokens`
///   directly. `cached_input_tokens` is already counted inside
///   `input_tokens` there, so the Claude-style sum would
///   double-count.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    /// Tokens served from the prompt cache (Claude `cache_read_input_tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Tokens written into the prompt cache this turn
    /// (Claude `cache_creation_input_tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    /// Current context-window tokens as reported by the provider. If
    /// `context_window_tokens` is absent, runtime may pair this with its
    /// provider/model default max.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Provider-reported context-window capacity matching `context_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u64>,
}

/// Provider usage split by semantic consumer.
///
/// `billing` is accumulated into token statistics. `context` is used for the
/// live context-window HUD and threshold handling. Providers whose wire format
/// exposes only one shape can put the same value in both fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderUsageReport {
    pub billing: Option<ProviderUsage>,
    pub context: Option<ProviderUsage>,
}

impl ProviderUsageReport {
    pub fn new(billing: Option<ProviderUsage>, context: Option<ProviderUsage>) -> Self {
        Self { billing, context }
    }

    pub fn from_usage(usage: Option<ProviderUsage>) -> Self {
        Self {
            billing: usage.clone(),
            context: usage,
        }
    }
}

/// Context passed to prompt generation methods.
#[derive(Debug, Clone)]
pub struct PromptContext<'a> {
    pub handler: &'a str,
    pub model: Option<&'a str>,
}
