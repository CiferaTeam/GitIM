use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{
    preconditions, Event, ExecOptions, ExecResult, ExecStatus, PromptContext, Provider,
    ProviderConfig, ProviderError, ProviderUsage, ProviderUsageReport, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct ClaudeProvider {
    config: ProviderConfig,
}

impl ClaudeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn prompt_identity(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_identity(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    fn prompt_memory(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_memory(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    fn prompt_reset_protocol(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_reset_protocol(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    fn prompt_cold_start(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cold_start(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "claude".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut args = vec![
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(max_turns) = opts.max_turns {
            args.extend(["--max-turns".to_string(), max_turns.to_string()]);
        }
        if let Some(system_prompt) = &opts.system_prompt {
            args.extend(["--append-system-prompt".to_string(), system_prompt.clone()]);
        }
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["--resume".to_string(), resume_token.clone()]);
        }
        args.extend(["-p".to_string(), prompt.to_string()]);

        let mut cmd = Command::new(&exec_path);
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "claude started");

        // piped stdio: use preconditions helpers for system-library invariants
        let stdout = preconditions::take_tokio_piped_stdout(&mut child);
        let stdin = preconditions::take_tokio_piped_stdin(&mut child);
        let stderr = preconditions::take_tokio_piped_stderr(&mut child);

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(
                child,
                stdout,
                stdin,
                stderr,
                event_tx,
                result_tx,
                timeout,
                pid,
                cancel_token_inner,
            )
            .await;
        });

        Ok(Session::new(
            event_rx,
            result_rx,
            join_handle.abort_handle(),
            cancel_token,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_session(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stdin: tokio::process::ChildStdin,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    timeout: Duration,
    pid: u32,
    cancel_token: CancellationToken,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut saw_result = false;
    let mut num_turns: u32 = 0;
    let mut context_usage: Option<ProviderUsage> = None;
    let mut billing_usage: Option<ProviderUsage> = None;

    let mut reader = BufReader::new(stdout).lines();
    let mut stdin = stdin;

    // Collect stderr tail for error reporting
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "claude:stderr", "{}", line);
            // mutex_lock documents and enforces the poisoned-guard invariant
            let mut tail = preconditions::mutex_lock(&stderr_tail_clone);
            tail.push(line);
            if tail.len() > TAIL_LINES {
                tail.remove(0);
            }
        }
    });

    let read_result = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                line = reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            let parsed = match parse_line(&line) {
                                Some(p) => p,
                                None => {
                                    debug!(pid, line_len = line.len(), "unparsed line");
                                    continue;
                                }
                            };

                            match parsed {
                                ParsedMessage::System { session_id: sid } => {
                                    session_id = sid;
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::AssistantEvents { events, usage } => {
                                    num_turns += 1;
                                    // Per-iteration usage reflects actual window occupancy
                                    // at this step. Result.usage sums across iterations and
                                    // is the billing signal.
                                    if usage.as_ref().is_some_and(usage_has_reported_tokens) {
                                        context_usage = usage;
                                    }
                                    for event in events {
                                        if let Event::Text { ref content } = event {
                                            output.push_str(content);
                                        }
                                        try_send_event(&event_tx, event);
                                    }
                                }
                                ParsedMessage::UserEvents(events) => {
                                    for event in events {
                                        try_send_event(&event_tx, event);
                                    }
                                }
                                ParsedMessage::Result {
                                    session_id: sid,
                                    output: result_text,
                                    is_error,
                                    usage: result_usage,
                                } => {
                                    saw_result = true;
                                    session_id = sid;
                                    if result_usage.as_ref().is_some_and(usage_has_reported_tokens) {
                                        billing_usage = result_usage.clone();
                                    }
                                    // Fall back to result.usage for context when no assistant
                                    // message surfaced real per-iteration usage. Some
                                    // Anthropic-compatible endpoints emit zero-filled assistant
                                    // usage placeholders and put real counts on the final result.
                                    if should_replace_captured_usage(&context_usage, &result_usage) {
                                        context_usage = result_usage;
                                    }
                                    info!(
                                        pid,
                                        is_error,
                                        turns = num_turns,
                                        result_len = result_text.len(),
                                        "claude result received"
                                    );
                                    if is_error {
                                        final_status = ExecStatus::Failed;
                                        final_error = if result_text.is_empty() {
                                            None
                                        } else {
                                            Some(result_text)
                                        };
                                    } else if !result_text.is_empty() {
                                        output = result_text;
                                    }
                                }
                                ParsedMessage::ControlRequest { request_id, input } => {
                                    let response = build_auto_approve_response(&request_id, &input);
                                    if let Ok(data) = serde_json::to_vec(&response) {
                                        let mut buf = data;
                                        buf.push(b'\n');
                                        if let Err(e) = stdin.write_all(&buf).await {
                                            warn!("failed to write control response: {e}");
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!(pid, error = %e, "stdout read error");
                            break;
                        }
                    }
                }
                _ = cancel_token.cancelled() => {
                    info!(pid, "cancelled by steering");
                    final_status = ExecStatus::Aborted;
                    final_error = Some("cancelled by steering".to_string());
                    break;
                }
            }
        }
    })
    .await;

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("claude timed out after {timeout:?}"));
        // Kill the child process — kill_on_drop only fires on Drop,
        // but we still hold the Child reference.
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else if !saw_result && final_status == ExecStatus::Completed {
        // Stream ended without a result message — truncated or protocol error
        final_status = ExecStatus::Failed;
        final_error = Some("claude stream ended without a result message".to_string());
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("claude exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for claude: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(
        pid,
        ?final_status,
        turns = num_turns,
        ?duration,
        "claude finished"
    );

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
        let tail = preconditions::mutex_lock(&stderr_tail);
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

    let report_billing = billing_usage.or_else(|| context_usage.clone());
    let usage_report = ProviderUsageReport::new(report_billing, context_usage);
    let usage = usage_report.billing.clone();
    let _ = result_tx.send(ExecResult {
        status: final_status,
        output,
        error: final_error,
        duration_ms: duration.as_millis() as u64,
        session_token: if session_id.is_empty() {
            None
        } else {
            Some(session_id)
        },
        usage_report,
        usage,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

fn should_replace_captured_usage(
    captured_usage: &Option<ProviderUsage>,
    candidate_usage: &Option<ProviderUsage>,
) -> bool {
    captured_usage
        .as_ref()
        .is_none_or(|usage| !usage_has_reported_tokens(usage))
        && candidate_usage
            .as_ref()
            .is_some_and(usage_has_reported_tokens)
}

fn usage_has_reported_tokens(usage: &ProviderUsage) -> bool {
    usage.input_tokens.unwrap_or(0) > 0
        || usage.output_tokens.unwrap_or(0) > 0
        || usage.cache_read_tokens.unwrap_or(0) > 0
        || usage.cache_creation_tokens.unwrap_or(0) > 0
        || usage.context_tokens.unwrap_or(0) > 0
        || usage.context_window_tokens.unwrap_or(0) > 0
        || usage.used_percent.is_some_and(|pct| pct > 0.0)
}

fn build_auto_approve_response(request_id: &str, input: &Value) -> Value {
    let updated_input = if input.is_object() {
        input.clone()
    } else {
        Value::Object(Default::default())
    };
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "allow",
                "updatedInput": updated_input
            }
        }
    })
}

