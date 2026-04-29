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
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct PiProvider {
    config: ProviderConfig,
}

impl PiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for PiProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "pi".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let inv = build_invocation(&opts);

        let mut cmd = Command::new(&exec_path);
        cmd.args(&inv.args)
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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, resume = opts.resume_token.is_some(), "pi started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stdin = child.stdin.take().expect("stdin piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let prompt = prompt.to_string();
        let join_handle = tokio::spawn(async move {
            drive_session(
                child,
                stdout,
                stdin,
                stderr,
                prompt,
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
    prompt: String,
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
    let mut abort_sent = false;

    let mut reader = BufReader::new(stdout).lines();
    let mut stdin = stdin;

    // Collect stderr tail for error reporting.
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "pi:stderr", "{}", line);
            let mut tail = stderr_tail_clone.lock().unwrap();
            tail.push(line);
            if tail.len() > TAIL_LINES {
                tail.remove(0);
            }
        }
    });

    // Send the prompt.
    let prompt_msg = build_prompt_command(&prompt);
    if let Err(e) = stdin.write_all(&prompt_msg).await {
        warn!(pid, error = %e, "failed to write prompt to pi stdin");
        let _ = result_tx.send(ExecResult {
            status: ExecStatus::Failed,
            output: String::new(),
            error: Some(format!("failed to write prompt: {e}")),
            duration_ms: start.elapsed().as_millis() as u64,
            session_token: None,
            usage: None,
        });
        return;
    }

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
                            debug!(pid, line_len = line.len(), "pi stdout");

                            match parse_event(&line) {
                                Some(PiEvent::TextDelta { content }) => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                Some(PiEvent::ToolUse { name, call_id, input }) => {
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool: name,
                                        call_id,
                                        input,
                                    });
                                }
                                Some(PiEvent::ToolResult { call_id, output: tool_out }) => {
                                    try_send_event(&event_tx, Event::ToolResult {
                                        call_id,
                                        output: tool_out,
                                    });
                                }
                                Some(PiEvent::AgentStart) => {
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                Some(PiEvent::TurnEnd { stop_reason }) => {
                                    if stop_reason.as_deref() == Some("aborted") {
                                        final_status = ExecStatus::Aborted;
                                        final_error = Some("cancelled by steering".to_string());
                                    }
                                }
                                Some(PiEvent::AgentEnd) => {
                                    // Execution complete. Send getState to capture full sessionId.
                                    if let Err(e) = stdin.write_all(build_get_state_command()).await {
                                        warn!(pid, error = %e, "failed to send getState");
                                    }
                                    // Continue reading for the getState response.
                                }
                                Some(PiEvent::GetStateResponse { session_id: sid }) => {
                                    session_id = sid;
                                    // We have the session ID — break out of the read loop.
                                    break;
                                }
                                Some(PiEvent::AbortResponse { success }) => {
                                    info!(pid, success, "pi abort response");
                                    // After abort confirmation we still wait for agent_end,
                                    // which was already processed above.
                                }
                                None => {
                                    debug!(pid, "unrecognized pi event line");
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!(pid, error = %e, "pi stdout read error");
                            break;
                        }
                    }
                }
                _ = cancel_token.cancelled(), if !abort_sent => {
                    info!(pid, "cancelling pi via abort command");
                    let abort_msg = b"{\"type\":\"abort\"}\n";
                    if let Err(e) = stdin.write_all(abort_msg).await {
                        warn!(pid, error = %e, "failed to send abort to pi");
                        break;
                    }
                    abort_sent = true;
                    // Continue reading — Pi will send turn_end(aborted) + agent_end.
                }
            }
        }
    })
    .await;

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("pi timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else {
        // Normal termination — kill the long-running RPC process.
        let _ = child.start_kill();
    }

    let duration = start.elapsed();
    info!(
        pid,
        ?final_status,
        has_session = !session_id.is_empty(),
        ?duration,
        "pi finished"
    );

    stderr_handle.abort();

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
        warn!("pi event channel full, dropping event");
    }
}

/// Plan of record for invoking `pi` in RPC mode.
#[derive(Debug)]
pub struct Invocation {
    pub args: Vec<String>,
}

