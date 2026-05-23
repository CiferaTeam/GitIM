use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    preconditions, Event, ExecOptions, ExecResult, ExecStatus, PromptContext, Provider,
    ProviderConfig, ProviderError, ProviderUsage, ProviderUsageReport, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;
const STDERR_TAIL_LINES: usize = 20;

/// Long-lived Claude CLI process. Stays resident across turns so the SDK's
/// `<system-reminder>` memoization survives, keeping prompt-cache hit rate
/// high. Killed only on `[[RESET]]`, cancel/abort, error, or process death.
struct PersistentClaude {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    pid: u32,
    /// `session_id` reported by the first `system` event after spawn.
    /// Populated after the first turn completes — fresh-spawn from cold start
    /// won't know it until the CLI prints it.
    session_id: Option<String>,
    /// Spawn-time signature; used to decide whether a new `execute()` call can
    /// reuse this process or has to kill+respawn.
    sig: SpawnSig,
    stderr_tail: Arc<std::sync::Mutex<Vec<String>>>,
    stderr_handle: JoinHandle<()>,
}

impl PersistentClaude {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    async fn kill(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        self.stderr_handle.abort();
    }
}

impl Drop for PersistentClaude {
    fn drop(&mut self) {
        // child has kill_on_drop=true; only the detached stderr pump needs help.
        self.stderr_handle.abort();
    }
}

/// Fields that, if changed across turns, force a respawn (and a fresh prompt
/// cache). `system_prompt` is intentionally absent — agent_loop only sends it
/// on cold start, and we use `resume_token`/`session_id` to decide reuse.
#[derive(Debug, Clone, PartialEq)]
struct SpawnSig {
    cwd: Option<PathBuf>,
    model: Option<String>,
}

pub struct ClaudeProvider {
    config: ProviderConfig,
    persistent: Arc<Mutex<Option<PersistentClaude>>>,
}

