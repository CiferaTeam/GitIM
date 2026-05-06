use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError,
    ProviderUsage, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct OpencodeProvider {
    config: ProviderConfig,
}

impl OpencodeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for OpencodeProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "opencode".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let inv = build_invocation(prompt, &opts);

        let mut cmd = Command::new(&exec_path);
        cmd.args(&inv.args)
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Apply provider-level env first so invocation env can override.
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }
        for (k, v) in &inv.env {
            cmd.env(k, v);
        }

        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(
            pid,
            cwd = ?opts.cwd,
            model = ?opts.model,
            has_sys = opts.system_prompt.is_some(),
            "opencode started"
        );

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
    let mut captured_usage: Option<ProviderUsage> = None;

    let mut reader = BufReader::new(stdout).lines();

    // Collect stderr tail for error reporting
    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "opencode:stderr", "{}", line);
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
                                ParsedMessage::StepFinish { usage } => {
                                    captured_usage = Some(usage);
                                }
                                ParsedMessage::Text { content } => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                ParsedMessage::ToolUse { tool, call_id, input, status, output: tool_output } => {
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool, call_id: call_id.clone(), input,
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
        final_error = Some(format!("opencode timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("opencode exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for opencode: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "opencode finished");

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
        usage: captured_usage,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

/// Plan of record for invoking `opencode run`. Separated from execute() so
/// command construction is testable without spawning a real process.
#[derive(Debug)]
pub struct Invocation {
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// Build the argv + env for `opencode run` given a user prompt and options.
///
/// System prompt is injected via OPENCODE_CONFIG_CONTENT as a custom `gitim`
/// agent, selected on the CLI with `--agent gitim`. There is NO CLI flag for
/// system prompt — `opencode run --help` confirms this.
pub fn build_invocation(prompt: &str, opts: &ExecOptions) -> Invocation {
    let mut args: Vec<String> = vec![
        "run".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--dangerously-skip-permissions".to_string(),
    ];

    if let Some(model) = &opts.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(resume_token) = &opts.resume_token {
        args.push("--session".to_string());
        args.push(resume_token.clone());
    }

    let mut env: HashMap<String, String> = HashMap::new();
    // OPENCODE_PERMISSION merges into final permission config; "*":"allow"
    // flattens external_directory ask, .env ask, etc. so the agent can touch
    // the workspace without per-path approval.
    env.insert(
        "OPENCODE_PERMISSION".to_string(),
        r#"{"*":"allow"}"#.to_string(),
    );

    if let Some(system_prompt) = opts.system_prompt.as_ref().filter(|s| !s.is_empty()) {
        let config = json!({
            "agent": {
                "gitim": {
                    "prompt": system_prompt,
                    "mode": "primary",
                }
            }
        });
        env.insert("OPENCODE_CONFIG_CONTENT".to_string(), config.to_string());
        args.push("--agent".to_string());
        args.push("gitim".to_string());
    }

    // `--` terminator so messages starting with `-` don't get parsed as flags.
    args.push("--".to_string());
    args.push(prompt.to_string());

    Invocation { args, env }
}

/// Parsed result from a single line of OpenCode JSON output.
#[derive(Debug)]
pub enum ParsedMessage {
    /// Step start with session ID.
    StepStart { session_id: String },
    /// Step finish with token usage.
    StepFinish { usage: ProviderUsage },
    /// Text content from an assistant message.
    Text { content: String },
    /// Tool use event — combined: carries invocation + optional result when completed.
    ToolUse {
        tool: String,
        call_id: String,
        input: Value,
        status: String,
        output: Option<String>,
    },
    /// Error event.
    Error { message: String },
}

/// Parse a single line of OpenCode JSON output.
/// Returns None for empty lines, malformed JSON, unrecognized types, or step_finish without tokens.
pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let raw: RawEvent = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "step_start" => Some(ParsedMessage::StepStart {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "step_finish" => {
            let tokens = raw.part?.tokens?;
            Some(ParsedMessage::StepFinish {
                usage: ProviderUsage {
                    input_tokens: Some(tokens.input),
                    output_tokens: Some(tokens.output.saturating_add(tokens.reasoning)),
                    used_percent: None,
                    cache_read_tokens: Some(tokens.cache.read),
                    cache_creation_tokens: Some(tokens.cache.write),
                },
            })
        }
        "text" => {
            let part = raw.part.unwrap_or_default();
            Some(ParsedMessage::Text {
                content: part.text.unwrap_or_default(),
            })
        }
        "tool_use" => {
            let part = raw.part.unwrap_or_default();
            let state = part.state.unwrap_or_default();
            let output = state.output.map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            });
            Some(ParsedMessage::ToolUse {
                tool: part.tool.unwrap_or_default(),
                call_id: part.call_id.unwrap_or_default(),
                input: state.input.unwrap_or(Value::Object(Default::default())),
                status: state.status.unwrap_or_default(),
                output,
            })
        }
        "error" => {
            let err = raw.error?;
            let message = err
                .data
                .and_then(|d| d.get("message").and_then(|v| v.as_str().map(String::from)))
                .or(err.name)
                .unwrap_or_default();
            Some(ParsedMessage::Error { message })
        }
        _ => None,
    }
}

// ── OpenCode NDJSON event types (internal) ──

#[derive(Deserialize)]
struct RawEvent {
    r#type: String,
    #[serde(default, rename = "sessionID")]
    session_id: Option<String>,
    #[serde(default)]
    part: Option<RawPart>,
    #[serde(default)]
    error: Option<RawError>,
}

#[derive(Deserialize, Default)]
struct RawPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default, rename = "callID")]
    call_id: Option<String>,
    #[serde(default)]
    state: Option<RawToolState>,
    #[serde(default)]
    tokens: Option<RawTokens>,
}

#[derive(Deserialize, Default)]
struct RawToolState {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    output: Option<Value>,
}

#[derive(Deserialize, Default)]
struct RawTokens {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    cache: RawTokenCache,
}

#[derive(Deserialize, Default)]
struct RawTokenCache {
    #[serde(default)]
    read: u64,
    #[serde(default)]
    write: u64,
}

#[derive(Deserialize)]
struct RawError {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}
