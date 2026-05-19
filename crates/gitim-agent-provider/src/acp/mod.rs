//! ACP (Agent Client Protocol) JSON-RPC transport shared by the
//! `hermes` and `kimi` providers.
//!
//! ## Lifecycle
//!
//! An [`AcpClient`] binds to one provider sub-process' stdin (write side)
//! while the caller drives a reader task that pumps every stdout line
//! through [`AcpClient::handle_line`]. The client never owns the child
//! itself — that stays in the provider's `execute()`, where `kill_on_drop`
//! and stderr collection live. When `execute()` returns, the client is
//! dropped along with the child; nothing is retained across turns. Cross-
//! turn session state (the opaque `session_token`) lives in the runtime,
//! not in the client.
//!
//! See `docs/plans/kimi-cursor-providers/00-requirements.md`
//! §"会话管理模型(重要前提)" for the full reasoning behind the per-`execute()`
//! lifetime.
//!
//! ## Concurrency model
//!
//! Two tokio tasks share an [`AcpClient`]:
//! - The **reader** repeatedly calls [`AcpClient::handle_line`] for each
//!   line read off the child's stdout. `handle_line` routes responses to
//!   pending one-shot senders, and notifications to `Event::*` emitted on
//!   `event_tx`.
//! - The **driver** calls [`AcpClient::initialize`] → `new_session` /
//!   `resume_session` → optional `set_session_model` → `prompt` (each of
//!   which awaits a one-shot response posted by the reader task).
//!
//! Internal state is guarded by `tokio::sync::Mutex` so both tasks can
//! hold an `&Arc<AcpClient>` without ownership contention. `stdin` writes
//! are serialized through its own mutex.
//!
//! See `multica/server/pkg/agent/hermes.go::hermesClient` for the Go
//! reference this is modelled on (read-only cross-check).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, warn};

use crate::{Event, ProviderError, ProviderUsage};

pub mod parse;

use parse::{parse_acp_usage, parse_notification, ParsedNotification};

/// Caller-provided customization points that differentiate hermes/kimi
/// while sharing the transport.
pub struct AcpHooks {
    /// Map an ACP tool title (e.g. `"Read file: …"` for kimi, raw `"terminal"`
    /// for hermes) into the snake_case identifier the runtime/UI expects.
    /// `parse_notification` already strips everything after the first `:`
    /// and trims; the mapper sees only the prefix and may further normalize
    /// (e.g. kimi `"Read file"` -> `"read_file"`).
    pub tool_name_mapper: fn(&str) -> String,
    /// `None` (the v1 default for hermes and kimi) lets every `session/update`
    /// notification through. `Some(pred)` returns `false` for notifications
    /// that should be dropped — kept on the type so kiro-style "current-turn
    /// only" filtering can be plugged in later without a breaking change.
    pub accept_notification: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Whether mid-stream `usage_update` notifications should be turned
    /// into `Event::Usage` for live HUD/SSE consumers. Hermes sets this
    /// to `false` because its `ExecResult.usage` must come from the
    /// session/prompt response (a deterministic per-turn delta);
    /// emitting mid-stream events would risk callers double-counting
    /// against the runtime token accumulator. Kimi may set this to
    /// `true` once it has a live-usage consumer.
    pub emit_live_usage: bool,
}

impl AcpHooks {
    /// Identity mapper — used when the provider's tool titles are already
    /// the canonical name.
    pub fn identity_tool_mapper(name: &str) -> String {
        name.to_string()
    }
}

/// Per-`execute()` ACP transport bound to one provider sub-process'
/// stdin. **Not** retained across turns.
pub struct AcpClient {
    provider_name: &'static str,
    stdin: Mutex<ChildStdin>,
    next_id: Mutex<i64>,
    pending: Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>,
    /// Tracks tool calls between their `tool_call` and `tool_call_update`
    /// notifications. Hermes usually sends complete `rawInput` up front;
    /// Kimi streams argument JSON across `tool_call_update` frames, so we
    /// buffer until completion before emitting the UI-facing `ToolUse`.
    pending_tools: Mutex<HashMap<String, PendingToolCall>>,
    /// Cumulative usage observed via `session/update` notifications during
    /// the current `execute()`. The final `session/prompt` response carries
    /// the authoritative cumulative snapshot which the driver uses for
    /// `ExecResult.usage`; this accumulator is only kept so a future caller
    /// can drain mid-stream pushes if they need a synthesized total.
    usage: Mutex<ProviderUsage>,
    /// All text content emitted by `agent_message_chunk` notifications,
    /// concatenated in arrival order. Drained by the driver via
    /// `collected_output` at session end to populate `ExecResult.output`.
    text_accumulator: Mutex<String>,
    /// Session id assigned by `session/new` or `session/resume`. Cached so
    /// `handle_line` can embed it in `Event::Usage` without the driver
    /// passing it in for every line.
    current_session_id: Mutex<Option<String>>,
    hooks: AcpHooks,
}