impl ClaudeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            persistent: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    fn prompt_identity(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_identity(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    fn prompt_memory(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_memory(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    fn prompt_reset_protocol(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_reset_protocol(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    fn prompt_cold_start(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cold_start(ctx).replace("AGENTS.md", "CLAUDE.md")
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "claude".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let env = self.config.env.clone();
        let inner = self.persistent.clone();
        let prompt_owned = prompt.to_string();
        let opts_owned = opts;
        let exec_path_owned = exec_path;

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let cancel_token = CancellationToken::new();
        let cancel_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            run_turn(
                inner,
                exec_path_owned,
                env,
                prompt_owned,
                opts_owned,
                event_tx,
                result_tx,
                cancel_inner,
                timeout,
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
async fn run_turn(
    inner: Arc<Mutex<Option<PersistentClaude>>>,
    exec_path: String,
    env: std::collections::HashMap<String, String>,
    prompt: String,
    opts: ExecOptions,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    cancel_token: CancellationToken,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut guard = inner.lock().await;

    // Decide reuse vs respawn. If we have to throw out the existing process,
    // do it before spawning the next one — both for resource accounting and
    // so the new process doesn't inherit any stdin/stdout state.
    let reused = match guard.take() {
        Some(mut p) => {
            if can_reuse(&mut p, &opts) {
                Some(p)
            } else {
                debug!(pid = p.pid, "discarding incompatible persistent claude");
                p.kill().await;
                None
            }
        }
        None => None,
    };

    let mut p = match reused {
        Some(p) => p,
        None => match spawn_persistent(&exec_path, &env, &opts).await {
            Ok(p) => p,
            Err(e) => {
                let _ = result_tx.send(ExecResult {
                    status: ExecStatus::Failed,
                    output: String::new(),
                    error: Some(format!("failed to spawn claude: {e}")),
                    duration_ms: start.elapsed().as_millis() as u64,
                    session_token: None,
                    usage_report: ProviderUsageReport::default(),
                    usage: None,
                });
                return;
            }
        },
    };

    // Stream the user message into the running CLI.
    if let Err(e) = write_user_message(&mut p.stdin, &prompt).await {
        warn!(pid = p.pid, error = %e, "stdin write failed");
        p.kill().await;
        let _ = result_tx.send(ExecResult {
            status: ExecStatus::Failed,
            output: String::new(),
            error: Some(format!("failed to write user message: {e}")),
            duration_ms: start.elapsed().as_millis() as u64,
            session_token: None,
            usage_report: ProviderUsageReport::default(),
            usage: None,
        });
        return;
    }

    let pid = p.pid;
    info!(pid, cwd = ?opts.cwd, model = ?opts.model, "claude turn started");

    let outcome = drive_one_turn(&mut p, &event_tx, timeout, &cancel_token).await;

    if outcome.keep_process {
        *guard = Some(p);
    } else {
        p.kill().await;
    }

    let duration = start.elapsed();
    info!(
        pid,
        status = ?outcome.status,
        turns = outcome.num_turns,
        ?duration,
        "claude turn finished"
    );

    let _ = result_tx.send(outcome.into_exec_result(duration));
}

fn can_reuse(p: &mut PersistentClaude, opts: &ExecOptions) -> bool {
    if !p.is_alive() {
        return false;
    }
    if p.sig.cwd != opts.cwd || p.sig.model != opts.model {
        return false;
    }
    // Cold-start request (agent_loop passes no resume_token when starting
    // fresh after [[RESET]] or first boot) — never reuse, the agent expects a
    // virgin session.
    let Some(rt) = opts.resume_token.as_ref() else {
        return false;
    };
    p.session_id.as_deref() == Some(rt.as_str())
}

async fn spawn_persistent(
    exec_path: &str,
    env: &std::collections::HashMap<String, String>,
    opts: &ExecOptions,
) -> Result<PersistentClaude, std::io::Error> {
    let mut args = vec![
        "--print".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ];
    if let Some(model) = &opts.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(max_turns) = opts.max_turns {
        args.extend(["--max-turns".to_string(), max_turns.to_string()]);
    }
    if let Some(system_prompt) = &opts.system_prompt {
        args.extend(["--append-system-prompt".to_string(), system_prompt.clone()]);
    }
    if let Some(resume_token) = &opts.resume_token {
        args.extend(["--resume".to_string(), resume_token.clone()]);
    }

    let mut cmd = Command::new(exec_path);
    cmd.args(&args)
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    if let Some(cwd) = &opts.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);

    let stdout = preconditions::take_tokio_piped_stdout(&mut child);
    let stdin = preconditions::take_tokio_piped_stdin(&mut child);
    let stderr = preconditions::take_tokio_piped_stderr(&mut child);

    let stderr_tail = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_handle = spawn_stderr_pump(stderr, stderr_tail.clone());

    info!(pid, "claude spawned (persistent)");

    Ok(PersistentClaude {
        child,
        stdin,
        stdout: BufReader::new(stdout).lines(),
        pid,
        session_id: opts.resume_token.clone(),
        sig: SpawnSig {
            cwd: opts.cwd.clone(),
            model: opts.model.clone(),
        },
        stderr_tail,
        stderr_handle,
    })
}

fn spawn_stderr_pump(
    stderr: ChildStderr,
    tail: Arc<std::sync::Mutex<Vec<String>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "claude:stderr", "{}", line);
            // mutex_lock documents and enforces the poisoned-guard invariant
            let mut t = preconditions::mutex_lock(&tail);
            t.push(line);
            if t.len() > STDERR_TAIL_LINES {
                t.remove(0);
            }
        }
    })
}

async fn write_user_message(stdin: &mut ChildStdin, prompt: &str) -> Result<(), std::io::Error> {
    let msg = serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": prompt,
        }
    });
    let mut buf = preconditions::static_json_to_vec(&msg);
    buf.push(b'\n');
    stdin.write_all(&buf).await?;
    stdin.flush().await?;
    Ok(())
}

struct TurnOutcome {
    status: ExecStatus,
    output: String,
    error: Option<String>,
    session_id: Option<String>,
    num_turns: u32,
    context_usage: Option<ProviderUsage>,
    billing_usage: Option<ProviderUsage>,
    keep_process: bool,
}

impl TurnOutcome {
    fn into_exec_result(self, duration: std::time::Duration) -> ExecResult {
        let report_billing = self.billing_usage.or_else(|| self.context_usage.clone());
        let usage_report = ProviderUsageReport::new(report_billing, self.context_usage);
        let usage = usage_report.billing.clone();
        ExecResult {
            status: self.status,
            output: self.output,
            error: self.error,
            duration_ms: duration.as_millis() as u64,
            session_token: self.session_id,
            usage_report,
            usage,
        }
    }
}

