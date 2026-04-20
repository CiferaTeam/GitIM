use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

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

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(child, stdout, stdin, stderr, event_tx, result_tx, timeout, pid, cancel_token_inner).await;
        });

        Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
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
            let mut tail = stderr_tail_clone.lock().unwrap();
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
                                ParsedMessage::AssistantEvents(events) => {
                                    num_turns += 1;
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
                                } => {
                                    saw_result = true;
                                    session_id = sid;
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
    info!(pid, ?final_status, turns = num_turns, ?duration, "claude finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail
    if final_status == ExecStatus::Failed
        && final_error.as_ref().is_none_or(|e| e.is_empty())
    {
        let tail = stderr_tail.lock().unwrap();
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

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
        usage: None,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
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
    AssistantEvents(Vec<Event>),
    /// Events from a user message (tool results, not accumulated into output).
    UserEvents(Vec<Event>),
    /// Final result.
    Result {
        session_id: String,
        output: String,
        is_error: bool,
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
                Some(ParsedMessage::AssistantEvents(events))
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
        }),
        "log" => {
            let log = raw.log?;
            Some(ParsedMessage::AssistantEvents(vec![Event::Log {
                level: log.level,
                content: log.message,
            }]))
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
    log: Option<LogEntry>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    request: Option<Value>,
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