/// Response carried by the per-request one-shot. Either a JSON-RPC
/// `result` value or the `error` object decomposed into code + message.
#[derive(Debug)]
pub enum JsonRpcResponse {
    Ok(Value),
    Err { code: i64, message: String },
}

/// State a single tool call carries between its initiating `tool_call`
/// notification and the matching `tool_call_update` completion. Crate-
/// private because callers only need the normalized `Event::ToolUse` /
/// `Event::ToolResult` stream.
#[derive(Debug)]
pub(crate) struct PendingToolCall {
    pub tool: String,
    pub input: Option<Value>,
    pub args_text: String,
    pub emitted: bool,
}

/// Terminal outcome of `session/prompt` — captures the stop_reason and
/// the cumulative usage snapshot the response carries.
#[derive(Debug)]
pub struct PromptOutcome {
    /// ACP `stopReason` field — `"end_turn"`, `"cancelled"`,
    /// `"max_tokens"`, …. Callers map this onto [`crate::ExecStatus`].
    pub stop_reason: String,
    /// Per-response usage snapshot (cumulative for hermes/kimi). `None`
    /// when the response carried no `usage` field — caller should leave
    /// `ExecResult.usage` as `None` in that case rather than fabricating a
    /// zero delta.
    pub usage: Option<ProviderUsage>,
}

impl AcpClient {
    pub fn new(provider_name: &'static str, stdin: ChildStdin, hooks: AcpHooks) -> Self {
        Self {
            provider_name,
            stdin: Mutex::new(stdin),
            next_id: Mutex::new(0),
            pending: Mutex::new(HashMap::new()),
            pending_tools: Mutex::new(HashMap::new()),
            usage: Mutex::new(ProviderUsage::default()),
            text_accumulator: Mutex::new(String::new()),
            current_session_id: Mutex::new(None),
            hooks,
        }
    }