async fn drive_one_turn(
    p: &mut PersistentClaude,
    event_tx: &mpsc::Sender<Event>,
    timeout: Duration,
    cancel_token: &CancellationToken,
) -> TurnOutcome {
    let mut output = String::new();
    let mut session_id: Option<String> = p.session_id.clone();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut saw_result = false;
    let mut num_turns: u32 = 0;
    let mut context_usage: Option<ProviderUsage> = None;
    let mut billing_usage: Option<ProviderUsage> = None;

    let read_result = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                line = p.stdout.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            let parsed = match parse_line(&line) {
                                Some(p) => p,
                                None => {
                                    debug!(pid = p.pid, line_len = line.len(), "unparsed line");
                                    continue;
                                }
                            };

                            match parsed {
                                ParsedMessage::System { session_id: sid } => {
                                    session_id = Some(sid);
                                    try_send_event(event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::AssistantEvents { events, usage } => {
                                    num_turns += 1;
                                    if usage.as_ref().is_some_and(usage_has_reported_tokens) {
                                        context_usage = usage;
                                    }
                                    for event in events {
                                        if let Event::Text { ref content } = event {
                                            output.push_str(content);
                                        }
                                        try_send_event(event_tx, event);
                                    }
                                }
                                ParsedMessage::UserEvents(events) => {
                                    for event in events {
                                        try_send_event(event_tx, event);
                                    }
                                }
                                ParsedMessage::Result {
                                    session_id: sid,
                                    output: result_text,
                                    is_error,
                                    usage: result_usage,
                                } => {
                                    saw_result = true;
                                    session_id = Some(sid);
                                    if result_usage.as_ref().is_some_and(usage_has_reported_tokens) {
                                        billing_usage = result_usage.clone();
                                    }
                                    if should_replace_captured_usage(&context_usage, &result_usage) {
                                        context_usage = result_usage;
                                    }
                                    if is_error {
                                        final_status = ExecStatus::Failed;
                                        final_error = if result_text.is_empty() {
                                            None
                                        } else {
                                            Some(result_text)
                                        };
                                    } else if !result_text.is_empty() {
                                        output = result_text;
                                    }
                                    // `result` marks the end of one turn — return control to the
                                    // caller without killing the CLI.
                                    return;
                                }
                                ParsedMessage::ControlRequest { request_id, input } => {
                                    let response = build_auto_approve_response(&request_id, &input);
                                    if let Ok(data) = serde_json::to_vec(&response) {
                                        let mut buf = data;
                                        buf.push(b'\n');
                                        if let Err(e) = p.stdin.write_all(&buf).await {
                                            warn!("failed to write control response: {e}");
                                        }
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            // stdout closed — CLI exited mid-turn.
                            final_status = ExecStatus::Failed;
                            final_error = Some("claude stdout closed before result".to_string());
                            return;
                        }
                        Err(e) => {
                            warn!(pid = p.pid, error = %e, "stdout read error");
                            final_status = ExecStatus::Failed;
                            final_error = Some(format!("stdout read error: {e}"));
                            return;
                        }
                    }
                }
                _ = cancel_token.cancelled() => {
                    info!(pid = p.pid, "cancelled by steering");
                    final_status = ExecStatus::Aborted;
                    final_error = Some("cancelled by steering".to_string());
                    return;
                }
            }
        }
    })
    .await;

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("claude timed out after {timeout:?}"));
    }

    // Fall back to stderr tail for empty error messages.
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
        let tail = preconditions::mutex_lock(&p.stderr_tail);
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

    // Stream truncated without a `result` event but no other error path
    // triggered — treat as failure.
    if !saw_result && final_status == ExecStatus::Completed {
        final_status = ExecStatus::Failed;
        final_error = Some("claude stream ended without a result message".to_string());
    }

    let keep_process = final_status == ExecStatus::Completed && saw_result && p.is_alive();
    p.session_id = session_id.clone();

    TurnOutcome {
        status: final_status,
        output,
        error: final_error,
        session_id,
        num_turns,
        context_usage,
        billing_usage,
        keep_process,
    }
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

fn should_replace_captured_usage(
    captured_usage: &Option<ProviderUsage>,
    candidate_usage: &Option<ProviderUsage>,
) -> bool {
    captured_usage
        .as_ref()
        .is_none_or(|usage| !usage_has_reported_tokens(usage))
        && candidate_usage
            .as_ref()
            .is_some_and(usage_has_reported_tokens)
}