/// Build argv for Pi RPC mode.
///
/// Pi documents provider/model as separate CLI flags. For GitIM config values
/// written as `provider/model`, split only the first `/` and leave the rest as
/// the model id.
pub fn build_invocation(opts: &ExecOptions) -> Invocation {
    let mut args = vec!["--mode".to_string(), "rpc".to_string()];

    if let Some(model) = opts.model.as_deref().filter(|m| !m.is_empty()) {
        if let Some((provider, model_id)) = split_provider_model(model) {
            args.extend(["--provider".to_string(), provider.to_string()]);
            args.extend(["--model".to_string(), model_id.to_string()]);
        } else {
            args.extend(["--model".to_string(), model.to_string()]);
        }
    }

    if opts.resume_token.is_none() {
        if let Some(system_prompt) = opts.system_prompt.as_ref().filter(|s| !s.is_empty()) {
            args.extend(["--append-system-prompt".to_string(), system_prompt.clone()]);
        }
    }

    if let Some(token) = opts.resume_token.as_ref().filter(|s| !s.is_empty()) {
        args.extend(["--session".to_string(), token.clone()]);
    }

    Invocation { args }
}

fn split_provider_model(model: &str) -> Option<(&str, &str)> {
    let (provider, model_id) = model.split_once('/')?;
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some((provider, model_id))
}

fn build_prompt_command(prompt: &str) -> Vec<u8> {
    let mut buf = serde_json::to_vec(&serde_json::json!({
        "type": "prompt",
        "text": prompt,
    }))
    .expect("prompt command serializes");
    buf.push(b'\n');
    buf
}

fn build_get_state_command() -> &'static [u8] {
    b"{\"type\":\"getState\"}\n"
}

/// Parsed event from a single Pi RPC stdout line.
#[derive(Debug)]
enum PiEvent {
    AgentStart,
    TextDelta {
        content: String,
    },
    ToolUse {
        name: String,
        call_id: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        output: String,
    },
    TurnEnd {
        stop_reason: Option<String>,
    },
    AgentEnd,
    GetStateResponse {
        session_id: String,
    },
    AbortResponse {
        success: bool,
    },
}

