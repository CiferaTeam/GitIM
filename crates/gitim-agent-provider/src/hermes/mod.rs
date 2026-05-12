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
    Event, ExecOptions, ExecResult, ExecStatus, PromptContext, Provider, ProviderConfig,
    ProviderError, ProviderUsage, Session,
};

pub mod prompts;

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
    /// Hermes compresses in-loop at `compression.threshold` (default 50%).
    /// Opt out of the runtime's occupancy gauge + `[[RESET]]` preamble — see
    /// `Provider::self_managed_context` for the full reasoning.
    fn self_managed_context(&self) -> bool {
        true
    }

    /// Hermes' ACP `result.usage` reports session-cumulative billing
    /// (sum of `session_input_tokens` / `session_output_tokens` /
    /// `session_cache_read_tokens` across every LLM call this hermes
    /// session has made — see `run_agent.py::run_conversation`'s return
    /// dict and `acp_adapter/server.py::prompt`'s Usage construction).
    /// `normalize_to_delta` uses this flag to subtract a per-session
    /// baseline from each turn's reported total so the accumulator gets
    /// real per-turn deltas. compute_snapshot is short-circuited by
    /// `self_managed_context` so the cumulative numbers never feed the
    /// HUD's occupancy gauge.
    fn usage_is_cumulative(&self) -> bool {
        true
    }

    /// Drop sections that assume the Claude-style `AGENTS.md` + `notes/`
    /// filesystem-memory model. Hermes manages identity / memory through
    /// SOUL.md + MEMORY.md / USER.md inside the profile directory, and
    /// re-loads them after every in-loop compression, so re-injecting any
    /// of this from the runtime side would be noise (or worse, conflict
    /// with hermes' own guidance).
    fn prompt_memory(&self, _ctx: &PromptContext) -> String {
        // hermes injects its own MEMORY_GUIDANCE when the memory tool is
        // loaded — see run_agent.py::_build_system_prompt.
        String::new()
    }
    fn prompt_reset_protocol(&self, _ctx: &PromptContext) -> String {
        // hermes self-compresses; no `[[RESET]]` sentinel.
        String::new()
    }
    fn prompt_cold_start(&self, _ctx: &PromptContext) -> String {
        // No bootstrap of AGENTS.md / notes/ for hermes — memory files live
        // inside the hermes profile and are seeded by `hermes_profile`.
        String::new()
    }

    /// Hermes-flavored identity (drops the `AGENTS.md` / `notes/` carve-out).
    fn prompt_identity(&self, ctx: &PromptContext) -> String {
        prompts::identity(ctx)
    }
    /// Hermes-flavored collaboration norms (drops the
    /// "用你的记忆 / `notes/` 跟踪每条线" filesystem-memory suggestion).
    fn prompt_collaboration(&self, ctx: &PromptContext) -> String {
        prompts::collaboration(ctx)
    }
    /// Hermes-flavored gitim CLI guidance (neutralises the AGENTS.md
    /// continuity-sink line in the board / memory contrast).
    fn prompt_gitim_api(&self, ctx: &PromptContext) -> String {
        prompts::gitim_api(ctx)
    }

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

        // `opts.system_prompt` deliberately unused — SOUL.md is the channel.
        let _ = &opts.system_prompt;
        let prompt = build_prompt_payload(prompt);
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
    /// Mid-session token usage push. Hermes emits these as
    /// `sessionUpdate: "usage_update"` with camelCase fields, separately
    /// from the snake_case usage on the final session/prompt response.
    Usage(ProviderUsage),
}

/// Detect a hermes-internal API failure that's been streamed as plain
/// assistant text rather than a JSON-RPC error. Hermes catches LLM API
/// exceptions in its agent loop and turns them into a `final_response`
/// string, so the ACP `session/prompt` reply still looks successful
/// (`stop_reason=end_turn`, no `error` field) — but the agent never
/// actually runs any tools. We have to fall back to substring matching
/// against the stable error prefixes hermes emits, otherwise the
/// runtime reports "done" while the user sees no IM reply.
///
/// Returns the first line of the output (trimmed) when it starts with a
/// known failure prefix; `None` otherwise.
pub fn detect_api_failure(output: &str) -> Option<String> {
    const KNOWN_PREFIXES: &[&str] = &[
        // Botocore retry wrapper around AWS Bedrock / Anthropic / OpenAI
        "API call failed after",
        // Botocore validation
        "Parameter validation failed",
    ];
    let trimmed = output.trim_start();
    for prefix in KNOWN_PREFIXES {
        if trimmed.starts_with(prefix) {
            let line = trimmed.lines().next()?.trim();
            return Some(line.to_string());
        }
    }
    None
}