fn usage_has_reported_tokens(usage: &ProviderUsage) -> bool {
    usage.input_tokens.unwrap_or(0) > 0
        || usage.output_tokens.unwrap_or(0) > 0
        || usage.cache_read_tokens.unwrap_or(0) > 0
        || usage.cache_creation_tokens.unwrap_or(0) > 0
        || usage.context_tokens.unwrap_or(0) > 0
        || usage.context_window_tokens.unwrap_or(0) > 0
        || usage.used_percent.is_some_and(|pct| pct > 0.0)
}

fn build_auto_approve_response(request_id: &str, input: &Value) -> Value {
    let updated_input = if input.is_object() {
        input.clone()
    } else {
        Value::Object(Default::default())
    };
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "allow",
                "updatedInput": updated_input
            }
        }
    })
}

/// Parsed result from a single line of Claude stream-json output.
#[derive(Debug)]
pub enum ParsedMessage {
    /// System init message with session ID.
    System { session_id: String },
    /// Events from an assistant message (text contributes to output).
    AssistantEvents {
        events: Vec<Event>,
        /// Per-iteration usage carried on this assistant message. See the
        /// `MessageContent.usage` doc comment for why this beats
        /// `Result.usage` as a window-occupancy signal.
        usage: Option<ProviderUsage>,
    },
    /// Events from a user message (tool results, not accumulated into output).
    UserEvents(Vec<Event>),
    /// Final result.
    Result {
        session_id: String,
        output: String,
        is_error: bool,
        usage: Option<ProviderUsage>,
    },
    /// Permission control request requiring a response on stdin.
    ControlRequest { request_id: String, input: Value },
}

/// Parse a single line of Claude stream-json output.
/// Returns None for empty lines, malformed JSON, or unrecognized message types.
pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let raw: RawMessage = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "system" => Some(ParsedMessage::System {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "assistant" => {
            let content: MessageContent = serde_json::from_value(raw.message?).ok()?;
            let events = parse_content_blocks(&content);
            if events.is_empty() {
                None
            } else {
                let usage = content.usage.map(|u| ProviderUsage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    used_percent: None,
                    cache_read_tokens: u.cache_read_tokens,
                    cache_creation_tokens: u.cache_creation_tokens,
                    context_tokens: None,
                    context_window_tokens: None,
                });
                Some(ParsedMessage::AssistantEvents { events, usage })
            }
        }
        "user" => {
            let content: MessageContent = serde_json::from_value(raw.message?).ok()?;
            let events = parse_content_blocks(&content);
            if events.is_empty() {
                None
            } else {
                Some(ParsedMessage::UserEvents(events))
            }
        }
        "result" => Some(ParsedMessage::Result {
            session_id: raw.session_id.unwrap_or_default(),
            output: raw.result.unwrap_or_default(),
            is_error: raw.is_error.unwrap_or(false),
            usage: raw.usage.map(|u| ProviderUsage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                used_percent: None,
                cache_read_tokens: u.cache_read_tokens,
                cache_creation_tokens: u.cache_creation_tokens,
                context_tokens: None,
                context_window_tokens: None,
            }),
        }),
        "log" => {
            let log = raw.log?;
            Some(ParsedMessage::AssistantEvents {
                events: vec![Event::Log {
                    level: log.level,
                    content: log.message,
                }],
                usage: None,
            })
        }
        "control_request" => {
            let request: ControlRequestPayload = serde_json::from_value(raw.request?).ok()?;
            let input = request.input.unwrap_or(Value::Object(Default::default()));
            Some(ParsedMessage::ControlRequest {
                request_id: raw.request_id?,
                input,
            })
        }
        _ => None,
    }
}

fn parse_content_blocks(content: &MessageContent) -> Vec<Event> {
    let mut events = Vec::new();
    for block in &content.content {
        match block.r#type.as_str() {
            "text" => {
                if let Some(text) = &block.text {
                    if !text.is_empty() {
                        events.push(Event::Text {
                            content: text.clone(),
                        });
                    }
                }
            }
            "thinking" => {
                if let Some(text) = &block.text {
                    if !text.is_empty() {
                        events.push(Event::Thinking {
                            content: text.clone(),
                        });
                    }
                }
            }
            "tool_use" => {
                let input = block
                    .input
                    .clone()
                    .unwrap_or(Value::Object(Default::default()));
                events.push(Event::ToolUse {
                    tool: block.name.clone().unwrap_or_default(),
                    call_id: block.id.clone().unwrap_or_default(),
                    input,
                });
            }
            "tool_result" => {
                let output = block
                    .tool_result_content
                    .as_ref()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                events.push(Event::ToolResult {
                    call_id: block.tool_use_id.clone().unwrap_or_default(),
                    output,
                });
            }
            _ => {}
        }
    }
    events
}

