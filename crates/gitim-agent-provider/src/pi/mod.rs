use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError,
    ProviderUsage, ProviderUsageReport, Session, preconditions,
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

        let stdout = preconditions::take_tokio_piped_stdout(&mut child);
        let stdin = preconditions::take_tokio_piped_stdin(&mut child);
        let stderr = preconditions::take_tokio_piped_stderr(&mut child);

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
    // Pi emits one `turn_end` per assistant turn. A single GitIM prompt can
    // loop through many assistant turns when the model uses tools, and Pi's
    // own stats add all of them. Keep billing counters cumulative for the
    // result, while preserving the final turn as the live context signal.
    let mut accumulated_usage: Option<ProviderUsage> = None;
    let mut latest_context_usage: Option<ProviderUsage> = None;

    // Collect stderr tail for error reporting.
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "pi:stderr", "{}", line);
            let mut tail = preconditions::mutex_lock_arc(&stderr_tail_clone);
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
            usage_report: ProviderUsageReport::default(),
            usage: None, // prompt never made it out — no usage to report
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
                                Some(PiEvent::ThinkingDelta { content }) => {
                                    try_send_event(&event_tx, Event::Thinking { content });
                                }
                                Some(PiEvent::ToolExecStart { tool, call_id, input }) => {
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool,
                                        call_id,
                                        input,
                                    });
                                }
                                Some(PiEvent::ToolExecEnd { call_id, output: tool_out }) => {
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
                                Some(PiEvent::TurnEnd { stop_reason, usage }) => {
                                    if stop_reason.as_deref() == Some("aborted") {
                                        final_status = ExecStatus::Aborted;
                                        final_error = Some("cancelled by steering".to_string());
                                    }
                                    if let Some(u) = usage {
                                        accumulate_pi_usage(&mut accumulated_usage, &u);
                                        latest_context_usage = Some(u);
                                    }
                                }
                                Some(PiEvent::AgentEnd) => {
                                    // Execution complete. Send get_state to capture full sessionId.
                                    if let Err(e) = stdin.write_all(build_get_state_command()).await {
                                        warn!(pid, error = %e, "failed to send get_state");
                                    }
                                    // Continue reading for the getState response.
                                }
                                Some(PiEvent::AutoRetryFailed { final_error: err }) => {
                                    // Sticky failure — first auto-retry exhaustion wins, so
                                    // a stray success later doesn't paper over the error.
                                    let msg = err.unwrap_or_else(|| {
                                        "pi exhausted automatic retries".to_string()
                                    });
                                    warn!(pid, error = %msg, "pi auto-retry failed");
                                    if final_status == ExecStatus::Completed {
                                        final_status = ExecStatus::Failed;
                                        final_error = Some(msg.clone());
                                    }
                                    try_send_event(&event_tx, Event::Error { content: msg });
                                    // Keep reading — pi may still emit turn_end / agent_end
                                    // after retries fail, and we want a clean session shutdown.
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
                                Some(PiEvent::RpcError { command, error }) => {
                                    let message = format!("pi RPC {command} failed: {error}");
                                    warn!(pid, %message);
                                    final_status = ExecStatus::Failed;
                                    final_error = Some(message.clone());
                                    try_send_event(&event_tx, Event::Error { content: message });
                                    break;
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
        let tail = preconditions::mutex_lock_arc(&stderr_tail);
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

    let billing_usage = accumulated_usage;
    let context_usage = latest_context_usage;
    let usage = finalize_pi_usage(billing_usage.clone(), context_usage.as_ref());
    let usage_report = ProviderUsageReport::new(billing_usage, context_usage);
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
    // SAFETY: `serde_json::to_vec` with a static struct literal can only fail on
    // programming errors (e.g., Serialize not implemented). This is a bug if it panics.
    let mut buf = preconditions::static_json_to_vec(&serde_json::json!({
        "type": "prompt",
        "message": prompt,
    }));
    buf.push(b'\n');
    buf
}

fn build_get_state_command() -> &'static [u8] {
    b"{\"type\":\"get_state\"}\n"
}

/// Parsed event from a single Pi RPC stdout line.
///
/// Schema source of truth: `@mariozechner/pi-coding-agent` `AgentEvent` (RPC
/// mode serializes the agent's `session.subscribe` events verbatim to stdout).
/// Tool calls travel through the **top-level** `tool_execution_*` events, not
/// through any `assistantMessageEvent.tool_*` shape — the latter does not
/// exist in pi-ai's `AssistantMessageEvent` union (which uses `toolcall_*` as
/// streaming markers for the model's incremental JSON-arg emission, with the
/// full `toolCall` object only on `toolcall_end`).
#[derive(Debug)]
enum PiEvent {
    AgentStart,
    TextDelta {
        content: String,
    },
    ThinkingDelta {
        content: String,
    },
    ToolExecStart {
        tool: String,
        call_id: String,
        input: Value,
    },
    ToolExecEnd {
        call_id: String,
        output: String,
    },
    TurnEnd {
        stop_reason: Option<String>,
        usage: Option<ProviderUsage>,
    },
    AgentEnd,
    /// Emitted only when pi's automatic retry on a transient error has
    /// exhausted all attempts (`auto_retry_end {success: false}`). The
    /// `success: true` shape is dropped by `parse_event` since it carries
    /// no actionable info — the next turn will just resume normally.
    AutoRetryFailed {
        final_error: Option<String>,
    },
    GetStateResponse {
        session_id: String,
    },
    AbortResponse {
        success: bool,
    },
    RpcError {
        command: String,
        error: String,
    },
}

fn parse_event(line: &str) -> Option<PiEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let event_type = v.get("type")?.as_str()?;

    match event_type {
        "agent_start" => Some(PiEvent::AgentStart),
        "agent_end" => Some(PiEvent::AgentEnd),

        "message_update" => {
            // pi-ai `AssistantMessageEvent` union; we only surface the two
            // delta variants that contribute to user-visible state. Streaming
            // markers (`*_start`/`*_end`, `start`, `done`, `error`) and the
            // model's incremental tool-call JSON (`toolcall_*`) are skipped —
            // the actual tool invocation is captured by top-level
            // `tool_execution_*` events below, which carry the assembled
            // args / result without us having to reassemble JSON deltas.
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
                "thinking_delta" => {
                    let delta = ae.get("delta")?.as_str()?.to_string();
                    if delta.is_empty() {
                        None
                    } else {
                        Some(PiEvent::ThinkingDelta { content: delta })
                    }
                }
                _ => None,
            }
        }

        "tool_execution_start" => {
            let tool = v.get("toolName")?.as_str()?.to_string();
            let call_id = v.get("toolCallId")?.as_str()?.to_string();
            let input = v
                .get("args")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            Some(PiEvent::ToolExecStart {
                tool,
                call_id,
                input,
            })
        }

        "tool_execution_end" => {
            let call_id = v.get("toolCallId")?.as_str()?.to_string();
            let output = v
                .get("result")
                .map(extract_tool_result_text)
                .unwrap_or_default();
            Some(PiEvent::ToolExecEnd { call_id, output })
        }

        "auto_retry_end" => {
            // pi retries transient LLM errors (rate limits, 5xx) automatically.
            // success=true means the retry worked and the session continues
            // normally — no UI signal needed. success=false means all attempts
            // exhausted; we surface the failure so the runtime can mark the
            // turn as Failed and the user sees an error in the activity feed.
            if v.get("success").and_then(Value::as_bool) != Some(false) {
                return None;
            }
            let final_error = v
                .get("finalError")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
            Some(PiEvent::AutoRetryFailed { final_error })
        }

        "turn_end" => {
            let message = v.get("message");
            let stop_reason = message
                .and_then(|m| m.get("stopReason"))
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let usage = message
                .and_then(|m| m.get("usage"))
                .and_then(parse_pi_usage);
            Some(PiEvent::TurnEnd { stop_reason, usage })
        }

        "response" => {
            let command = v.get("command")?.as_str()?;
            if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
                let error = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("RPC command failed")
                    .to_string();
                return Some(PiEvent::RpcError {
                    command: command.to_string(),
                    error,
                });
            }
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

/// Project-wide convention for extracting a tool result into a string —
/// passthrough for primitive strings, JSON-stringify for everything else.
/// Matches the Claude, OpenCode, and OpenClaw providers.
///
/// The string is consumed by `agent_loop`'s `assistant_text_buf` for the
/// tiktoken context-window estimate only — it is not surfaced to the UI —
/// so faithful preservation of `result`'s wire shape beats per-block text
/// extraction. Pi structures the value as `AgentToolResult` (content blocks
/// + details) but we treat it opaquely.
fn extract_tool_result_text(result: &Value) -> String {
    let Some(content) = result.get("content") else {
        return String::new();
    };
    match content {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Map Pi's `usage` object onto the provider-agnostic `ProviderUsage`.
///
/// Pi (via `@mariozechner/pi-ai`) emits camelCase, flat:
/// `{ input, output, cacheRead, cacheWrite, totalTokens, cost: {...} }`.
///
/// Field mapping:
/// - `input` → `input_tokens` (Anthropic semantics: tokens NOT served from cache)
/// - `output` → `output_tokens`
/// - `cacheRead` → `cache_read_tokens`
/// - `cacheWrite` → `cache_creation_tokens`
/// - `totalTokens` → `context_tokens` (latest-turn context signal)
/// - `cost` is dropped — GitIM tracks tokens, not money.
///
/// Returns `None` if the value isn't an object or every numeric field is
/// missing — pi sometimes emits an empty `usage: {}` on degenerate paths.
fn parse_pi_usage(v: &Value) -> Option<ProviderUsage> {
    let obj = v.as_object()?;
    let input = obj.get("input").and_then(Value::as_u64);
    let output = obj.get("output").and_then(Value::as_u64);
    let cache_read = obj.get("cacheRead").and_then(Value::as_u64);
    let cache_write = obj.get("cacheWrite").and_then(Value::as_u64);
    let total_tokens = obj.get("totalTokens").and_then(Value::as_u64);
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    Some(ProviderUsage {
        input_tokens: input,
        output_tokens: output,
        used_percent: None,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_write,
        context_tokens: total_tokens.or_else(|| {
            Some(
                input
                    .unwrap_or(0)
                    .saturating_add(output.unwrap_or(0))
                    .saturating_add(cache_read.unwrap_or(0))
                    .saturating_add(cache_write.unwrap_or(0)),
            )
        }),
        context_window_tokens: None,
    })
}

fn accumulate_pi_usage(accumulated: &mut Option<ProviderUsage>, next: &ProviderUsage) {
    let usage = accumulated.get_or_insert_with(ProviderUsage::default);
    usage.input_tokens = add_optional_tokens(usage.input_tokens, next.input_tokens);
    usage.output_tokens = add_optional_tokens(usage.output_tokens, next.output_tokens);
    usage.cache_read_tokens = add_optional_tokens(usage.cache_read_tokens, next.cache_read_tokens);
    usage.cache_creation_tokens =
        add_optional_tokens(usage.cache_creation_tokens, next.cache_creation_tokens);
}

fn add_optional_tokens(current: Option<u64>, next: Option<u64>) -> Option<u64> {
    if current.is_none() && next.is_none() {
        return None;
    }
    Some(current.unwrap_or(0).saturating_add(next.unwrap_or(0)))
}

fn finalize_pi_usage(
    mut accumulated: Option<ProviderUsage>,
    latest_context: Option<&ProviderUsage>,
) -> Option<ProviderUsage> {
    if let (Some(accumulated), Some(latest_context)) = (accumulated.as_mut(), latest_context) {
        accumulated.context_tokens = latest_context.context_tokens;
        accumulated.context_window_tokens = latest_context.context_window_tokens;
    }
    accumulated
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
        let PiEvent::TurnEnd { stop_reason, .. } = event else {
            panic!("expected TurnEnd");
        };
        assert_eq!(stop_reason.as_deref(), Some("aborted"));
    }

    #[test]
    fn parse_turn_end_stop() {
        let line = r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"stop"},"toolResults":[]}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TurnEnd { stop_reason, .. } = event else {
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
    fn prompt_command_uses_message_field() {
        let command = build_prompt_command("hello");
        let parsed: Value = serde_json::from_slice(&command).expect("json command");

        assert_eq!(parsed["type"], "prompt");
        assert_eq!(parsed["message"], "hello");
        assert!(parsed.get("text").is_none());
    }

    #[test]
    fn get_state_command_uses_snake_case_name() {
        let command = build_get_state_command();
        let parsed: Value = serde_json::from_slice(command).expect("json command");

        assert_eq!(parsed["type"], "get_state");
    }

    #[test]
    fn parse_turn_end_with_usage() {
        // Real Pi turn_end shape: message.usage carries camelCase Pi-AI fields.
        let line = r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"api":"openai","provider":"openai","model":"gpt-4o-mini","usage":{"input":150,"output":200,"cacheRead":12000,"cacheWrite":300,"totalTokens":12650,"cost":{"input":0.001,"output":0.0006,"cacheRead":0,"cacheWrite":0,"total":0.0016}},"stopReason":"stop"},"toolResults":[]}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TurnEnd { stop_reason, usage } = event else {
            panic!("expected TurnEnd");
        };
        assert_eq!(stop_reason.as_deref(), Some("stop"));
        let usage = usage.expect("usage extracted");
        assert_eq!(usage.input_tokens, Some(150));
        assert_eq!(usage.output_tokens, Some(200));
        assert_eq!(usage.cache_read_tokens, Some(12_000));
        assert_eq!(usage.cache_creation_tokens, Some(300));
        assert!(
            usage.used_percent.is_none(),
            "compute_snapshot derives the percent — pi never sets it"
        );
    }

    #[test]
    fn parse_turn_end_without_usage_field_yields_none_usage() {
        // Defensive: older Pi versions / aborted turns may omit `usage`.
        let line = r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"stop"},"toolResults":[]}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TurnEnd { usage, .. } = event else {
            panic!("expected TurnEnd");
        };
        assert!(usage.is_none());
    }

    #[test]
    fn parse_turn_end_with_empty_usage_object_yields_none() {
        // Pi has been seen to emit `usage: {}` on degenerate paths — that
        // shouldn't trigger a 0% snapshot, it should fall through to estimate.
        let line = r#"{"type":"turn_end","message":{"usage":{},"stopReason":"stop"}}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::TurnEnd { usage, .. } = event else {
            panic!("expected TurnEnd");
        };
        assert!(usage.is_none());
    }

    #[test]
    fn parse_pi_usage_partial_input_only() {
        // Streaming providers sometimes report only input on first chunk;
        // missing fields should stay None rather than coerce to 0.
        let usage = parse_pi_usage(&serde_json::json!({"input": 42})).expect("partial usage");
        assert_eq!(usage.input_tokens, Some(42));
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.cache_read_tokens, None);
        assert_eq!(usage.cache_creation_tokens, None);
    }

    #[test]
    fn parse_rpc_error_response() {
        let line = r#"{"type":"response","command":"prompt","success":false,"error":"Cannot read properties of undefined (reading 'startsWith')"}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::RpcError { command, error } = event else {
            panic!("expected RpcError");
        };
        assert_eq!(command, "prompt");
        assert!(error.contains("startsWith"));
    }

    // ── tool execution lifecycle (top-level events, NOT nested in assistantMessageEvent) ──

    #[test]
    fn parse_tool_execution_start_extracts_name_id_and_args() {
        // Real shape per pi-coding-agent docs/rpc.md §tool_execution_*.
        let line = r#"{"type":"tool_execution_start","toolCallId":"call_abc123","toolName":"bash","args":{"command":"ls -la"}}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::ToolExecStart {
            tool,
            call_id,
            input,
        } = event
        else {
            panic!("expected ToolExecStart, got {event:?}");
        };
        assert_eq!(tool, "bash");
        assert_eq!(call_id, "call_abc123");
        assert_eq!(input["command"], "ls -la");
    }

    #[test]
    fn parse_tool_execution_start_missing_args_yields_empty_object() {
        // Defensive: pi-ai tools without arguments emit an empty `args` or omit it.
        let line = r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"now"}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::ToolExecStart { input, .. } = event else {
            panic!("expected ToolExecStart");
        };
        assert!(input.is_object(), "args defaults to empty object");
    }

    #[test]
    fn parse_tool_execution_end_stringifies_content_array() {
        // Matches the project-wide pattern: result.content (a Pi
        // (TextContent | ImageContent)[]) is preserved as raw JSON so the
        // assistant_text_buf token estimate sees the same wire bytes the
        // model emitted. Per-block text extraction was over-engineering.
        let line = r#"{"type":"tool_execution_end","toolCallId":"call_abc123","toolName":"bash","result":{"content":[{"type":"text","text":"first chunk\n"},{"type":"text","text":"second chunk"}],"details":{}},"isError":false}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::ToolExecEnd { call_id, output } = event else {
            panic!("expected ToolExecEnd");
        };
        assert_eq!(call_id, "call_abc123");
        // serde_json's compact serialization of the content array.
        assert_eq!(
            output,
            r#"[{"text":"first chunk\n","type":"text"},{"text":"second chunk","type":"text"}]"#
        );
    }

    #[test]
    fn parse_tool_execution_end_preserves_mixed_text_and_image_content() {
        // Image blocks are kept in the raw JSON — matches Claude/OpenCode
        // behavior where the LLM sees structured content blocks and we
        // don't strip them on the way to the token estimator.
        let line = r#"{"type":"tool_execution_end","toolCallId":"c2","toolName":"screenshot","result":{"content":[{"type":"text","text":"saved"},{"type":"image","data":"abc","mimeType":"image/png"}]},"isError":false}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::ToolExecEnd { output, .. } = event else {
            panic!("expected ToolExecEnd");
        };
        assert!(output.contains(r#""text":"saved""#));
        assert!(output.contains(r#""type":"image""#));
    }

    #[test]
    fn parse_tool_execution_end_missing_content_yields_empty_string() {
        let line = r#"{"type":"tool_execution_end","toolCallId":"c3","toolName":"noop","result":{"details":{}},"isError":false}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::ToolExecEnd { output, .. } = event else {
            panic!("expected ToolExecEnd");
        };
        assert_eq!(output, "");
    }

    #[test]
    fn parse_tool_execution_update_is_skipped() {
        // tool_execution_update streams partial output (e.g. bash stdout as it
        // arrives). Surfacing each chunk would flood the activity feed, and
        // partialResult is cumulative not incremental — tool_execution_end
        // carries the final result we already capture.
        let line = r#"{"type":"tool_execution_update","toolCallId":"c1","toolName":"bash","args":{"command":"ls"},"partialResult":{"content":[{"type":"text","text":"so far"}],"details":{}}}"#;
        assert!(parse_event(line).is_none());
    }

    // ── thinking ──

    #[test]
    fn parse_thinking_delta_extracts_content() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"hmm, considering options"},"message":{}}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::ThinkingDelta { content } = event else {
            panic!("expected ThinkingDelta, got {event:?}");
        };
        assert_eq!(content, "hmm, considering options");
    }

    #[test]
    fn parse_empty_thinking_delta_returns_none() {
        // Same posture as text_delta — empty deltas are noise.
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":""},"message":{}}"#;
        assert!(parse_event(line).is_none());
    }

    // ── streaming markers we intentionally drop ──

    #[test]
    fn parse_toolcall_streaming_events_are_skipped() {
        // pi-ai emits `toolcall_start/delta/end` as the model streams its tool-
        // call args as JSON deltas. We don't need them — `tool_execution_start`
        // gives us the assembled args downstream. Each form must return None.
        let start = r#"{"type":"message_update","assistantMessageEvent":{"type":"toolcall_start","contentIndex":1},"message":{}}"#;
        let delta = r#"{"type":"message_update","assistantMessageEvent":{"type":"toolcall_delta","contentIndex":1,"delta":"\"command\":\"ls\""},"message":{}}"#;
        let end = r#"{"type":"message_update","assistantMessageEvent":{"type":"toolcall_end","contentIndex":1,"toolCall":{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}},"message":{}}"#;
        assert!(parse_event(start).is_none(), "toolcall_start dropped");
        assert!(parse_event(delta).is_none(), "toolcall_delta dropped");
        assert!(parse_event(end).is_none(), "toolcall_end dropped");
    }

    #[test]
    fn parse_text_and_thinking_boundary_events_are_skipped() {
        // text_start/end, thinking_start/end, and the `start` marker are
        // streaming scaffolding — only the *_delta variants carry content.
        for line in [
            r#"{"type":"message_update","assistantMessageEvent":{"type":"start"},"message":{}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0},"message":{}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"hello"},"message":{}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0},"message":{}}"#,
            r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_end","contentIndex":0,"content":"…"},"message":{}}"#,
        ] {
            assert!(
                parse_event(line).is_none(),
                "expected None for boundary event: {line}"
            );
        }
    }

    #[test]
    fn parse_unknown_top_level_events_are_skipped() {
        // Pi session emits several lifecycle events we don't need to act on:
        // turn_start, message_start/end, queue_update, compaction_*,
        // auto_retry_start, session_info_changed, thinking_level_changed,
        // extension_error. All must parse to None so the read loop continues.
        // `auto_retry_end` is handled separately (see auto_retry_end_* tests).
        for line in [
            r#"{"type":"turn_start"}"#,
            r#"{"type":"message_start","message":{}}"#,
            r#"{"type":"message_end","message":{}}"#,
            r#"{"type":"queue_update","steering":[],"followUp":[]}"#,
            r#"{"type":"compaction_start","reason":"threshold"}"#,
            r#"{"type":"compaction_end","reason":"threshold","aborted":false,"willRetry":false,"result":{}}"#,
            r#"{"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":1000,"errorMessage":"rate limit"}"#,
            r#"{"type":"session_info_changed","name":"x"}"#,
            r#"{"type":"thinking_level_changed","level":"medium"}"#,
            r#"{"type":"extension_error","extensionPath":"x","event":"y","error":"z"}"#,
        ] {
            assert!(
                parse_event(line).is_none(),
                "expected None for event: {line}"
            );
        }
    }

    // ── auto_retry_end: failure surfaces, success is silent ──

    #[test]
    fn parse_auto_retry_end_success_returns_none() {
        // Successful retries are silent — the next turn just continues.
        let line = r#"{"type":"auto_retry_end","success":true,"attempt":2}"#;
        assert!(parse_event(line).is_none());
    }

    #[test]
    fn parse_auto_retry_end_failure_captures_final_error() {
        let line = r#"{"type":"auto_retry_end","success":false,"attempt":3,"finalError":"rate limited by anthropic.com"}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::AutoRetryFailed { final_error } = event else {
            panic!("expected AutoRetryFailed, got {event:?}");
        };
        assert_eq!(
            final_error.as_deref(),
            Some("rate limited by anthropic.com")
        );
    }

    #[test]
    fn parse_auto_retry_end_failure_without_final_error_yields_none_error() {
        // `finalError` is optional in pi's schema — when absent, the
        // drive_session dispatch substitutes a generic message so the user
        // still gets a non-empty Event::Error.
        let line = r#"{"type":"auto_retry_end","success":false,"attempt":3}"#;
        let event = parse_event(line).expect("should parse");
        let PiEvent::AutoRetryFailed { final_error } = event else {
            panic!("expected AutoRetryFailed");
        };
        assert!(final_error.is_none());
    }

    // ── helper unit tests ──

    #[test]
    fn extract_tool_result_text_passthroughs_string_content() {
        // String passthrough — the one case we don't JSON-stringify, so the
        // token estimate doesn't pay for an extra layer of quoting.
        let result = serde_json::json!({"content": "raw text", "details": {}});
        assert_eq!(extract_tool_result_text(&result), "raw text");
    }

    #[test]
    fn extract_tool_result_text_stringifies_non_string_content() {
        // Anything that isn't a string (array, object, number) goes through
        // serde's compact JSON — same posture as the other providers in this
        // crate. Verifying the *shape* (key included) rather than exact bytes
        // since serde may reorder map keys.
        let result = serde_json::json!({"content": {"unexpected": "shape"}});
        let out = extract_tool_result_text(&result);
        assert!(out.starts_with('{') && out.ends_with('}'));
        assert!(out.contains("unexpected"));
        assert!(out.contains("shape"));
    }
}
