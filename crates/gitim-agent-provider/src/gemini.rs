use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct GeminiProvider {
    config: ProviderConfig,
}

impl GeminiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "gemini".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut args = vec![
            "-p".to_string(), prompt.to_string(),
            "--yolo".to_string(),
            "-o".to_string(), "stream-json".to_string(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["-m".to_string(), model.clone()]);
        }
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["-r".to_string(), resume_token.clone()]);
        }

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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "gemini started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(child, stdout, stderr, event_tx, result_tx, timeout, pid, cancel_token_inner).await;
        });

        Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
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
            debug!(target: "gemini:stderr", "{}", line);
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
                                ParsedMessage::Init { session_id: sid } => {
                                    session_id = sid;
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::Text { content } => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                ParsedMessage::ToolUse { tool, call_id, input } => {
                                    try_send_event(&event_tx, Event::ToolUse { tool, call_id, input });
                                }
                                ParsedMessage::ToolResult { call_id, output: tool_output } => {
                                    try_send_event(&event_tx, Event::ToolResult { call_id, output: tool_output });
                                }
                                ParsedMessage::Error { message } => {
                                    // Sticky — once failed, stays failed
                                    if final_status != ExecStatus::Failed {
                                        final_status = ExecStatus::Failed;
                                        final_error = Some(message.clone());
                                    }
                                    try_send_event(&event_tx, Event::Error { content: message });
                                }
                                ParsedMessage::Result { status } => {
                                    saw_result = true;
                                    info!(pid, status, "gemini result received");
                                    if (status == "error" || status == "failed") && final_status != ExecStatus::Failed {
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
        final_error = Some(format!("gemini timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else if !saw_result && final_status == ExecStatus::Completed {
        // Stream ended without a result message — truncated or protocol error
        final_status = ExecStatus::Failed;
        final_error = Some("gemini stream ended without a result message".to_string());
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("gemini exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for gemini: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "gemini finished");

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
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

/// Parsed result from a single line of Gemini stream-json output.
#[derive(Debug)]
pub enum ParsedMessage {
    /// Init message with session ID.
    Init { session_id: String },
    /// Text content from an assistant message.
    Text { content: String },
    /// Tool use request.
    ToolUse { tool: String, call_id: String, input: Value },
    /// Tool result.
    ToolResult { call_id: String, output: String },
    /// Error event.
    Error { message: String },
    /// Final result with status.
    Result { status: String },
}

/// Parse a single line of Gemini stream-json output.
/// Returns None for empty lines, malformed JSON, or unrecognized message types.
pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let raw: RawEvent = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "init" => Some(ParsedMessage::Init {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "message" => Some(ParsedMessage::Text {
            content: raw.content.unwrap_or_default(),
        }),
        "tool_use" => Some(ParsedMessage::ToolUse {
            tool: raw.tool_name.unwrap_or_default(),
            call_id: raw.tool_id.unwrap_or_default(),
            input: raw.parameters.unwrap_or(Value::Object(Default::default())),
        }),
        "tool_result" => Some(ParsedMessage::ToolResult {
            call_id: raw.tool_id.unwrap_or_default(),
            output: raw.output.unwrap_or_default(),
        }),
        "error" => Some(ParsedMessage::Error {
            message: raw.message.unwrap_or_default(),
        }),
        "result" => Some(ParsedMessage::Result {
            status: raw.status.unwrap_or_default(),
        }),
        _ => None,
    }
}

// ── Gemini NDJSON event types (internal) ──

#[derive(Deserialize)]
struct RawEvent {
    r#type: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_id: Option<String>,
    #[serde(default)]
    parameters: Option<Value>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
}