// ── Claude SDK JSON types (internal) ──

#[derive(Deserialize)]
struct RawMessage {
    r#type: String,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
    #[serde(default)]
    log: Option<LogEntry>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    request: Option<Value>,
}

/// Usage block from Claude stream-json — appears on both per-iteration
/// `assistant` messages and the final `result` message.
///
/// Anthropic reports `input_tokens` **excluding** cache hits. Real
/// context-window occupancy is the sum `input + cache_read +
/// cache_creation`, so all three are propagated into `ProviderUsage`
/// and aggregated by the runtime when computing percentages.
#[derive(Debug, Clone, Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default, rename = "cache_read_input_tokens")]
    cache_read_tokens: Option<u64>,
    #[serde(default, rename = "cache_creation_input_tokens")]
    cache_creation_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct LogEntry {
    level: String,
    message: String,
}

#[derive(Deserialize)]
struct MessageContent {
    #[serde(default)]
    content: Vec<ContentBlock>,
    /// Per-iteration usage from a `type:assistant` message.
    ///
    /// Claude Code CLI emits one `assistant` message per inference step inside
    /// a single CLI invocation. The `usage` here is scoped to *that* request,
    /// so the last iteration's value reflects the actual context-window
    /// occupancy at turn end. The final `type:result` event carries an
    /// aggregate across iterations that double-counts cached context; prefer
    /// this field when available.
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    tool_use_id: Option<String>,
    /// The "content" field in tool_result blocks.
    /// Renamed to avoid conflict with the struct field name in ContentBlock list.
    #[serde(default, rename = "content")]
    tool_result_content: Option<Value>,
}

#[derive(Deserialize)]
struct ControlRequestPayload {
    #[serde(default)]
    input: Option<Value>,
}

#[cfg(test)]
mod usage_parse_tests {
    use super::*;

