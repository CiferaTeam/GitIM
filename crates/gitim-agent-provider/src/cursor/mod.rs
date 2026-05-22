//! Cursor Agent CLI provider — stream-json protocol.
//!
//! Spec: docs/plans/kimi-cursor-providers/00-requirements.md §"Cursor 设计"
//! Reference: multica/server/pkg/agent/cursor.go (the Go decoder this is
//! translated from).
//!
//! Provider semantics:
//! - `reports_usage() = true`   — cursor emits `step_finish` + `result` usage
//! - `usage_is_cumulative() = false` — `result.usage` is one-turn total since
//!   `execute()` only ever runs a single prompt turn
//! - context-window occupancy is not provider-reported; Cursor token usage is
//!   accounting-only across internal model calls, so the runtime estimator
//!   drives the HUD
//! - `self_managed_context() = false` — runtime owns the `[[RESET]]` channel
//!   and occupancy preamble, same as claude/codex

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError,
    ProviderUsage, ProviderUsageReport, Session,
};

pub mod parse;

use parse::{
    cursor_error_text, normalize_stream_line, parse_event, CursorStreamEvent, CursorUsage,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct CursorProvider {
    config: ProviderConfig,
}

impl CursorProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for CursorProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "cursor-agent".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let args = build_args(prompt, &opts);

        let mut cmd = Command::new(&exec_path);
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "cursor-agent started");

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

