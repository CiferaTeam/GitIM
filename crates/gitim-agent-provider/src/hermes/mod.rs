use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct HermesProvider {
    config: ProviderConfig,
}

impl HermesProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for HermesProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "hermes".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let args = vec!["acp".to_string()];

        let mut cmd = Command::new(&exec_path);
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env("HERMES_YOLO_MODE", "1");

        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(pid, cwd = ?opts.cwd, "hermes started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let prompt = prompt.to_string();
        let resume_token = opts.resume_token.clone();
        let cwd_str = opts
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let join_handle = tokio::spawn(async move {
            drive_session(
                child,
                stdin,
                stdout,
                stderr,
                event_tx,
                result_tx,
                timeout,
                pid,
                cancel_token_inner,
                prompt,
                resume_token,
                cwd_str,
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

// ── Public types for parse tests ──

/// Parsed result from a session/update notification's params object.
#[derive(Debug)]
pub enum ParsedNotification {
    /// Text content from an agent message chunk.
    Text { content: String },
    /// Thinking / reasoning content.
    Thinking { content: String },
    /// Tool invocation.
    ToolCall {
        tool: String,
        call_id: String,
        input: Value,
    },
    /// Tool result (completed or failed).
    ToolResult { call_id: String, output: String },
}

/// Parse the `params` object from a `session/update` JSON-RPC notification.
/// Returns None for unrecognized or ignorable update types.
pub fn parse_notification(params: &Value) -> Option<ParsedNotification> {
    let update = params.get("update")?;
    let update_type = update.get("sessionUpdate")?.as_str()?;

    match update_type {
        "agent_message_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?;
            if text.is_empty() {
                return None;
            }
            Some(ParsedNotification::Text {
                content: text.to_string(),
            })
        }
        "agent_thought_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?;
            if text.is_empty() {
                return None;
            }
            Some(ParsedNotification::Thinking {
                content: text.to_string(),
            })
        }
        "tool_call" => {
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let title = update.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let tool = title
                .split(':')
                .next()
                .unwrap_or("unknown")
                .trim()
                .to_string();
            let input = update
                .get("rawInput")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            Some(ParsedNotification::ToolCall {
                tool,
                call_id,
                input,
            })
        }
        "tool_call_update" => {
            let status = update.get("status")?.as_str()?;
            if status != "completed" && status != "failed" {
                return None;
            }
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let output = update
                .get("rawOutput")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(ParsedNotification::ToolResult { call_id, output })
        }
        _ => None,
    }
}

// ── JSON-RPC types (internal) ──

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}

// ── Session driver ──

#[allow(clippy::too_many_arguments)]
async fn drive_session(
    mut child: tokio::process::Child,
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    timeout: Duration,
    pid: u32,
    cancel_token: CancellationToken,
    prompt: String,
    resume_token: Option<String>,
    cwd_str: String,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;

    let mut reader = BufReader::new(stdout).lines();

    // Collect stderr tail for error reporting
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "hermes:stderr", "{}", line);
            let mut tail = stderr_tail_clone.lock().unwrap();
            tail.push(line);
            if tail.len() > TAIL_LINES {
                tail.remove(0);
            }
        }
    });

    // JSON-RPC helper — sends a request and reads back the response with matching id.
    async fn rpc_call(
        stdin: &mut tokio::process::ChildStdin,
        reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut buf = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        buf.push(b'\n');
        stdin
            .write_all(&buf)
            .await
            .map_err(|e| format!("stdin write: {e}"))?;

        // Read lines until we get response with matching id
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(resp) = serde_json::from_str::<RpcResponse>(&line) {
                        if resp.id == Some(id) {
                            if let Some(err) = resp.error {
                                return Err(format!(
                                    "{method}: {} (code={})",
                                    err.get("message")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown"),
                                    err.get("code").and_then(|v| v.as_i64()).unwrap_or(0)
                                ));
                            }
                            return Ok(resp.result.unwrap_or(Value::Null));
                        }
                    }
                }
                Ok(None) => return Err(format!("{method}: stream ended")),
                Err(e) => return Err(format!("{method}: read error: {e}")),
            }
        }
    }

    // Helper to send ExecResult and return early on handshake failure
    fn send_result(
        result_tx: oneshot::Sender<ExecResult>,
        status: ExecStatus,
        output: String,
        error: Option<String>,
        start: Instant,
        session_id: &str,
    ) {
        let _ = result_tx.send(ExecResult {
            status,
            output,
            error,
            duration_ms: start.elapsed().as_millis() as u64,
            session_token: if session_id.is_empty() {
                None
            } else {
                Some(session_id.to_string())
            },
            usage: None,
        });
    }

    // ── Handshake (30s timeout — separate from the main event loop timeout) ──
    // Note: any session/update notifications arriving during handshake are intentionally
    // dropped. In practice hermes doesn't send them before the prompt response begins.

    let handshake_timeout = timeout.min(Duration::from_secs(30));

    let handshake = async {
        // Step 1: initialize
        let init_result = rpc_call(
            &mut stdin,
            &mut reader,
            0,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": {"name": "gitim-agent-sdk", "version": "0.1.0"},
                "clientCapabilities": {},
            }),
        )
        .await?;

        // Step 2: authenticate using first available auth method (if any)
        if let Some(method_id) = init_result
            .get("authMethods")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|m| m.get("id"))
            .and_then(|id| id.as_str())
        {
            rpc_call(
                &mut stdin,
                &mut reader,
                1,
                "authenticate",
                json!({"methodId": method_id}),
            )
            .await?;
        }

        // Step 3: session/new or session/resume
        // session/resume returns only {models} — session_id stays the same as the token passed in.
        // session/new returns {sessionId, models}.
        let sid = if let Some(ref token) = resume_token {
            rpc_call(
                &mut stdin,
                &mut reader,
                2,
                "session/resume",
                json!({"cwd": cwd_str, "sessionId": token}),
            )
            .await?;
            token.clone()
        } else {
            let result = rpc_call(
                &mut stdin,
                &mut reader,
                2,
                "session/new",
                json!({"cwd": cwd_str, "mcpServers": []}),
            )
            .await?;
            result
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };

        // Step 4: send session/prompt (fire and forget — response arrives in event loop)
        let prompt_req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": sid, "prompt": [{"type": "text", "text": prompt}]}
        });
        let mut buf = serde_json::to_vec(&prompt_req).map_err(|e| e.to_string())?;
        buf.push(b'\n');
        stdin
            .write_all(&buf)
            .await
            .map_err(|e| format!("stdin write: {e}"))?;

        Ok::<String, String>(sid)
    };

    match tokio::time::timeout(handshake_timeout, handshake).await {
        Ok(Ok(sid)) => {
            session_id = sid;
            info!(pid, session_id = %session_id, "hermes session established");
        }
        Ok(Err(e)) => {
            warn!(pid, error = %e, "hermes handshake failed");
            let _ = child.start_kill();
            send_result(
                result_tx,
                ExecStatus::Failed,
                output,
                Some(e),
                start,
                &session_id,
            );
            stderr_handle.abort();
            return;
        }
        Err(_) => {
            warn!(
                pid,
                "hermes handshake timed out after {handshake_timeout:?}"
            );
            let _ = child.start_kill();
            send_result(
                result_tx,
                ExecStatus::Timeout,
                output,
                Some(format!(
                    "hermes handshake timed out after {handshake_timeout:?}"
                )),
                start,
                &session_id,
            );
            stderr_handle.abort();
            return;
        }
    }

    try_send_event(
        &event_tx,
        Event::Status {
            status: "running".to_string(),
        },
    );

    // ── Event loop: read notifications + final prompt response ──

    let read_result = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                line = reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() { continue; }

                            let raw: Value = match serde_json::from_str(&line) {
                                Ok(v) => v,
                                Err(_) => {
                                    debug!(pid, line_len = line.len(), "unparsed line");
                                    continue;
                                }
                            };

                            // Prompt response (id=3) — final result
                            if raw.get("id").and_then(|v| v.as_u64()) == Some(3) {
                                if raw.get("error").is_some() {
                                    final_status = ExecStatus::Failed;
                                    final_error = Some(
                                        raw["error"]["message"]
                                            .as_str()
                                            .unwrap_or("prompt failed")
                                            .to_string(),
                                    );
                                }
                                break;
                            }

                            // Notification: session/update
                            if raw.get("method").and_then(|v| v.as_str()) == Some("session/update") {
                                if let Some(params) = raw.get("params") {
                                    if let Some(parsed) = parse_notification(params) {
                                        match parsed {
                                            ParsedNotification::Text { content } => {
                                                output.push_str(&content);
                                                try_send_event(&event_tx, Event::Text { content });
                                            }
                                            ParsedNotification::Thinking { content } => {
                                                try_send_event(&event_tx, Event::Thinking { content });
                                            }
                                            ParsedNotification::ToolCall { tool, call_id, input } => {
                                                try_send_event(&event_tx, Event::ToolUse { tool, call_id, input });
                                            }
                                            ParsedNotification::ToolResult { call_id, output: tool_output } => {
                                                try_send_event(&event_tx, Event::ToolResult { call_id, output: tool_output });
                                            }
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

    // ── Post-loop cleanup ──

    // Drop stdin to signal EOF to hermes
    drop(stdin);

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("hermes timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("hermes exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for hermes: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "hermes finished");

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
        usage: None,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}