    /// Send a JSON-RPC request, await its response. The caller must run
    /// a reader task in parallel calling [`Self::handle_line`] for each
    /// stdout line so responses can land on the registered one-shot.
    ///
    /// On a JSON-RPC `error` object, returns
    /// `ProviderError::Protocol("<method>: <message> (code=<n>)")`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let cur = *next;
            *next += 1;
            cur
        };

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let payload = match serde_json::to_vec(&req) {
            Ok(mut v) => {
                v.push(b'\n');
                v
            }
            Err(e) => {
                self.pending.lock().await.remove(&id);
                return Err(ProviderError::Protocol(format!(
                    "{}: failed to serialize {method}: {e}",
                    self.provider_name
                )));
            }
        };

        {
            let mut stdin = self.stdin.lock().await;
            if let Err(e) = stdin.write_all(&payload).await {
                self.pending.lock().await.remove(&id);
                return Err(ProviderError::Protocol(format!(
                    "{}: stdin write failed for {method}: {e}",
                    self.provider_name
                )));
            }
        }

        match rx.await {
            Ok(JsonRpcResponse::Ok(v)) => Ok(v),
            Ok(JsonRpcResponse::Err { code, message }) => Err(ProviderError::Protocol(format!(
                "{method}: {message} (code={code})"
            ))),
            Err(_) => Err(ProviderError::Protocol(format!(
                "{}: {method} response channel closed (stream ended)",
                self.provider_name
            ))),
        }
    }

    /// ACP `initialize` — declares protocol version + client capabilities.
    pub async fn initialize(&self) -> Result<Value, ProviderError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "gitim-agent-sdk",
                    "version": "0.1.0",
                },
                "clientCapabilities": {},
            }),
        )
        .await
    }

    /// ACP `authenticate` — first available auth method from the
    /// `initialize` response. Returns Ok(()) when there is no auth method
    /// to authenticate with, so callers can drive this unconditionally
    /// after `initialize`.
    pub async fn authenticate_first_method(
        &self,
        init_result: &Value,
    ) -> Result<(), ProviderError> {
        let method_id = init_result
            .get("authMethods")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|m| m.get("id"))
            .and_then(|id| id.as_str());
        if let Some(id) = method_id {
            self.request("authenticate", json!({ "methodId": id }))
                .await?;
        }
        Ok(())
    }

    /// ACP `session/new` — returns the freshly-assigned session id.
    /// `mcpServers` is sent empty; provider-side MCP wiring is not part
    /// of GitIM's v1 surface.
    pub async fn new_session(&self, cwd: &str) -> Result<String, ProviderError> {
        let res = self
            .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await?;
        let sid = extract_session_id(&res).ok_or_else(|| {
            ProviderError::Protocol(format!(
                "{}: session/new returned no session ID",
                self.provider_name
            ))
        })?;
        *self.current_session_id.lock().await = Some(sid.clone());
        Ok(sid)
    }

    /// ACP `session/resume` — provider may hand back a different id if
    /// the requested one expired. Returns `(actual, was_changed)` so the
    /// caller can log a switch.
    pub async fn resume_session(
        &self,
        cwd: &str,
        requested: &str,
    ) -> Result<(String, bool), ProviderError> {
        let res = self
            .request(
                "session/resume",
                json!({ "cwd": cwd, "sessionId": requested }),
            )
            .await?;
        // Hermes' session/resume returns only {models}; the id stays the
        // requested one. Spec-compliant servers may include sessionId.
        let actual = extract_session_id(&res).unwrap_or_else(|| requested.to_string());
        let was_changed = actual != requested;
        *self.current_session_id.lock().await = Some(actual.clone());
        Ok((actual, was_changed))
    }

    /// ACP `session/set_model` — switch the active model. Kimi calls this
    /// after `new_session` when `ExecOptions::model` is set; failure must
    /// be propagated so the driver fails the task rather than silently
    /// falling back to whatever default the provider picked.
    pub async fn set_session_model(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<(), ProviderError> {
        self.request(
            "session/set_model",
            json!({ "sessionId": session_id, "modelId": model_id }),
        )
        .await?;
        Ok(())
    }

    /// ACP `session/prompt` — sends the user payload as a single text
    /// content block. Awaits the response; `session/update` notifications
    /// arriving in the interim flow as `Event::*` via the reader task.
    pub async fn prompt(
        &self,
        session_id: &str,
        payload: &str,
    ) -> Result<PromptOutcome, ProviderError> {
        let res = self
            .request(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": payload }],
                }),
            )
            .await?;
        let stop_reason = res
            .get("stopReason")
            .and_then(|v| v.as_str())
            .unwrap_or("end_turn")
            .to_string();
        let usage = res.get("usage").and_then(parse_acp_usage);
        Ok(PromptOutcome { stop_reason, usage })
    }

    /// Process one stdout line. Either:
    /// - it carries `id` → it's a response, routed to the matching pending
    ///   one-shot;
    /// - it carries `method == "session/update"` → it's a notification
    ///   decoded by `parse::parse_notification` and emitted as `Event::*`;
    /// - anything else (malformed JSON, unknown method) is logged at debug
    ///   and dropped.
    pub async fn handle_line(&self, line: &str, event_tx: &mpsc::Sender<Event>) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                debug!(
                    provider = self.provider_name,
                    line_len = trimmed.len(),
                    "unparsed line"
                );
                return;
            }
        };

        if v.get("id").is_some()
            && v.get("method").is_some()
            && v.get("result").is_none()
            && v.get("error").is_none()
        {
            self.handle_agent_request(&v).await;
            return;
        }

        if let Some(id) = v.get("id").and_then(|x| x.as_i64()) {
            // Response — route to the pending sender, if any.
            let sender = self.pending.lock().await.remove(&id);
            if let Some(tx) = sender {
                let response = if let Some(err) = v.get("error") {
                    JsonRpcResponse::Err {
                        code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                        message: err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                    }
                } else {
                    JsonRpcResponse::Ok(v.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(response);
            } else {
                debug!(provider = self.provider_name, id, "response for unknown id");
            }
            return;
        }

        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method != "session/update" {
            return;
        }

        if let Some(should_accept) = &self.hooks.accept_notification {
            if !should_accept() {
                return;
            }
        }

        let Some(params) = v.get("params") else {
            return;
        };
        let Some(parsed) = parse_notification(params) else {
            return;
        };

        self.dispatch_parsed(parsed, event_tx).await;
    }

    async fn handle_agent_request(&self, v: &Value) {
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let response = match method {
            "session/request_permission" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "outcome": {
                        "outcome": "selected",
                        "optionId": "approve_for_session",
                    },
                },
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("method not found: {method}"),
                },
            }),
        };

        match serde_json::to_vec(&response) {
            Ok(mut payload) => {
                payload.push(b'\n');
                let mut stdin = self.stdin.lock().await;
                if let Err(e) = stdin.write_all(&payload).await {
                    warn!(
                        provider = self.provider_name,
                        method,
                        error = %e,
                        "failed to reply to ACP agent request"
                    );
                }
            }
            Err(e) => {
                warn!(
                    provider = self.provider_name,
                    method,
                    error = %e,
                    "failed to serialize ACP agent-request response"
                );
            }
        }
    }

    /// Drain the accumulated usage snapshot at session end. Hermes today
    /// prefers the `session/prompt` response value over this accumulator
    /// (mid-stream `usage_update` is dropped per `drive_session`), but
    /// kimi may opt to use it.
    pub async fn finalize_usage(&self) -> ProviderUsage {
        self.usage.lock().await.clone()
    }

    /// Drain the assistant text accumulated from `agent_message_chunk`
    /// notifications. Use this at session end to populate
    /// `ExecResult.output`.
    pub async fn collected_output(&self) -> String {
        self.text_accumulator.lock().await.clone()
    }

    /// Flush and close the underlying stdin handle. The provider sees EOF
    /// on its stdin reader and shuts the session down cleanly. Callers do
    /// this once the terminal `session/prompt` response has been received
    /// so the reader task can drain the trailing stdout (if any) and exit.
    ///
    /// After this is called, any subsequent `request()` will fail with a
    /// stdin-write error, which is fine — the client is at end of life.
    pub async fn close_stdin(&self) {
        let mut stdin = self.stdin.lock().await;
        let _ = stdin.shutdown().await;
    }

    /// Drop every pending request sender so blocked `request()` awaiters
    /// observe `Err(_)` from their oneshot receiver immediately, mapped to
    /// `ProviderError::Protocol("…response channel closed (stream ended)")`.
    ///
    /// The reader task MUST call this on stream exit (`Ok(None)` or
    /// `Err(_)` from `next_line()`). Without it, an in-flight `request`
    /// would block on its oneshot until the driver's outer
    /// `tokio::time::timeout` fires — defaulting to 20 minutes for hermes
    /// — turning every mid-prompt child crash into a 20-minute hang.
    pub async fn fail_pending(&self) {
        // Dropping every Sender wakes its receiver with `Err(Closed)`.
        self.pending.lock().await.clear();
    }

    /// Internal helper — translates a [`ParsedNotification`] into the
    /// corresponding `Event::*` (with the hook-mapped tool name), and
    /// updates the per-execute accumulators.
    async fn dispatch_parsed(&self, parsed: ParsedNotification, event_tx: &mpsc::Sender<Event>) {
        match parsed {
            ParsedNotification::Text { content } => {
                self.text_accumulator.lock().await.push_str(&content);
                try_send_event(event_tx, Event::Text { content });
            }
            ParsedNotification::Thinking { content } => {
                try_send_event(event_tx, Event::Thinking { content });
            }
            ParsedNotification::ToolCall {
                tool,
                call_id,
                input,
                args_text,
            } => {
                let mapped = (self.hooks.tool_name_mapper)(&tool);
                if let Some(input) = input.clone() {
                    self.pending_tools.lock().await.insert(
                        call_id.clone(),
                        PendingToolCall {
                            tool: mapped.clone(),
                            input: Some(input.clone()),
                            args_text,
                            emitted: true,
                        },
                    );
                    try_send_event(
                        event_tx,
                        Event::ToolUse {
                            tool: mapped,
                            call_id,
                            input,
                        },
                    );
                } else {
                    self.pending_tools.lock().await.insert(
                        call_id,
                        PendingToolCall {
                            tool: mapped,
                            input: None,
                            args_text,
                            emitted: false,
                        },
                    );
                }
            }
            ParsedNotification::ToolCallUpdate {
                tool,
                call_id,
                status,
                input,
                output,
                args_text,
            } => {
                if status != "completed" && status != "failed" {
                    if let Some(pending) = self.pending_tools.lock().await.get_mut(&call_id) {
                        if !pending.emitted && !args_text.is_empty() {
                            pending.args_text = args_text;
                        }
                    }
                    return;
                }

                let pending = self.pending_tools.lock().await.remove(&call_id);
                self.emit_deferred_tool_use(
                    event_tx,
                    &call_id,
                    pending,
                    tool,
                    input,
                    args_text.clone(),
                )
                .await;
                let tool_output = output.unwrap_or(args_text);
                try_send_event(
                    event_tx,
                    Event::ToolResult {
                        call_id,
                        output: tool_output,
                    },
                );
            }
            ParsedNotification::Usage(u) => {
                // Always update the accumulator (callers can drain via
                // `finalize_usage`). Emit a live `Event::Usage` only when
                // the provider opts in — hermes drives `ExecResult.usage`
                // off the prompt-response and would double-count mid-stream
                // pushes if the runtime token accumulator listened to them.
                *self.usage.lock().await = u.clone();
                if self.hooks.emit_live_usage {
                    // Drop pre-session usage updates silently — some ACP
                    // servers emit them during initialize, before any
                    // `session/new` response sets `current_session_id`.
                    // Fabricating an empty session_id would corrupt the
                    // runtime's per-session usage book-keeping.
                    if let Some(session_id) = self.current_session_id.lock().await.clone() {
                        try_send_event(
                            event_tx,
                            Event::Usage {
                                session_id,
                                usage: u,
                            },
                        );
                    }
                }
            }
        }
    }

    async fn emit_deferred_tool_use(
        &self,
        event_tx: &mpsc::Sender<Event>,
        call_id: &str,
        pending: Option<PendingToolCall>,
        update_tool: String,
        update_input: Option<Value>,
        update_args_text: String,
    ) {
        if pending.as_ref().is_some_and(|p| p.emitted) {
            return;
        }

        let tool = pending
            .as_ref()
            .map(|p| p.tool.clone())
            .unwrap_or_else(|| (self.hooks.tool_name_mapper)(&update_tool));
        let input = pending
            .as_ref()
            .and_then(|p| p.input.clone())
            .or(update_input)
            .or_else(|| {
                pending
                    .as_ref()
                    .and_then(|p| parse_tool_args_json(&p.args_text))
            })
            .or_else(|| parse_tool_args_json(&update_args_text))
            .unwrap_or_else(|| Value::Object(Default::default()));

        try_send_event(
            event_tx,
            Event::ToolUse {
                tool,
                call_id: call_id.to_string(),
                input,
            },
        );
    }
}