/// Build the argv vector for a one-shot `cursor-agent` invocation.
/// Reference: multica/server/pkg/agent/cursor.go:397-422.
///
/// Shape: `--print --output-format stream-json --yolo
///   [--workspace <cwd>] [--model <m>] [--resume <id>]`
///
/// `merged_prompt` = `system_prompt + "\n\n---\n\n" + prompt` when
/// `opts.system_prompt` is Some(non-empty), else just `prompt`. cursor-agent
/// CLI does not support `--system-prompt` (see multica/cursor.go:415-416).
pub(crate) fn build_args(prompt: &str, opts: &ExecOptions) -> Vec<String> {
    let merged_prompt = match &opts.system_prompt {
        Some(sp) if !sp.is_empty() => format!("{sp}\n\n---\n\n{prompt}"),
        _ => prompt.to_string(),
    };
    let mut args = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--yolo".to_string(),
    ];
    if let Some(cwd) = &opts.cwd {
        args.push("--workspace".to_string());
        args.push(cwd.to_string_lossy().into_owned());
    }
    if let Some(model) = &opts.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(resume) = &opts.resume_token {
        args.push("--resume".to_string());
        args.push(resume.clone());
    }
    args.push(merged_prompt);
    args
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
    // Tracks whether we saw a terminal `result` envelope. If the stdout stream
    // closes cleanly without one, we promote `final_status` to `Failed` —
    // truncation / protocol error. Mirrors claude/mod.rs:294-298.
    let mut saw_result = false;
    // step_usage accumulates per-step token counts from `step_finish` events.
    // result_usage holds the authoritative session total from a `result` event.
    // If `result` carries usage, we prefer it; otherwise fall back to step_usage.
    // Reference: multica/cursor.go:84-91.
    let mut step_usage = ProviderUsage::default();
    let mut result_usage: Option<ProviderUsage> = None;

    let mut reader = BufReader::new(stdout).lines();

    // Collect stderr tail for error reporting (same pattern as claude/mod.rs).
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "cursor:stderr", "{}", line);
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
                        Ok(Some(raw_line)) => {
                            let line = normalize_stream_line(&raw_line);
                            if line.is_empty() {
                                continue;
                            }

                            let evt = match parse_event(&line) {
                                Some(e) => e,
                                None => {
                                    debug!(pid, line_len = line.len(), "unparsed cursor line");
                                    continue;
                                }
                            };

                            // Capture session_id from any event that carries it.
                            if let Some(sid) = evt.session_id.as_ref() {
                                let trimmed = sid.trim();
                                if !trimmed.is_empty() {
                                    session_id = trimmed.to_string();
                                }
                            }

                            match evt.r#type.as_str() {
                                "system" => {
                                    let subtype = evt.subtype.as_deref().unwrap_or("");
                                    if subtype == "init" {
                                        try_send_event(&event_tx, Event::Status {
                                            status: "running".to_string(),
                                        });
                                    } else if subtype == "error" {
                                        let err_msg = cursor_error_text(&evt);
                                        if !err_msg.is_empty() {
                                            // Mirror claude's "if not already terminal,
                                            // promote to Failed now" discipline so the
                                            // ExecResult never carries a status/error
                                            // contradiction.
                                            promote_to_failed_if_completed(
                                                &mut final_status,
                                                &mut final_error,
                                                &err_msg,
                                            );
                                            try_send_event(&event_tx, Event::Error {
                                                content: err_msg,
                                            });
                                        }
                                    }
                                }
                                "assistant" => {
                                    handle_assistant_message(&evt, &event_tx, &mut output);
                                }
                                "tool_use" => {
                                    let input = evt.parameters.clone()
                                        .unwrap_or(serde_json::Value::Object(Default::default()));
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool: evt.tool_name.clone().unwrap_or_default(),
                                        call_id: evt.tool_id.clone().unwrap_or_default(),
                                        input,
                                    });
                                }
                                "tool_result" => {
                                    try_send_event(&event_tx, Event::ToolResult {
                                        call_id: evt.tool_id.clone().unwrap_or_default(),
                                        output: evt.output.clone().unwrap_or_default(),
                                    });
                                }
                                "tool_call" => match evt.subtype.as_deref() {
                                    Some("started") => {
                                        if let Some(event) = cursor_tool_call_started(&evt) {
                                            try_send_event(&event_tx, event);
                                        }
                                    }
                                    Some("completed") => {
                                        if let Some(event) = cursor_tool_call_completed(&evt) {
                                            try_send_event(&event_tx, event);
                                        }
                                    }
                                    _ => {
                                        debug!(
                                            pid,
                                            subtype = ?evt.subtype,
                                            "ignored cursor tool_call event"
                                        );
                                    }
                                },
                                "result" => {
                                    saw_result = true;
                                    // is_error or subtype="error" → fail-status.
                                    let is_error = evt.is_error
                                        || evt.subtype.as_deref() == Some("error");
                                    if is_error {
                                        final_status = ExecStatus::Failed;
                                        let err_msg = cursor_error_text(&evt);
                                        final_error = if err_msg.is_empty() {
                                            None
                                        } else {
                                            Some(err_msg)
                                        };
                                    }
                                    if let Some(text) = evt.result_text.as_deref() {
                                        if !text.is_empty() && output.is_empty() {
                                            output.push_str(text);
                                        }
                                    }
                                    // result.usage takes precedence; otherwise fall back later.
                                    if let Some(u) = evt.usage.as_ref() {
                                        result_usage = Some(cursor_to_provider_usage(u));
                                    }
                                    info!(
                                        pid,
                                        is_error,
                                        result_len = evt.result_text.as_deref()
                                            .map(|s| s.len()).unwrap_or(0),
                                        "cursor result received"
                                    );
                                }
                                "error" => {
                                    let err_msg = cursor_error_text(&evt);
                                    if !err_msg.is_empty() {
                                        promote_to_failed_if_completed(
                                            &mut final_status,
                                            &mut final_error,
                                            &err_msg,
                                        );
                                        try_send_event(&event_tx, Event::Error {
                                            content: err_msg,
                                        });
                                    }
                                }
                                "text" => {
                                    if let Some(part) = evt.part.as_ref() {
                                        if let Ok(parsed) = serde_json::from_value::<CursorTextPart>(part.clone()) {
                                            if !parsed.text.is_empty() {
                                                output.push_str(&parsed.text);
                                                try_send_event(&event_tx, Event::Text {
                                                    content: parsed.text,
                                                });
                                            }
                                        }
                                    }
                                }
                                "step_finish" => {
                                    if let Some(part) = evt.part.as_ref() {
                                        if let Ok(parsed) =
                                            serde_json::from_value::<CursorStepFinishPart>(part.clone())
                                        {
                                            accumulate_step_usage(&mut step_usage, &parsed);
                                        }
                                    }
                                }
                                _ => {
                                    // Unknown envelope — log and continue.
                                    debug!(pid, evt_type = %evt.r#type, "ignored cursor event");
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
        final_error = Some(format!("cursor-agent timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else if !saw_result && final_status == ExecStatus::Completed {
        // Stream ended without a result message — truncated or protocol error.
        // Mirrors claude/mod.rs:294-298.
        final_status = ExecStatus::Failed;
        final_error = Some("cursor-agent stream ended without a result message".to_string());
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("cursor-agent exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for cursor-agent: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "cursor-agent finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail.
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
        let tail = stderr_tail.lock().unwrap();
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

    // result.usage > step_usage. Reference: multica/cursor.go:192-196.
    let final_usage = result_usage.or_else(|| {
        if step_usage == ProviderUsage::default() {
            None
        } else {
            Some(step_usage)
        }
    });

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
        usage_report: ProviderUsageReport::from_usage(final_usage.clone()),
        usage: final_usage,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("cursor event channel full, dropping event");
    }
}

/// Promote `final_status` to `Failed` and stash `err_msg` as `final_error`,
/// but only if the status is still `Completed`. Once a stream has reached a
/// terminal state (`Failed` / `Aborted` / `Timeout`), preserve the first
/// terminal cause — later errors get logged via the event channel but don't
/// rewrite the verdict. Mirrors claude/mod.rs's "if not already terminal,
/// set it now" discipline (see claude/mod.rs:302-305).
fn promote_to_failed_if_completed(
    final_status: &mut ExecStatus,
    final_error: &mut Option<String>,
    err_msg: &str,
) {
    if *final_status == ExecStatus::Completed {
        *final_status = ExecStatus::Failed;
        *final_error = Some(err_msg.to_string());
    }
}

/// Handle an `assistant` envelope's content blocks. Mirrors
/// multica/cursor.go:227-265 — iterates `message.content[]`, dispatches each
/// block by `block.type`.
///
/// Note: per-message `usage` on assistant events is intentionally ignored to
/// avoid double-counting; cursor only reports authoritative totals on `result`
/// (or fallback aggregate on `step_finish`). See multica/cursor.go:237-239.
#[allow(clippy::collapsible_match)]
fn handle_assistant_message(
    evt: &CursorStreamEvent,
    event_tx: &mpsc::Sender<Event>,
    output: &mut String,
) {
    let message = match evt.message.as_ref() {
        Some(m) => m,
        None => return,
    };
    let parsed: CursorAssistantMessage = match serde_json::from_value(message.clone()) {
        Ok(v) => v,
        Err(_) => return,
    };

    for block in parsed.content {
        match block.r#type.as_str() {
            "output_text" | "text" => {
                if !block.text.is_empty() {
                    output.push_str(&block.text);
                    try_send_event(
                        event_tx,
                        Event::Text {
                            content: block.text,
                        },
                    );
                }
            }
            "thinking" => {
                if !block.text.is_empty() {
                    try_send_event(
                        event_tx,
                        Event::Thinking {
                            content: block.text,
                        },
                    );
                }
            }
            "tool_use" => {
                let input = block
                    .input
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                try_send_event(
                    event_tx,
                    Event::ToolUse {
                        tool: block.name.unwrap_or_default(),
                        call_id: block.id.unwrap_or_default(),
                        input,
                    },
                );
            }
            _ => {}
        }
    }
}

fn cursor_tool_call_started(evt: &CursorStreamEvent) -> Option<Event> {
    let call_id = cursor_tool_call_id(evt);
    let tool_call = evt.tool_call.as_ref()?;

    if let Some(shell) = tool_call.get("shellToolCall") {
        let args = shell.get("args").unwrap_or(&serde_json::Value::Null);
        let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let mut input = serde_json::Map::new();
        if !command.is_empty() {
            input.insert(
                "command".to_string(),
                serde_json::Value::String(command.to_string()),
            );
        }
        if let Some(cwd) = args.get("workingDirectory").and_then(|v| v.as_str()) {
            if !cwd.is_empty() {
                input.insert(
                    "workingDirectory".to_string(),
                    serde_json::Value::String(cwd.to_string()),
                );
            }
        }
        if let Some(timeout) = args.get("timeout").and_then(|v| v.as_i64()) {
            input.insert(
                "timeout".to_string(),
                serde_json::Value::Number(timeout.into()),
            );
        }
        if input.is_empty() {
            return None;
        }
        return Some(Event::ToolUse {
            tool: "Bash".to_string(),
            call_id,
            input: serde_json::Value::Object(input),
        });
    }

    let obj = tool_call.as_object()?;
    let (raw_name, payload) = obj.iter().next()?;
    Some(Event::ToolUse {
        tool: cursor_tool_name(raw_name),
        call_id,
        input: payload.clone(),
    })
}

fn cursor_tool_call_completed(evt: &CursorStreamEvent) -> Option<Event> {
    let call_id = cursor_tool_call_id(evt);
    let tool_call = evt.tool_call.as_ref()?;

    if let Some(shell) = tool_call.get("shellToolCall") {
        return Some(Event::ToolResult {
            call_id,
            output: cursor_shell_tool_output(shell),
        });
    }

    Some(Event::ToolResult {
        call_id,
        output: tool_call.to_string(),
    })
}

fn cursor_tool_call_id(evt: &CursorStreamEvent) -> String {
    evt.call_id
        .clone()
        .or_else(|| evt.tool_id.clone())
        .unwrap_or_default()
}

fn cursor_tool_name(raw: &str) -> String {
    if raw == "shellToolCall" {
        return "Bash".to_string();
    }
    raw.strip_suffix("ToolCall").unwrap_or(raw).to_string()
}

fn cursor_shell_tool_output(shell: &serde_json::Value) -> String {
    let Some(result) = shell.get("result") else {
        return String::new();
    };

    if let Some(success) = result.get("success") {
        if let Some(output) = success.get("interleavedOutput").and_then(|v| v.as_str()) {
            if !output.is_empty() {
                return output.to_string();
            }
        }

        let stdout = success.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let stderr = success.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
        if stdout.is_empty() {
            return stderr.to_string();
        }
        if stderr.is_empty() {
            return stdout.to_string();
        }
        return format!("{stdout}\n{stderr}");
    }

    if let Some(error) = result.get("error") {
        if let Some(message) = error.as_str() {
            return message.to_string();
        }
        return error.to_string();
    }

    result.to_string()
}

fn cursor_to_provider_usage(u: &CursorUsage) -> ProviderUsage {
    ProviderUsage {
        input_tokens: Some(u.input_tokens),
        output_tokens: Some(u.output_tokens),
        used_percent: None,
        cache_read_tokens: Some(u.cache_read_input_tokens),
        cache_creation_tokens: None,
        // Cursor's result usage is a billing/accounting total across internal
        // agent model calls, not current prompt occupancy. Mark the context
        // window unavailable so compute_snapshot falls back to the runtime
        // estimator while still accumulating token usage.
        context_tokens: Some(0),
        context_window_tokens: Some(0),
    }
}

fn accumulate_step_usage(acc: &mut ProviderUsage, part: &CursorStepFinishPart) {
    let input = part.tokens.input as u64;
    let output = part.tokens.output as u64;
    let cache_read = part.tokens.cache.read as u64;
    acc.input_tokens = Some(acc.input_tokens.unwrap_or(0) + input);
    acc.output_tokens = Some(acc.output_tokens.unwrap_or(0) + output);
    acc.cache_read_tokens = Some(acc.cache_read_tokens.unwrap_or(0) + cache_read);
    acc.context_tokens = Some(0);
    acc.context_window_tokens = Some(0);
}

// ── Cursor stream-json internal types ──

#[derive(Deserialize, Default)]
struct CursorAssistantMessage {
    #[serde(default)]
    content: Vec<CursorContentBlock>,
}

#[derive(Deserialize, Default)]
struct CursorContentBlock {
    r#type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct CursorTextPart {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize, Default)]
struct CursorStepFinishPart {
    #[serde(default)]
    tokens: CursorStepFinishTokens,
}

#[derive(Deserialize, Default)]
struct CursorStepFinishTokens {
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(default)]
    cache: CursorStepFinishCache,
}

#[derive(Deserialize, Default)]
struct CursorStepFinishCache {
    #[serde(default)]
    read: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_args_minimal() {
        let args = build_args("hi", &ExecOptions::default());
        assert_eq!(
            args,
            vec!["--print", "--output-format", "stream-json", "--yolo", "hi",]
        );
    }

    #[test]
    fn build_args_with_system_prompt_merges() {
        let opts = ExecOptions {
            system_prompt: Some("sys".to_string()),
            ..Default::default()
        };
        let args = build_args("hi", &opts);
        assert_eq!(args.last().map(String::as_str), Some("sys\n\n---\n\nhi"));
    }

    #[test]
    fn build_args_with_empty_system_prompt_does_not_merge() {
        let opts = ExecOptions {
            system_prompt: Some(String::new()),
            ..Default::default()
        };
        let args = build_args("hi", &opts);
        assert_eq!(args.last().map(String::as_str), Some("hi"));
    }

    #[test]
    fn build_args_with_cwd_model_resume() {
        let opts = ExecOptions {
            cwd: Some(PathBuf::from("/tmp/x")),
            model: Some("claude-sonnet-4-6".to_string()),
            resume_token: Some("sess-abc".to_string()),
            ..Default::default()
        };
        let args = build_args("hi", &opts);
        assert!(args.windows(2).any(|w| w == ["--workspace", "/tmp/x"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--model", "claude-sonnet-4-6"]));
        assert!(args.windows(2).any(|w| w == ["--resume", "sess-abc"]));
    }

    #[test]
    fn maps_real_cursor_shell_tool_call_started_to_bash() {
        let evt = parse_event(
            r#"{"type":"tool_call","subtype":"started","call_id":"tool-1","tool_call":{"shellToolCall":{"args":{"command":"printf ok","workingDirectory":"/tmp","timeout":30000},"description":"Print ok"}}}"#,
        )
        .unwrap();

        let event = cursor_tool_call_started(&evt).unwrap();
        match event {
            Event::ToolUse {
                tool,
                call_id,
                input,
            } => {
                assert_eq!(tool, "Bash");
                assert_eq!(call_id, "tool-1");
                assert_eq!(input["command"], "printf ok");
                assert_eq!(input["workingDirectory"], "/tmp");
                assert_eq!(input["timeout"], 30000);
            }
            _ => panic!("expected tool use"),
        }
    }

    #[test]
    fn maps_real_cursor_shell_tool_call_completed_to_tool_result() {
        let evt = parse_event(
            r#"{"type":"tool_call","subtype":"completed","call_id":"tool-1","tool_call":{"shellToolCall":{"result":{"success":{"stdout":"ok","stderr":"","interleavedOutput":"ok","exitCode":0}}}}}"#,
        )
        .unwrap();

        let event = cursor_tool_call_completed(&evt).unwrap();
        match event {
            Event::ToolResult { call_id, output } => {
                assert_eq!(call_id, "tool-1");
                assert_eq!(output, "ok");
            }
            _ => panic!("expected tool result"),
        }
    }

    #[test]
    fn result_usage_marks_context_window_unavailable() {
        let usage = cursor_to_provider_usage(&CursorUsage {
            input_tokens: 109_725,
            output_tokens: 888,
            cache_read_input_tokens: 31_840,
        });

        assert_eq!(usage.input_tokens, Some(109_725));
        assert_eq!(usage.output_tokens, Some(888));
        assert_eq!(usage.cache_read_tokens, Some(31_840));
        assert_eq!(usage.context_tokens, Some(0));
        assert_eq!(usage.context_window_tokens, Some(0));
    }

    #[test]
    fn provider_trait_flags() {
        let p = CursorProvider::new(ProviderConfig::default());
        assert!(p.reports_usage());
        assert!(!p.usage_is_cumulative());
        assert!(!p.self_managed_context());
    }

    /// `promote_to_failed_if_completed` must flip Completed → Failed on the
    /// first error, but never overwrite a status that's already terminal —
    /// otherwise a late `error` envelope arriving after a cancel/timeout
    /// would rewrite the (more accurate) original verdict.
    #[test]
    fn promote_to_failed_only_when_completed() {
        // Completed → Failed (the happy path: cursor emits an error envelope
        // and the run hadn't reached any terminal state yet).
        let mut status = ExecStatus::Completed;
        let mut err: Option<String> = None;
        promote_to_failed_if_completed(&mut status, &mut err, "boom");
        assert_eq!(status, ExecStatus::Failed);
        assert_eq!(err.as_deref(), Some("boom"));

        // Already Failed → no-op (preserve first cause).
        let mut status = ExecStatus::Failed;
        let mut err = Some("first cause".to_string());
        promote_to_failed_if_completed(&mut status, &mut err, "second cause");
        assert_eq!(status, ExecStatus::Failed);
        assert_eq!(err.as_deref(), Some("first cause"));

        // Aborted → no-op (cancel-by-steering wins).
        let mut status = ExecStatus::Aborted;
        let mut err = Some("cancelled by steering".to_string());
        promote_to_failed_if_completed(&mut status, &mut err, "late error");
        assert_eq!(status, ExecStatus::Aborted);
        assert_eq!(err.as_deref(), Some("cancelled by steering"));

        // Timeout → no-op (timeout wins).
        let mut status = ExecStatus::Timeout;
        let mut err = Some("timed out".to_string());
        promote_to_failed_if_completed(&mut status, &mut err, "late error");
        assert_eq!(status, ExecStatus::Timeout);
        assert_eq!(err.as_deref(), Some("timed out"));
    }
}