/// Parsed result from a single line of Claude stream-json output.
#[derive(Debug)]
pub enum ParsedMessage {
    /// System init message with session ID.
    System { session_id: String },
    /// Events from an assistant message (text contributes to output).
    AssistantEvents {
        events: Vec<Event>,
        /// Per-iteration usage carried on this assistant message. See the
        /// `MessageContent.usage` doc comment for why this beats
        /// `Result.usage` as a window-occupancy signal.
        usage: Option<ProviderUsage>,
    },
    /// Events from a user message (tool results, not accumulated into output).
    UserEvents(Vec<Event>),
    /// Final result.
    Result {
        session_id: String,
        output: String,
        is_error: bool,
        usage: Option<ProviderUsage>,
    },
    /// Permission control request requiring a response on stdin.
    ControlRequest { request_id: String, input: Value },
}

/// Parse a single line of Claude stream-json output.
/// Returns None for empty lines, malformed JSON, or unrecognized message types.
pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let raw: RawMessage = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "system" => Some(ParsedMessage::System {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "assistant" => {
            let content: MessageContent = serde_json::from_value(raw.message?).ok()?;
            let events = parse_content_blocks(&content);
            if events.is_empty() {
                None
            } else {
                let usage = content.usage.map(|u| ProviderUsage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    used_percent: None,
                    cache_read_tokens: u.cache_read_tokens,
                    cache_creation_tokens: u.cache_creation_tokens,
                    context_tokens: None,
                    context_window_tokens: None,
                });
                Some(ParsedMessage::AssistantEvents { events, usage })
            }
        }
        "user" => {
            let content: MessageContent = serde_json::from_value(raw.message?).ok()?;
            let events = parse_content_blocks(&content);
            if events.is_empty() {
                None
            } else {
                Some(ParsedMessage::UserEvents(events))
            }
        }
        "result" => Some(ParsedMessage::Result {
            session_id: raw.session_id.unwrap_or_default(),
            output: raw.result.unwrap_or_default(),
            is_error: raw.is_error.unwrap_or(false),
            usage: raw.usage.map(|u| ProviderUsage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                used_percent: None,
                cache_read_tokens: u.cache_read_tokens,
                cache_creation_tokens: u.cache_creation_tokens,
                context_tokens: None,
                context_window_tokens: None,
            }),
        }),
        "log" => {
            let log = raw.log?;
            Some(ParsedMessage::AssistantEvents {
                events: vec![Event::Log {
                    level: log.level,
                    content: log.message,
                }],
                usage: None,
            })
        }
        "control_request" => {
            let request: ControlRequestPayload = serde_json::from_value(raw.request?).ok()?;
            let input = request.input.unwrap_or(Value::Object(Default::default()));
            Some(ParsedMessage::ControlRequest {
                request_id: raw.request_id?,
                input,
            })
        }
        _ => None,
    }
}

