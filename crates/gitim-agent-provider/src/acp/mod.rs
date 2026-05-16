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
    /// notifications. Currently informational only — hermes and kimi
    /// receive full `rawInput` up front and don't need argument streaming.
    /// Kept on the struct (per design) so kiro / future ACP servers that
    /// do stream arguments can plug in without changing the API.
    #[allow(dead_code)]
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
/// private — kept on the [`AcpClient`] type only as a placeholder for
/// future argument-streaming ACP servers (see the field doc).
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct PendingToolCall {
    pub tool: String,
    pub input: Value,
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
            self.request("authenticate", json!({ "methodId": id })).await?;
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
                debug!(
                    provider = self.provider_name,
                    id, "response for unknown id"
                );
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

    /// Internal helper — translates a [`ParsedNotification`] into the
    /// corresponding `Event::*` (with the hook-mapped tool name), and
    /// updates the per-execute accumulators.
    async fn dispatch_parsed(
        &self,
        parsed: ParsedNotification,
        event_tx: &mpsc::Sender<Event>,
    ) {
        match parsed {
            ParsedNotification::Text { content } => {
                self.text_accumulator
                    .lock()
                    .await
                    .push_str(&content);
                try_send_event(event_tx, Event::Text { content });
            }
            ParsedNotification::Thinking { content } => {
                try_send_event(event_tx, Event::Thinking { content });
            }
            ParsedNotification::ToolCall {
                tool,
                call_id,
                input,
            } => {
                let mapped = (self.hooks.tool_name_mapper)(&tool);
                self.pending_tools.lock().await.insert(
                    call_id.clone(),
                    PendingToolCall {
                        tool: mapped.clone(),
                        input: input.clone(),
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
            }
            ParsedNotification::ToolResult {
                call_id,
                output: tool_output,
            } => {
                // Discard the pending entry — we currently re-emit nothing
                // from it (call_id alone is the link the runtime needs).
                let _ = self.pending_tools.lock().await.remove(&call_id);
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
                    let session_id = self
                        .current_session_id
                        .lock()
                        .await
                        .clone()
                        .unwrap_or_default();
                    try_send_event(event_tx, Event::Usage { session_id, usage: u });
                }
            }
        }
    }
}

fn extract_session_id(v: &Value) -> Option<String> {
    v.get("sessionId").and_then(|x| x.as_str()).map(String::from)
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}
