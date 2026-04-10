use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

/// Configuration for creating a provider instance.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Path to the CLI executable. If None, uses the default for the provider.
    pub executable_path: Option<String>,
    /// Extra environment variables for the child process.
    pub env: HashMap<String, String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            executable_path: None,
            env: HashMap::new(),
        }
    }
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
}

impl Session {
    pub fn new(
        events: mpsc::Receiver<Event>,
        result: oneshot::Receiver<ExecResult>,
        abort_handle: AbortHandle,
    ) -> Self {
        Self {
            events,
            result,
            abort_handle,
        }
    }

    /// Abort the running execution. The child process will be killed.
    pub fn abort(&self) {
        self.abort_handle.abort();
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
}

/// Execution outcome status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecStatus {
    Completed,
    Failed,
    Aborted,
    Timeout,
}