    #[test]
    fn parse_result_with_usage_block() {
        let line = r#"{
            "type": "result",
            "session_id": "sess-abc",
            "result": "hello",
            "is_error": false,
            "usage": {
                "input_tokens": 164000,
                "output_tokens": 520,
                "cache_read_input_tokens": 120000,
                "cache_creation_input_tokens": 800
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::Result { usage, .. } = parsed else {
            panic!("expected Result variant");
        };
        let usage = usage.expect("usage field present");
        assert_eq!(usage.input_tokens, Some(164_000));
        assert_eq!(usage.output_tokens, Some(520));
        assert_eq!(usage.cache_read_tokens, Some(120_000));
        assert_eq!(usage.cache_creation_tokens, Some(800));
    }

    #[test]
    fn parse_result_with_cache_only_has_tiny_input() {
        // Real-world turn 2+: prompt caching active. input_tokens is just the
        // delta; the bulk of context comes through cache_read_input_tokens.
        // This is the scenario that produced the "0%" display bug.
        let line = r#"{
            "type": "result",
            "session_id": "sess-xyz",
            "result": "ok",
            "is_error": false,
            "usage": {
                "input_tokens": 312,
                "output_tokens": 180,
                "cache_read_input_tokens": 159500,
                "cache_creation_input_tokens": 220
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::Result { usage, .. } = parsed else {
            panic!("expected Result variant");
        };
        let usage = usage.expect("usage field present");
        assert_eq!(usage.input_tokens, Some(312));
        assert_eq!(usage.cache_read_tokens, Some(159_500));
        assert_eq!(usage.cache_creation_tokens, Some(220));
    }

    #[test]
    fn assistant_message_surfaces_per_iteration_usage() {
        // Per-iteration usage carried on the assistant message is the
        // authoritative window-occupancy signal. The aggregated
        // `result.usage` is a sum across all iterations and inflates the
        // denominator (N× cached context) — never use it for occupancy.
        let line = r#"{
            "type": "assistant",
            "session_id": "sess-abc",
            "message": {
                "content": [{"type": "text", "text": "ok"}],
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 34,
                    "cache_read_input_tokens": 59560,
                    "cache_creation_input_tokens": 325
                }
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::AssistantEvents { usage, .. } = parsed else {
            panic!("expected AssistantEvents variant");
        };
        let usage = usage.expect("per-iteration usage present");
        assert_eq!(usage.input_tokens, Some(1));
        assert_eq!(usage.cache_read_tokens, Some(59_560));
        assert_eq!(usage.cache_creation_tokens, Some(325));
        assert!(
            usage.used_percent.is_none(),
            "Claude never sets used_percent"
        );

        // Effective = 59,886 — the actual window-occupancy number the runtime
        // should divide into max_tokens. The aggregated 177k across 3
        // iterations must never be produced by the parser.
        let effective = usage.input_tokens.unwrap_or(0)
            + usage.cache_read_tokens.unwrap_or(0)
            + usage.cache_creation_tokens.unwrap_or(0);
        assert_eq!(effective, 59_886);
    }

    #[test]
    fn assistant_message_without_usage_field_ok() {
        // Older Claude CLI versions may omit usage on assistant messages; the
        // runtime then falls back to result.usage. Make sure parse doesn't
        // reject the line.
        let line = r#"{
            "type": "assistant",
            "session_id": "sess-abc",
            "message": {
                "content": [{"type": "text", "text": "hi"}]
            }
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::AssistantEvents { usage, events } = parsed else {
            panic!("expected AssistantEvents variant");
        };
        assert!(usage.is_none());
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_result_without_usage_block_ok() {
        let line = r#"{
            "type": "result",
            "session_id": "sess-abc",
            "result": "hello",
            "is_error": false
        }"#;

        let parsed = parse_line(line).expect("should parse");
        let ParsedMessage::Result { usage, .. } = parsed else {
            panic!("expected Result variant");
        };
        assert!(usage.is_none());
    }

    #[test]
    fn zero_usage_assistant_event_does_not_block_result_usage_fallback() {
        // Some Anthropic-compatible Claude Code backends emit a zero-filled
        // assistant usage placeholder, then put the real token counts on the
        // final result event.
        let assistant_line = r#"{
            "type": "assistant",
            "session_id": "sess-glm",
            "message": {
                "content": [{"type": "text", "text": "ok"}],
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0
                }
            }
        }"#;
        let result_line = r#"{
            "type": "result",
            "session_id": "sess-glm",
            "result": "ok",
            "is_error": false,
            "usage": {
                "input_tokens": 51512,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 7,
                "server_tool_use": {
                    "web_search_requests": 0,
                    "web_fetch_requests": 0
                },
                "service_tier": "standard"
            }
        }"#;

        let ParsedMessage::AssistantEvents {
            usage: assistant_usage,
            ..
        } = parse_line(assistant_line).expect("assistant line should parse")
        else {
            panic!("expected AssistantEvents variant");
        };
        assert!(
            !usage_has_reported_tokens(assistant_usage.as_ref().expect("usage is present")),
            "zero placeholder should not count as provider usage"
        );

        let mut captured_usage = None;
        if assistant_usage
            .as_ref()
            .is_some_and(usage_has_reported_tokens)
        {
            captured_usage = assistant_usage;
        }

        let ParsedMessage::Result {
            usage: result_usage,
            ..
        } = parse_line(result_line).expect("result line should parse")
        else {
            panic!("expected Result variant");
        };
        assert!(should_replace_captured_usage(
            &captured_usage,
            &result_usage
        ));
        if should_replace_captured_usage(&captured_usage, &result_usage) {
            captured_usage = result_usage;
        }

        let usage = captured_usage.expect("result usage should be captured");
        assert_eq!(usage.input_tokens, Some(51_512));
        assert_eq!(usage.output_tokens, Some(7));
    }

    #[test]
    fn nonzero_assistant_usage_stays_authoritative_over_result_usage() {
        let assistant_usage = Some(ProviderUsage {
            input_tokens: Some(1),
            output_tokens: Some(34),
            cache_read_tokens: Some(59_560),
            cache_creation_tokens: Some(325),
            ..ProviderUsage::default()
        });
        let result_usage = Some(ProviderUsage {
            input_tokens: Some(120_000),
            output_tokens: Some(80),
            cache_read_tokens: Some(60_000),
            cache_creation_tokens: Some(500),
            ..ProviderUsage::default()
        });

        assert!(!should_replace_captured_usage(
            &assistant_usage,
            &result_usage
        ));
    }
}