fn parse_content_blocks(content: &MessageContent) -> Vec<Event> {
    let mut events = Vec::new();
    for block in &content.content {
        match block.r#type.as_str() {
            "text" => {
                if let Some(text) = &block.text {
                    if !text.is_empty() {
                        events.push(Event::Text {
                            content: text.clone(),
                        });
                    }
                }
            }
            "thinking" => {
                if let Some(text) = &block.text {
                    if !text.is_empty() {
                        events.push(Event::Thinking {
                            content: text.clone(),
                        });
                    }
                }
            }
            "tool_use" => {
                let input = block
                    .input
                    .clone()
                    .unwrap_or(Value::Object(Default::default()));
                events.push(Event::ToolUse {
                    tool: block.name.clone().unwrap_or_default(),
                    call_id: block.id.clone().unwrap_or_default(),
                    input,
                });
            }
            "tool_result" => {
                let output = block
                    .tool_result_content
                    .as_ref()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                events.push(Event::ToolResult {
                    call_id: block.tool_use_id.clone().unwrap_or_default(),
                    output,
                });
            }
            _ => {}
        }
    }
    events
}

// ── Claude SDK JSON types (internal) ──

#[derive(Deserialize)]
struct RawMessage {
    r#type: String,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
    #[serde(default)]
    log: Option<LogEntry>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    request: Option<Value>,
}