/// Build the text sent to `session/prompt`.
///
/// Hermes ACP does not expose a per-request system prompt parameter. Earlier
/// versions of this provider prepended the runtime's system prompt to the
/// first user message, but that put GitIM identity / protocol rules into
/// hermes' conversation history — where its in-loop compressor was free to
/// summarise them away.
///
/// The system prompt now lives in `~/.hermes/profiles/gitim-<handler>/SOUL.md`
/// (managed by `gitim_runtime::hermes_profile`), which hermes auto-loads into
/// its frozen system-prompt slot at every session start and rebuilds after
/// each compression event. So the ACP user payload is just the user text —
/// `opts.system_prompt` is intentionally ignored here.
pub fn build_prompt_payload(prompt: &str) -> String {
    prompt.to_string()
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
        "usage_update" => parse_acp_usage(update.get("usage")?).map(ParsedNotification::Usage),
        _ => None,
    }
}

/// Map a Hermes `usage` object to the provider-agnostic `ProviderUsage`.
///
/// Hermes surfaces usage in two distinct shapes:
///
/// 1. **session/prompt response** (id=3) — ACP-spec snake_case:
///    `{input_tokens, output_tokens, cache_read_input_tokens?,
///      cache_creation_input_tokens?}` — Hermes wraps Claude today and
///    relays Anthropic fields verbatim per the Agent Client Protocol.
///
/// 2. **session/update with sessionUpdate=usage_update** — Hermes' own
///    mid-stream push, camelCase: `{inputTokens, outputTokens,
///    cacheReadInputTokens?, cacheCreationInputTokens?}`. The existing
///    `parse_usage_update_returns_none` test fixture documents the
///    camelCase shape; this used to be intentionally ignored.
///
/// Accept both naming conventions in one parser: try snake_case first
/// (it's what the ACP spec mandates), then camelCase as a fallback.
/// Returns `None` when none of the four counts are present, so an empty
/// `usage: {}` doesn't fabricate a 0% snapshot.
/// Test-only re-export so `tests/hermes_usage_semantics_test.rs` can pin the
/// shape of the id=3 prompt-response usage parser. Production code paths still
/// reach `parse_acp_usage` through the private module boundary.
pub fn parse_acp_usage_for_test(v: &Value) -> Option<ProviderUsage> {
    parse_acp_usage(v)
}

fn parse_acp_usage(v: &Value) -> Option<ProviderUsage> {
    let obj = v.as_object()?;
    let pick = |snake: &str, camel: &str| -> Option<u64> {
        obj.get(snake)
            .or_else(|| obj.get(camel))
            .and_then(Value::as_u64)
    };
    let input = pick("input_tokens", "inputTokens");
    let output = pick("output_tokens", "outputTokens");
    let cache_read = pick("cache_read_input_tokens", "cacheReadInputTokens");
    let cache_creation = pick("cache_creation_input_tokens", "cacheCreationInputTokens");
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_creation.is_none() {
        return None;
    }
    Some(ProviderUsage {
        input_tokens: input,
        output_tokens: output,
        used_percent: None,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
    })
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
    // Hermes' id=3 prompt response carries session-cumulative `result.usage`
    // (input / output / cache_read are `run_agent.py` `session_*_tokens`
    // running totals across every LLM call in the resumed ACP session).
    // Capture it as `latest_usage`; the runtime maps these onto per-turn
    // deltas via `Provider::usage_is_cumulative() -> true` +
    // `normalize_to_delta`'s baseline math. We do NOT use these numbers
    // for occupancy — `HermesProvider::self_managed_context() -> true`
    // routes that path through hermes' own compression instead. Billing
    // accumulator gets accurate per-turn values, HUD gauge stays off, no
    // monotonic 100% clamp.
    let mut latest_usage: Option<ProviderUsage> = None;

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
                                } else if let Some(usage_val) = raw.pointer("/result/usage") {
                                    if let Some(u) = parse_acp_usage(usage_val) {
                                        latest_usage = Some(u);
                                    }
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
                                            ParsedNotification::Usage(_) => {
                                                // Drop: mid-stream usage_update is display-only;
                                                // ExecResult.usage must come from the id=3 prompt
                                                // response (per-turn delta) for the runtime token
                                                // accumulator to stay deterministic.
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
    } else if final_status == ExecStatus::Completed {
        if let Some(api_err) = detect_api_failure(&output) {
            warn!(pid, error = %api_err, "hermes returned API failure as text");
            final_status = ExecStatus::Failed;
            final_error = Some(api_err);
        }
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
        usage: latest_usage,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}