fn parse_event(line: &str) -> Option<PiEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let event_type = v.get("type")?.as_str()?;

    match event_type {
        "agent_start" => Some(PiEvent::AgentStart),
        "agent_end" => Some(PiEvent::AgentEnd),

        "message_update" => {
            let ae = v.get("assistantMessageEvent")?;
            let sub = ae.get("type")?.as_str()?;
            match sub {
                "text_delta" => {
                    let delta = ae.get("delta")?.as_str()?.to_string();
                    if delta.is_empty() {
                        None
                    } else {
                        Some(PiEvent::TextDelta { content: delta })
                    }
                }
                "tool_start" => {
                    let name = ae
                        .get("tool")
                        .and_then(|t| t.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let call_id = ae
                        .get("tool")
                        .and_then(|t| t.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = ae
                        .get("tool")
                        .and_then(|t| t.get("input"))
                        .cloned()
                        .unwrap_or(Value::Object(Default::default()));
                    Some(PiEvent::ToolUse {
                        name,
                        call_id,
                        input,
                    })
                }
                "tool_end" => {
                    let call_id = ae
                        .get("tool")
                        .and_then(|t| t.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let output = ae
                        .get("tool")
                        .and_then(|t| t.get("result"))
                        .map(|r| match r {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default();
                    Some(PiEvent::ToolResult { call_id, output })
                }
                _ => None,
            }
        }

        "turn_end" => {
            let stop_reason = v
                .get("message")
                .and_then(|m| m.get("stopReason"))
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            Some(PiEvent::TurnEnd { stop_reason })
        }

        "response" => {
            let command = v.get("command")?.as_str()?;
            match command {
                "getState" | "get_state" => {
                    let session_id = v
                        .get("data")
                        .and_then(|d| d.get("sessionId"))
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(PiEvent::GetStateResponse { session_id })
                }
                "abort" => {
                    let success = v.get("success").and_then(|s| s.as_bool()).unwrap_or(false);
                    Some(PiEvent::AbortResponse { success })
                }
                _ => None,
            }
        }

        _ => None,
    }
}

// ── Deserialize helpers (kept minimal — we use Value-based parsing above) ──

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawPiResponse {
    #[serde(rename = "type")]
    r#type: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    data: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"hello"},"message":{}}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TextDelta { content } = event else {
            panic!("expected TextDelta");
        };
        assert_eq!(content, "hello");
    }

    #[test]
    fn parse_agent_end() {
        let line = r#"{"type":"agent_end","messages":[]}"#;
        let event = parse_event(line).expect("should parse");
        assert!(matches!(event, PiEvent::AgentEnd));
    }

    #[test]
    fn parse_get_state_response() {
        let line = r#"{"type":"response","command":"getState","success":true,"data":{"sessionId":"019db56e-1c53-7280-b2eb-886215c9a5e6","messageCount":2}}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::GetStateResponse { session_id } = event else {
            panic!("expected GetStateResponse");
        };
        assert_eq!(session_id, "019db56e-1c53-7280-b2eb-886215c9a5e6");
    }

    #[test]
    fn parse_get_state_response_full_uuid_not_truncated() {
        // Resume token must be the full 36-char opaque UUID, not an 8-char prefix.
        let full_uuid = "019db958-369f-704c-acbe-aa0bb4389471";
        let line = format!(
            r#"{{"type":"response","command":"get_state","success":true,"data":{{"sessionId":"{}"}}}}"#,
            full_uuid
        );
        let event = parse_event(&line).expect("should parse");
        let PiEvent::GetStateResponse { session_id } = event else {
            panic!("expected GetStateResponse");
        };
        assert_eq!(
            session_id.len(),
            36,
            "sessionId must be full UUID, not truncated"
        );
        assert_eq!(session_id, full_uuid);
    }

    #[test]
    fn parse_abort_response() {
        let line = r#"{"type":"response","command":"abort","success":true}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::AbortResponse { success } = event else {
            panic!("expected AbortResponse");
        };
        assert!(success);
    }

    #[test]
    fn parse_turn_end_aborted() {
        let line = r#"{"type":"turn_end","message":{"role":"assistant","content":[],"stopReason":"aborted","errorMessage":"Request was aborted."},"toolResults":[]}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TurnEnd { stop_reason } = event else {
            panic!("expected TurnEnd");
        };
        assert_eq!(stop_reason.as_deref(), Some("aborted"));
    }

    #[test]
    fn parse_turn_end_stop() {
        let line = r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"stop"},"toolResults":[]}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TurnEnd { stop_reason } = event else {
            panic!("expected TurnEnd");
        };
        assert_eq!(stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parse_empty_text_delta_returns_none() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":""},"message":{}}"#;
        assert!(parse_event(line).is_none());
    }

    #[test]
    fn unrecognized_event_type_returns_none() {
        let line = r#"{"type":"message_start","message":{}}"#;
        assert!(parse_event(line).is_none());
    }

    #[test]
    fn build_invocation_splits_provider_model() {
        let opts = ExecOptions {
            model: Some("openai/gpt-4o-mini".to_string()),
            system_prompt: Some("sys".to_string()),
            ..Default::default()
        };
        let inv = build_invocation(&opts);

        assert_eq!(inv.args[0], "--mode");
        assert_eq!(inv.args[1], "rpc");

        let provider_idx = inv
            .args
            .iter()
            .position(|a| a == "--provider")
            .expect("--provider flag");
        assert_eq!(inv.args[provider_idx + 1], "openai");

        let model_idx = inv
            .args
            .iter()
            .position(|a| a == "--model")
            .expect("--model flag");
        assert_eq!(inv.args[model_idx + 1], "gpt-4o-mini");

        let prompt_idx = inv
            .args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("--append-system-prompt flag");
        assert_eq!(inv.args[prompt_idx + 1], "sys");
    }

    #[test]
    fn build_invocation_omits_system_prompt_on_resume() {
        let opts = ExecOptions {
            system_prompt: Some("sys".to_string()),
            resume_token: Some("session-1".to_string()),
            ..Default::default()
        };
        let inv = build_invocation(&opts);

        assert!(!inv.args.iter().any(|a| a == "--append-system-prompt"));

        let session_idx = inv
            .args
            .iter()
            .position(|a| a == "--session")
            .expect("--session flag");
        assert_eq!(inv.args[session_idx + 1], "session-1");
    }

    #[test]
    fn prompt_command_uses_text_field() {
        let command = build_prompt_command("hello");
        let parsed: Value = serde_json::from_slice(&command).expect("json command");

        assert_eq!(parsed["type"], "prompt");
        assert_eq!(parsed["text"], "hello");
        assert!(parsed.get("message").is_none());
    }

    #[test]
    fn get_state_command_uses_documented_name() {
        let command = build_get_state_command();
        let parsed: Value = serde_json::from_slice(command).expect("json command");

        assert_eq!(parsed["type"], "getState");
    }
}