fn parse_tool_args_json(args_text: &str) -> Option<Value> {
    let trimmed = args_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .or_else(|| Some(json!({ "text": trimmed })))
}

fn extract_session_id(v: &Value) -> Option<String> {
    v.get("sessionId")
        .and_then(|x| x.as_str())
        .map(String::from)
}

/// Best-effort event emission — drops the event on a full channel rather
/// than blocking the driver / reader task. Used inside the ACP dispatch
/// path and re-used by the hermes driver for its own `Event::Status`
/// emission so the warning text is consistent.
pub(crate) fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command as TokioCommand;
    use tokio::time::{timeout, Duration};

    fn test_hooks() -> AcpHooks {
        AcpHooks {
            tool_name_mapper: AcpHooks::identity_tool_mapper,
            accept_notification: None,
            emit_live_usage: false,
        }
    }

    /// Regression test for the stream-close hang the refactor introduced
    /// and the `fail_pending` call in the reader exit path fixes.
    ///
    /// Setup: spawn `sh -c 'sleep 30'` (a process that never reads stdin
    /// nor writes stdout but holds both pipes open), wrap its stdin in an
    /// `AcpClient`, fire a request, then kill the child. The reader task
    /// hits EOF on stdout, calls `fail_pending`, and the in-flight
    /// `request().await` must return `Err(_)` within milliseconds — not
    /// block until any outer timeout the caller might wrap us in. We give
    /// the test a generous 5s wall clock; the unblock should be
    /// ~immediate.
    ///
    /// We use a non-echoing fake (rather than `cat`) so the request's
    /// stdin write succeeds into the void instead of being echoed back as
    /// a synthetic JSON-RPC response — without that, `handle_line` would
    /// parse the echo's `id` field and resolve the pending oneshot as Ok.
    #[tokio::test]
    async fn stream_close_releases_pending() {
        let mut child = TokioCommand::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("sh must be on PATH");

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        let client = Arc::new(AcpClient::new("test", stdin, test_hooks()));

        // Reader task — mirrors the production hermes wiring including the
        // fail_pending() call on stream exit. This is the contract under
        // test.
        let reader_client = Arc::clone(&client);
        let (event_tx, _event_rx) = mpsc::channel::<Event>(8);
        let reader = tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                reader_client.handle_line(&line, &event_tx).await;
            }
            reader_client.fail_pending().await;
        });

        // Fire the request in the background — it'll block on its oneshot
        // until either a real response (cat won't send one) or fail_pending
        // drops the sender.
        let request_client = Arc::clone(&client);
        let request = tokio::spawn(async move { request_client.request("ping", json!({})).await });

        // Wait for the request's stdin write to complete so we exercise the
        // post-write window — the one where `fail_pending` is the critical
        // actor. Without this delay the kill races the write, broken-pipe
        // wins, and the test no longer regresses if `fail_pending` is
        // removed. cat echoes the line back so a quick `wait` on a sentinel
        // round-trip would be more deterministic, but a 50ms sleep keeps the
        // test trivially understandable and is still fast.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Kill cat so its stdout closes → reader sees EOF → fail_pending fires.
        let _ = child.start_kill();
        let _ = child.wait().await;

        let result = timeout(Duration::from_secs(5), request)
            .await
            .expect("request should unblock once reader calls fail_pending")
            .expect("join handle");

        match result {
            Err(ProviderError::Protocol(msg)) => {
                // Two races land in the same property — the request
                // either failed at write time ("stdin write failed …
                // Broken pipe") because we killed the child first, or
                // it succeeded into the void and was released by
                // `fail_pending` once the reader hit EOF
                // ("response channel closed (stream ended)"). Both
                // satisfy the contract: `request` must not hang.
                assert!(
                    msg.contains("stream ended") || msg.contains("Broken pipe"),
                    "expected stream-ended or broken-pipe diagnostic, got: {msg}"
                );
            }
            other => panic!("expected ProviderError::Protocol, got {other:?}"),
        }

        // Drain the reader so the test exits cleanly.
        let _ = timeout(Duration::from_secs(1), reader).await;
    }

    #[tokio::test]
    async fn auto_approves_agent_permission_request() {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut child = TokioCommand::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("cat must be on PATH");

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let client = AcpClient::new("test", stdin, test_hooks());
        let (event_tx, _event_rx) = mpsc::channel::<Event>(8);

        client
            .handle_line(
                r#"{"jsonrpc":"2.0","id":42,"method":"session/request_permission","params":{"sessionId":"ses_1","options":[{"optionId":"approve_for_session","kind":"allow_always"}]}}"#,
                &event_tx,
            )
            .await;

        let mut lines = BufReader::new(stdout).lines();
        let line = timeout(Duration::from_secs(1), lines.next_line())
            .await
            .expect("permission reply should be written")
            .expect("stdout read")
            .expect("cat should echo one line");
        let v: Value = serde_json::from_str(&line).expect("reply must be JSON");
        assert_eq!(v["id"], 42);
        assert_eq!(v["result"]["outcome"]["outcome"], "selected");
        assert_eq!(v["result"]["outcome"]["optionId"], "approve_for_session");

        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    #[tokio::test]
    async fn defers_kimi_streaming_tool_use_until_args_are_complete() {
        let mut child = TokioCommand::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("cat must be on PATH");

        let stdin = child.stdin.take().expect("piped stdin");
        let client = AcpClient::new("test", stdin, test_hooks());
        let (event_tx, mut event_rx) = mpsc::channel::<Event>(8);

        client
            .handle_line(
                r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"tool_call","toolCallId":"tc-kimi-1","title":"Shell","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":""}}]}}}"#,
                &event_tx,
            )
            .await;
        assert!(event_rx.try_recv().is_err());

        client
            .handle_line(
                r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-kimi-1","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":"{\"command\":\"echo hi\"}"}}]}}}"#,
                &event_tx,
            )
            .await;
        assert!(event_rx.try_recv().is_err());

        client
            .handle_line(
                r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"ses_1","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-kimi-1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"hi\n"}}]}}}"#,
                &event_tx,
            )
            .await;

        match event_rx.recv().await.expect("tool use event") {
            Event::ToolUse {
                tool,
                call_id,
                input,
            } => {
                assert_eq!(tool, "Shell");
                assert_eq!(call_id, "tc-kimi-1");
                assert_eq!(input["command"], "echo hi");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        match event_rx.recv().await.expect("tool result event") {
            Event::ToolResult { call_id, output } => {
                assert_eq!(call_id, "tc-kimi-1");
                assert_eq!(output, "hi\n");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }

        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}