/// Usage block from Claude stream-json — appears on both per-iteration
/// `assistant` messages and the final `result` message.
///
/// Anthropic reports `input_tokens` **excluding** cache hits. Real
/// context-window occupancy is the sum `input + cache_read +
/// cache_creation`, so all three are propagated into `ProviderUsage`
/// and aggregated by the runtime when computing percentages.
#[derive(Debug, Clone, Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default, rename = "cache_read_input_tokens")]
    cache_read_tokens: Option<u64>,
    #[serde(default, rename = "cache_creation_input_tokens")]
    cache_creation_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct LogEntry {
    level: String,
    message: String,
}

#[derive(Deserialize)]
struct MessageContent {
    #[serde(default)]
    content: Vec<ContentBlock>,
    /// Per-iteration usage from a `type:assistant` message.
    ///
    /// Claude Code CLI emits one `assistant` message per inference step inside
    /// a single CLI invocation. The `usage` here is scoped to *that* request,
    /// so the last iteration's value reflects the actual context-window
    /// occupancy at turn end. The final `type:result` event carries an
    /// aggregate across iterations that double-counts cached context; prefer
    /// this field when available.
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    tool_use_id: Option<String>,
    /// The "content" field in tool_result blocks.
    /// Renamed to avoid conflict with the struct field name in ContentBlock list.
    #[serde(default, rename = "content")]
    tool_result_content: Option<Value>,
}

#[derive(Deserialize)]
struct ControlRequestPayload {
    #[serde(default)]
    input: Option<Value>,
}

#[cfg(test)]
mod usage_parse_tests {
    use super::*;

