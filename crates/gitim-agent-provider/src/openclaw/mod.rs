use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError,
    ProviderUsageReport, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct OpenclawProvider {
    config: ProviderConfig,
}

impl OpenclawProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for OpenclawProvider {
    fn reports_usage(&self) -> bool {
        false
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "openclaw".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut args = vec![
            "agent".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--yes".to_string(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(system_prompt) = &opts.system_prompt {
            args.extend(["--system-prompt".to_string(), system_prompt.clone()]);
        }
        if let Some(max_turns) = opts.max_turns {
            args.extend(["--max-turns".to_string(), max_turns.to_string()]);
        }
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["--session".to_string(), resume_token.clone()]);
        }
        args.extend(["-p".to_string(), prompt.to_string()]);

        let mut cmd = Command::new(&exec_path);
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "openclaw started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(
                child,
                stdout,
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

    let mut reader = BufReader::new(stdout).lines();

    // Collect stderr tail for error reporting
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "openclaw:stderr", "{}", line);
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
                                ParsedMessage::StepStart { session_id: sid } => {
                                    // Only set session_id on first step_start
                                    if session_id.is_empty() {
                                        session_id = sid;
                                    }
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::Text { content } => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                ParsedMessage::Thinking { content } => {
                                    // Emit thinking event but don't append to output
                                    try_send_event(&event_tx, Event::Thinking { content });
                                }
                                ParsedMessage::ToolCall { name, call_id, input, status, output: tool_output } => {
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool: name, call_id: call_id.clone(), input,
                                    });
                                    if status == "completed" || status == "failed" {
                                        if let Some(out) = tool_output {
                                            try_send_event(&event_tx, Event::ToolResult { call_id, output: out });
                                        }
                                    }
                                }
                                ParsedMessage::Error { message } => {
                                    // Sticky — once failed, stays failed
                                    if final_status != ExecStatus::Failed {
                                        final_status = ExecStatus::Failed;
                                        final_error = Some(message.clone());
                                    }
                                    try_send_event(&event_tx, Event::Error { content: message });
                                }
                                ParsedMessage::Result { is_error } => {
                                    saw_result = true;
                                    info!(pid, is_error, "openclaw result received");
                                    if is_error && final_status != ExecStatus::Failed {
                                        final_status = ExecStatus::Failed;
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
        final_error = Some(format!("openclaw timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else if !saw_result && final_status == ExecStatus::Completed {
        // Stream ended without a result message — truncated or protocol error
        final_status = ExecStatus::Failed;
        final_error = Some("openclaw stream ended without a result message".to_string());
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("openclaw exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for openclaw: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "openclaw finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
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
        usage_report: ProviderUsageReport::default(),
        usage: None,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

/// Parsed result from a single line of OpenClaw stream-json output.
#[derive(Debug)]
pub enum ParsedMessage {
    /// Step start with session ID.
    StepStart { session_id: String },
    /// Text content from an assistant message.
    Text { content: String },
    /// Thinking / reasoning content (not appended to output).
    Thinking { content: String },
    /// Tool call event — combined: carries invocation + optional result when completed.
    ToolCall {
        name: String,
        call_id: String,
        input: Value,
        status: String,
        output: Option<String>,
    },
    /// Error event.
    Error { message: String },
    /// Final result.
    Result { is_error: bool },
}

/// Parse a single line of OpenClaw stream-json output.
/// Returns None for empty lines, malformed JSON, unrecognized types, or known-ignorable types (step_end).
pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let raw: RawEvent = serde_json::from_str(line).ok()?;
    let data = raw.data.unwrap_or_default();

    match raw.r#type.as_str() {
        "step_start" => Some(ParsedMessage::StepStart {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "text" => Some(ParsedMessage::Text {
            content: data.text.unwrap_or_default(),
        }),
        "thinking" => Some(ParsedMessage::Thinking {
            content: data.text.unwrap_or_default(),
        }),
        "tool_call" => {
            let output = data.output.map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            });
            Some(ParsedMessage::ToolCall {
                name: data.name.unwrap_or_default(),
                call_id: data.call_id.unwrap_or_default(),
                input: data.input.unwrap_or(Value::Object(Default::default())),
                status: data.status.unwrap_or_default(),
                output,
            })
        }
        "error" => Some(ParsedMessage::Error {
            message: data.message.unwrap_or_default(),
        }),
        "result" => {
            let status = data.status.unwrap_or_default();
            Some(ParsedMessage::Result {
                is_error: status == "error" || status == "failed",
            })
        }
        // step_end and anything else are intentionally ignored
        _ => None,
    }
}

// ── OpenClaw NDJSON event types (internal) ──

#[derive(Deserialize)]
struct RawEvent {
    r#type: String,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    data: Option<RawData>,
}

#[derive(Deserialize, Default)]
struct RawData {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "callId")]
    call_id: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    message: Option<String>,
}