    #[test]
    fn parse_result_with_usage_block() {
        let line = r#"{
            "type": "result",
            "session_id": "sess-abc",
            "result": "hello",
            "is_error": false,
            "usage": {
                "input_tokens": 164000,
                "output_tokens": 520,
                "cache_read_input_tokens": 120000,
                "cache_creation_input_tokens": 800
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::Result { usage, .. } = parsed else {
            panic!("expected Result variant");
        };
        let usage = usage.expect("usage field present");
        assert_eq!(usage.input_tokens, Some(164_000));
        assert_eq!(usage.output_tokens, Some(520));
        assert_eq!(usage.cache_read_tokens, Some(120_000));
        assert_eq!(usage.cache_creation_tokens, Some(800));
    }

    #[test]
    fn parse_result_with_cache_only_has_tiny_input() {
        // Real-world turn 2+: prompt caching active. input_tokens is just the
        // delta; the bulk of context comes through cache_read_input_tokens.
        // This is the scenario that produced the "0%" display bug.
        let line = r#"{
            "type": "result",
            "session_id": "sess-xyz",
            "result": "ok",
            "is_error": false,
            "usage": {
                "input_tokens": 312,
                "output_tokens": 180,
                "cache_read_input_tokens": 159500,
                "cache_creation_input_tokens": 220
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::Result { usage, .. } = parsed else {
            panic!("expected Result variant");
        };
        let usage = usage.expect("usage field present");
        assert_eq!(usage.input_tokens, Some(312));
        assert_eq!(usage.cache_read_tokens, Some(159_500));
        assert_eq!(usage.cache_creation_tokens, Some(220));
    }

    #[test]
    fn assistant_message_surfaces_per_iteration_usage() {
        // Per-iteration usage carried on the assistant message is the
        // authoritative window-occupancy signal. The aggregated
        // `result.usage` is a sum across all iterations and inflates the
        // denominator (N× cached context) — never use it for occupancy.
        let line = r#"{
            "type": "assistant",
            "session_id": "sess-abc",
            "message": {
                "content": [{"type": "text", "text": "ok"}],
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 34,
                    "cache_read_input_tokens": 59560,
                    "cache_creation_input_tokens": 325
                }
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::AssistantEvents { usage, .. } = parsed else {
            panic!("expected AssistantEvents variant");
        };
        let usage = usage.expect("per-iteration usage present");
        assert_eq!(usage.input_tokens, Some(1));
        assert_eq!(usage.cache_read_tokens, Some(59_560));
        assert_eq!(usage.cache_creation_tokens, Some(325));
        assert!(
            usage.used_percent.is_none(),
            "Claude never sets used_percent"
        );

        // Effective = 59,886 — the actual window-occupancy number the runtime
        // should divide into max_tokens. The aggregated 177k across 3
        // iterations must never be produced by the parser.
        let effective = usage.input_tokens.unwrap_or(0)
            + usage.cache_read_tokens.unwrap_or(0)
            + usage.cache_creation_tokens.unwrap_or(0);
        assert_eq!(effective, 59_886);
    }

    #[test]
    fn assistant_message_without_usage_field_ok() {
        // Older Claude CLI versions may omit usage on assistant messages; the
        // runtime then falls back to result.usage. Make sure parse doesn't
        // reject the line.
        let line = r#"{
            "type": "assistant",
            "session_id": "sess-abc",
            "message": {
                "content": [{"type": "text", "text": "hi"}]
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::AssistantEvents { usage, events } = parsed else {
            panic!("expected AssistantEvents variant");
        };
        assert!(usage.is_none());
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_result_without_usage_block_ok() {
        let line = r#"{
            "type": "result",
            "session_id": "sess-abc",
            "result": "hello",
            "is_error": false
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::Result { usage, .. } = parsed else {
            panic!("expected Result variant");
        };
        assert!(usage.is_none());
    }

    #[test]
    fn zero_usage_assistant_event_does_not_block_result_usage_fallback() {
        // Some Anthropic-compatible Claude Code backends emit a zero-filled
        // assistant usage placeholder, then put the real token counts on the
        // final result event.
        let assistant_line = r#"{
            "type": "assistant",
            "session_id": "sess-glm",
            "message": {
                "content": [{"type": "text", "text": "ok"}],
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0
                }
            }
        }"#;
        let result_line = r#"{
            "type": "result",
            "session_id": "sess-glm",
            "result": "ok",
            "is_error": false,
            "usage": {
                "input_tokens": 51512,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 7,
                "server_tool_use": {
                    "web_search_requests": 0,
                    "web_fetch_requests": 0
                },
                "service_tier": "standard"
            }
        }"#;

        let ParsedMessage::AssistantEvents {
            usage: assistant_usage,
            ..
        } = parse_line(assistant_line).expect("assistant line should parse")
        else {
            panic!("expected AssistantEvents variant");
        };
        assert!(
            !usage_has_reported_tokens(assistant_usage.as_ref().expect("usage is present")),
            "zero placeholder should not count as provider usage"
        );

        let mut captured_usage = None;
        if assistant_usage
            .as_ref()
            .is_some_and(usage_has_reported_tokens)
        {
            captured_usage = assistant_usage;
        }

        let ParsedMessage::Result {
            usage: result_usage,
            ..
        } = parse_line(result_line).expect("result line should parse")
        else {
            panic!("expected Result variant");
        };
        assert!(should_replace_captured_usage(
            &captured_usage,
            &result_usage
        ));
        if should_replace_captured_usage(&captured_usage, &result_usage) {
            captured_usage = result_usage;
        }

        let usage = captured_usage.expect("result usage should be captured");
        assert_eq!(usage.input_tokens, Some(51_512));
        assert_eq!(usage.output_tokens, Some(7));
    }

    #[test]
    fn nonzero_assistant_usage_stays_authoritative_over_result_usage() {
        let assistant_usage = Some(ProviderUsage {
            input_tokens: Some(1),
            output_tokens: Some(34),
            cache_read_tokens: Some(59_560),
            cache_creation_tokens: Some(325),
            ..ProviderUsage::default()
        });
        let result_usage = Some(ProviderUsage {
            input_tokens: Some(120_000),
            output_tokens: Some(80),
            cache_read_tokens: Some(60_000),
            cache_creation_tokens: Some(500),
            ..ProviderUsage::default()
        });

        assert!(!should_replace_captured_usage(
            &assistant_usage,
            &result_usage
        ));
    }
}
