use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    ProviderUsage, ProviderUsageReport, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;
const STDERR_TAIL_LINES: usize = 20;
const STDERR_TAIL_CHARS: usize = 4000;
const DEFAULT_REASONING_EFFORT: &str = "xhigh";

type SharedStderrTail = Arc<Mutex<VecDeque<String>>>;

pub struct CodexProvider {
    config: ProviderConfig,
}

impl CodexProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for CodexProvider {
    /// codex stdout's `turn.completed.usage.input_tokens` is the running
    /// session total (verified by resume probe — see
    /// `tests/provider_trait_declarations.rs::codex_reports_cumulative_session_usage`).
    /// `normalize_to_delta` subtracts the per-session baseline so the
    /// accumulator gets per-turn deltas. The HUD must not use that raw
    /// cumulative input as context occupancy; current context is attached
    /// separately from rollout `token_count` when available.
    fn usage_is_cumulative(&self) -> bool {
        true
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "codex".to_string());

        let resolved_path =
            crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
                path: exec_path.clone(),
            })?;
        let resolved_display = resolved_path.display().to_string();
        let canonical_display = std::fs::canonicalize(&resolved_path)
            .ok()
            .map(|p| p.display().to_string());
        let version = query_cli_version(&resolved_path);

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let prompt = build_prompt(prompt, opts.system_prompt.as_deref());

        let mut args = vec!["exec".to_string()];
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["resume".to_string(), resume_token.clone()]);
        }
        args.push("--json".to_string());
        // bypass sandbox — agents need to run gitim commands without approval prompts
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        args.extend([
            "-c".to_string(),
            format!("model_reasoning_effort=\"{DEFAULT_REASONING_EFFORT}\""),
        ]);
        if let Some(model) = &opts.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        args.push(prompt);

        let mut cmd = Command::new(&resolved_path);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
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
        info!(
            pid,
            cwd = ?opts.cwd,
            model = ?opts.model,
            exec = %resolved_display,
            canonical_exec = ?canonical_display,
            version = ?version,
            resume = opts.resume_token.is_some(),
            prompt_len = args.last().map(|p| p.len()).unwrap_or(0),
            "codex started"
        );

        // INVARIANT: `Command::stdout()`/`stderr()` return `Some` when
        // the corresponding `Stdio` is set to `piped()`. We always configure
        // `stdout(Stdio::piped())` etc., so these are always `Some`.
        #[allow(clippy::expect_used)]
        let stdout = child.stdout.take().expect("stdout piped");
        #[allow(clippy::expect_used)]
        let stderr = child.stderr.take().expect("stderr piped");
        let codex_home = codex_home_from_env(&self.config.env);

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let stderr_tail = stderr_tail();
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
                stderr_tail,
                cancel_token_inner,
                codex_home,
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
    stderr_tail: SharedStderrTail,
    cancel_token: CancellationToken,
    codex_home: Option<PathBuf>,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut thread_id: Option<String> = None;
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut saw_turn_completed = false;
    let mut latest_usage: Option<ProviderUsage> = None;
    let mut last_live_context_usage: Option<ProviderUsage> = None;
    let mut live_rollout_path: Option<PathBuf> = None;
    let mut live_rollout_len: Option<u64> = None;

    let mut reader = BufReader::new(stdout).lines();

    let stderr_tail_inner = Arc::clone(&stderr_tail);
    let stderr_handle = tokio::spawn(async move {
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "codex:stderr", "{}", line);
            push_stderr_tail(&stderr_tail_inner, line);
        }
    });

    let read_result = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                line = reader.next_line() => {
                    let line = match line {
                        Ok(Some(line)) => line.trim().to_string(),
                        Ok(None) => break,
                        Err(e) => {
                            warn!(pid, error = %e, "stdout read error");
                            break;
                        }
                    };
                    if line.is_empty() {
                        continue;
                    }

                    if let Some(parsed) = parse_line(&line) {
                        match parsed {
                            ParsedMessage::ThreadStarted { id } => {
                                thread_id = Some(id);
                                try_send_event(
                                    &event_tx,
                                    Event::Status {
                                        status: "running".to_string(),
                                    },
                                );
                            }
                            ParsedMessage::Text { content } => {
                                append_output(&mut output, &content);
                                try_send_event(&event_tx, Event::Text { content });
                            }
                            ParsedMessage::ToolUse { call_id, command } => {
                                try_send_event(
                                    &event_tx,
                                    Event::ToolUse {
                                        tool: "Bash".to_string(),
                                        call_id,
                                        input: json!({ "command": command }),
                                    },
                                );
                            }
                            ParsedMessage::ToolResult { call_id, output } => {
                                try_send_event(&event_tx, Event::ToolResult { call_id, output });
                            }
                            ParsedMessage::TurnCompleted { usage } => {
                                // `turn.completed.usage` is the only billing
                                // signal codex emits on stdout, and it's
                                // session-cumulative. The codex `Provider`
                                // impl declares `usage_is_cumulative() == true`
                                // so the runtime subtracts a per-session
                                // baseline for the accumulator. Context-window
                                // occupancy is filled separately from rollout
                                // `token_count` events scanned by thread_id.
                                if let Some(u) = usage {
                                    latest_usage = Some(u);
                                }
                                saw_turn_completed = true;
                            }
                        }
                    }
                    maybe_send_live_context_usage(
                        &event_tx,
                        codex_home.as_deref(),
                        thread_id.as_deref(),
                        &mut live_rollout_path,
                        &mut live_rollout_len,
                        &mut last_live_context_usage,
                    );
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
        final_error = Some(format!("codex timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    }

    match child.wait().await {
        Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
            final_status = ExecStatus::Failed;
            final_error = Some(format!("codex exited with status: {status}"));
        }
        Err(e) if final_status == ExecStatus::Completed => {
            final_status = ExecStatus::Failed;
            final_error = Some(format!("failed to wait for codex: {e}"));
        }
        _ => {}
    }

    if final_status == ExecStatus::Completed && !saw_turn_completed {
        final_status = ExecStatus::Failed;
        final_error = Some("codex stream ended without turn.completed".to_string());
    }

    let billing_usage = latest_usage;
    let mut legacy_usage = billing_usage.clone();
    let mut final_context_usage: Option<ProviderUsage> = None;
    if let Some(tid) = thread_id.as_deref() {
        if let Some(context_usage) = read_rollout_context_usage(codex_home.as_deref(), tid) {
            final_context_usage = Some(context_usage.clone());
            attach_context_usage(&mut legacy_usage, context_usage);
        }
    }

    if final_status == ExecStatus::Failed {
        // Codex writes richer diagnostics to the session rollout file than it
        // streams to stdout, so failed turns get one more pass over that file.
        if let Some(tid) = thread_id.as_deref() {
            if let Some(reason) = diagnose_rollout_failure(codex_home.as_deref(), tid) {
                final_error = append_failure_detail(final_error, reason);
            }
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "codex finished");

    if matches!(final_status, ExecStatus::Aborted | ExecStatus::Timeout) {
        stderr_handle.abort();
    } else {
        let _ = stderr_handle.await;
    }

    if final_status != ExecStatus::Completed {
        if let Some(stderr) = format_stderr_tail(&stderr_tail) {
            final_error = append_failure_detail(final_error, stderr);
        }
    }

    let _ = result_tx.send(ExecResult {
        status: final_status,
        output,
        error: final_error,
        duration_ms: duration.as_millis() as u64,
        session_token: thread_id,
        usage_report: ProviderUsageReport::new(billing_usage, final_context_usage),
        usage: legacy_usage,
    });
}

fn build_prompt(prompt: &str, system_prompt: Option<&str>) -> String {
    match system_prompt {
        Some(system_prompt) if !system_prompt.is_empty() => {
            format!("{system_prompt}\n\n{prompt}")
        }
        _ => prompt.to_string(),
    }
}

fn append_output(output: &mut String, content: &str) {
    if output.is_empty() {
        output.push_str(content);
    } else {
        output.push('\n');
        output.push_str(content);
    }
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

fn query_cli_version(path: &Path) -> Option<String> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|line| line.trim().to_string())
}

fn stderr_tail() -> SharedStderrTail {
    Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)))
}

fn push_stderr_tail(tail: &SharedStderrTail, line: String) {
    let Ok(mut guard) = tail.lock() else {
        return;
    };
    if guard.len() == STDERR_TAIL_LINES {
        guard.pop_front();
    }
    guard.push_back(line);
}

fn format_stderr_tail(tail: &SharedStderrTail) -> Option<String> {
    let Ok(guard) = tail.lock() else {
        return None;
    };
    let joined = guard
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.is_empty() {
        None
    } else {
        Some(format!(
            "codex stderr tail: {}",
            truncate_chars(&joined, STDERR_TAIL_CHARS)
        ))
    }
}

fn append_failure_detail(current: Option<String>, detail: String) -> Option<String> {
    Some(match current {
        Some(prev) if !prev.is_empty() => format!("{prev}; {detail}"),
        _ => detail,
    })
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[derive(Debug)]
enum ParsedMessage {
    ThreadStarted {
        id: String,
    },
    Text {
        content: String,
    },
    ToolUse {
        call_id: String,
        command: String,
    },
    ToolResult {
        call_id: String,
        output: String,
    },
    /// codex CLI 0.130.0-alpha.5 emits one of these per `codex exec`
    /// invocation, with cumulative session usage at the top level. The
    /// `usage` field is `Some` whenever the LLM call(s) inside the turn
    /// produced billing data; `None` for empty/error turns or older builds.
    TurnCompleted {
        usage: Option<ProviderUsage>,
    },
}

fn parse_line(line: &str) -> Option<ParsedMessage> {
    let raw: RawLine = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "thread.started" => Some(ParsedMessage::ThreadStarted { id: raw.thread_id? }),
        "item.started" | "item.completed" => {
            let item = raw.item?;
            match item.r#type.as_str() {
                "agent_message" => Some(ParsedMessage::Text {
                    content: item.text?,
                }),
                "command_execution" if raw.r#type == "item.started" => {
                    Some(ParsedMessage::ToolUse {
                        call_id: item.id?,
                        command: item.command?,
                    })
                }
                "command_execution" if raw.r#type == "item.completed" => {
                    Some(ParsedMessage::ToolResult {
                        call_id: item.id?,
                        output: item.aggregated_output.unwrap_or_default(),
                    })
                }
                _ => None,
            }
        }
        "turn.completed" => {
            // `usage` on stdout uses the same field names as Codex token
            // usage structs, but the values are session-cumulative billing
            // totals. Current window context comes from rollout token_count.
            // Empty or missing usage is fine: turn completion alone is still
            // a useful stream-end signal.
            let usage = raw.usage.as_ref().and_then(parse_codex_usage);
            Some(ParsedMessage::TurnCompleted { usage })
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct RawLine {
    r#type: String,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    item: Option<RawItem>,
    /// Only populated for `turn.completed` events. Kept as a raw `Value`
    /// so the parser is forgiving about extra fields codex may add
    /// (timestamps, request ids, etc.) without breaking deserialization.
    #[serde(default)]
    usage: Option<serde_json::Value>,
}

/// Look up the codex session rollout file for `thread_id` and extract a
/// human-readable failure reason if one of the known patterns appears.
/// Returns None when nothing matches — callers fall back to the generic
/// "exit status" message.
fn diagnose_rollout_failure(codex_home: Option<&Path>, thread_id: &str) -> Option<String> {
    let codex_home = codex_home?;
    let rollout = find_rollout_file(&codex_home.join("sessions"), thread_id)?;
    let content = std::fs::read_to_string(&rollout).ok()?;
    scan_rollout_content(&content)
}

fn read_rollout_context_usage(codex_home: Option<&Path>, thread_id: &str) -> Option<ProviderUsage> {
    let codex_home = codex_home?;
    let rollout = find_rollout_file(&codex_home.join("sessions"), thread_id)?;
    read_rollout_context_usage_from_path(&rollout)
}

fn read_rollout_context_usage_from_path(rollout: &Path) -> Option<ProviderUsage> {
    let content = std::fs::read_to_string(rollout).ok()?;
    scan_rollout_context_usage(&content)
}

fn attach_context_usage(latest_usage: &mut Option<ProviderUsage>, context_usage: ProviderUsage) {
    let usage = latest_usage.get_or_insert_with(ProviderUsage::default);
    usage.context_tokens = context_usage.context_tokens;
    usage.context_window_tokens = context_usage.context_window_tokens;
}

fn maybe_send_live_context_usage(
    event_tx: &mpsc::Sender<Event>,
    codex_home: Option<&Path>,
    thread_id: Option<&str>,
    rollout_path: &mut Option<PathBuf>,
    rollout_len: &mut Option<u64>,
    last_sent: &mut Option<ProviderUsage>,
) {
    let Some(thread_id) = thread_id else {
        return;
    };
    let Some(codex_home) = codex_home else {
        return;
    };
    let Some(path) = rollout_path_for_thread(codex_home, rollout_path, thread_id) else {
        return;
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return;
    };
    let len = metadata.len();
    if rollout_len.is_some_and(|prev| prev == len) {
        return;
    }
    *rollout_len = Some(len);
    let Some(context_usage) = read_rollout_context_usage_from_path(&path) else {
        return;
    };
    if same_context_usage(last_sent.as_ref(), &context_usage) {
        return;
    }
    *last_sent = Some(context_usage.clone());
    try_send_event(
        event_tx,
        Event::Usage {
            session_id: thread_id.to_string(),
            usage: context_usage,
        },
    );
}

fn rollout_path_for_thread(
    codex_home: &Path,
    cached: &mut Option<PathBuf>,
    thread_id: &str,
) -> Option<PathBuf> {
    if let Some(path) = cached.as_ref().filter(|path| path.exists()) {
        return Some(path.clone());
    }
    let path = find_rollout_file(&codex_home.join("sessions"), thread_id)?;
    *cached = Some(path.clone());
    Some(path)
}

fn same_context_usage(previous: Option<&ProviderUsage>, current: &ProviderUsage) -> bool {
    previous.is_some_and(|prev| {
        prev.context_tokens == current.context_tokens
            && prev.context_window_tokens == current.context_window_tokens
    })
}

fn codex_home_from_env(
    provider_env: &std::collections::HashMap<String, String>,
) -> Option<PathBuf> {
    if let Some(dir) = provider_env.get("CODEX_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".codex"))
}

/// Walk `sessions/YYYY/MM/DD/` looking for `rollout-*-<thread_id>.jsonl`.
/// Codex encodes the thread UUID as the filename suffix, so this stays cheap.
fn find_rollout_file(sessions_dir: &Path, thread_id: &str) -> Option<PathBuf> {
    let suffix = format!("-{thread_id}.jsonl");
    for year in read_subdirs(sessions_dir) {
        for month in read_subdirs(&year) {
            for day in read_subdirs(&month) {
                let entries = std::fs::read_dir(&day).ok()?;
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.ends_with(&suffix) {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn scan_rollout_content(content: &str) -> Option<String> {
    // Scan backward: the terminal failure signal is always near the tail.
    // Cap the walk so a very large rollout doesn't chew CPU.
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(500);
    let mut credits_exhausted: Option<String> = None;
    let mut missing_final_message: Option<String> = None;
    for line in lines[start..].iter().rev() {
        if let Some(msg) = parse_rollout_line(line) {
            return Some(msg);
        }
        if credits_exhausted.is_none() {
            credits_exhausted = parse_credits_exhausted(line);
        }
        if missing_final_message.is_none() {
            missing_final_message = parse_missing_final_message(line);
        }
    }
    credits_exhausted.or(missing_final_message)
}

fn scan_rollout_context_usage(content: &str) -> Option<ProviderUsage> {
    for line in content.lines().rev() {
        if let Some(usage) = parse_token_count_context_usage(line) {
            return Some(usage);
        }
    }
    None
}

fn parse_token_count_context_usage(line: &str) -> Option<ProviderUsage> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.pointer("/payload/type")?.as_str()? != "token_count" {
        return None;
    }
    let context_tokens = v
        .pointer("/payload/info/last_token_usage/total_tokens")
        .and_then(Value::as_u64)?;
    let context_window = v
        .pointer("/payload/info/model_context_window")
        .and_then(Value::as_u64)?;
    Some(ProviderUsage {
        context_tokens: Some(context_tokens),
        context_window_tokens: Some(context_window),
        ..Default::default()
    })
}

/// Strong signal: a `response_item.message` with an embedded
/// `<subagent_notification>` whose status is `errored`.
fn parse_rollout_line(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "response_item" {
        return None;
    }
    let content = v.pointer("/payload/content")?.as_array()?;
    for item in content {
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            continue;
        };
        if let Some(msg) = extract_subagent_error(text) {
            return Some(format!("codex subagent errored: {msg}"));
        }
    }
    None
}

/// Project a codex usage JSON object into the provider-agnostic shape.
///
/// Accepts the field layout codex uses for both `turn.completed.usage`
/// (stdout) and `last_token_usage` / `total_token_usage` (rollout file):
/// - `input_tokens` — session-cumulative input billing on stdout; the
///   runtime's `normalize_to_delta` (with `usage_is_cumulative()==true`)
///   subtracts a per-session baseline to recover per-turn deltas
/// - `cached_input_tokens` → `cache_read_tokens`
/// - `output_tokens` + `reasoning_output_tokens` → folded into
///   `output_tokens` (reasoning models bill these separately but they
///   consume context just the same)
/// - `cache_creation_tokens` left `None` — codex's prompt cache is
///   server-managed; the protocol only surfaces the read side
/// - `used_percent` left `None`; context occupancy comes from rollout
///   `token_count.info.last_token_usage.total_tokens`, not stdout
///
/// Returns `None` when all three primary fields are missing — that's the
/// shape codex emits for empty / error turns and the placeholder `{}`
/// usage object.
fn parse_codex_usage(total: &Value) -> Option<ProviderUsage> {
    let input = total
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64);
    let cached = total
        .get("cached_input_tokens")
        .and_then(serde_json::Value::as_u64);
    let output = total
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64);
    let reasoning = total
        .get("reasoning_output_tokens")
        .and_then(serde_json::Value::as_u64);
    if input.is_none() && cached.is_none() && output.is_none() {
        return None;
    }
    let output_total = match (output, reasoning) {
        (Some(o), Some(r)) => Some(o.saturating_add(r)),
        (Some(o), None) => Some(o),
        (None, Some(r)) => Some(r),
        (None, None) => None,
    };
    Some(ProviderUsage {
        input_tokens: input,
        output_tokens: output_total,
        used_percent: None,
        cache_read_tokens: cached,
        cache_creation_tokens: None,
        context_tokens: None,
        context_window_tokens: None,
    })
}

/// Weaker fallback: a `token_count` event reporting `credits.balance == "0"`
/// with `has_credits: false`. We surface this only if no stronger signal
/// was found — callers prefer the subagent text when both appear.
fn parse_credits_exhausted(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    let payload_type = v.pointer("/payload/type")?.as_str()?;
    if payload_type != "token_count" {
        return None;
    }
    let credits = v.pointer("/payload/rate_limits/credits")?;
    if credits.is_null() {
        return None;
    }
    if credits.get("has_credits")? == &Value::Bool(false)
        && credits.get("balance").and_then(Value::as_str) == Some("0")
    {
        return Some(
            "codex credits exhausted (rate_limits.credits.balance=0, has_credits=false)"
                .to_string(),
        );
    }
    None
}

fn parse_missing_final_message(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    let payload_type = v.pointer("/payload/type")?.as_str()?;
    if payload_type != "task_complete" {
        return None;
    }
    match v.pointer("/payload/last_agent_message") {
        Some(Value::Null) => {
            Some("codex task completed without final assistant message".to_string())
        }
        _ => None,
    }
}

fn extract_subagent_error(text: &str) -> Option<String> {
    const OPEN: &str = "<subagent_notification>";
    const CLOSE: &str = "</subagent_notification>";
    let open_idx = text.find(OPEN)?;
    let inner_start = open_idx + OPEN.len();
    let close_rel = text[inner_start..].find(CLOSE)?;
    let inner = &text[inner_start..inner_start + close_rel];
    let v: Value = serde_json::from_str(inner).ok()?;
    v.pointer("/status/errored")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

#[derive(Deserialize)]
struct RawItem {
    #[serde(default)]
    id: Option<String>,
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    aggregated_output: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    status: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    exit_code: Option<i32>,
    #[allow(dead_code)]
    #[serde(default)]
    metadata: Option<Value>,
}

#[cfg(test)]
mod rollout_tests {
    use super::*;

    const SUBAGENT_ERRORED_LINE: &str = r#"{"timestamp":"2026-04-19T15:21:30.483Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<subagent_notification>\n{\"agent_path\":\"019da651-72b3-72a3-9646-f14eb02e3258\",\"status\":{\"errored\":\"You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Apr 20th, 2026 3:51 AM.\"}}\n</subagent_notification>"}]}}"#;

    const CREDITS_EXHAUSTED_LINE: &str = r#"{"timestamp":"2026-04-19T15:21:31.459Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":1},"last_token_usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":1},"model_context_window":258400},"rate_limits":{"limit_id":"premium","limit_name":null,"primary":null,"secondary":null,"credits":{"has_credits":false,"unlimited":false,"balance":"0"},"plan_type":"plus"}}}"#;
    const TASK_COMPLETE_NO_MESSAGE_LINE: &str = r#"{"timestamp":"2026-04-24T04:23:43.000Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":null}}"#;

    #[test]
    fn parse_rollout_line_extracts_subagent_errored_status() {
        let msg = parse_rollout_line(SUBAGENT_ERRORED_LINE).expect("should parse");
        assert!(msg.starts_with("codex subagent errored:"));
        assert!(msg.contains("hit your usage limit"));
        assert!(msg.contains("Apr 20th, 2026 3:51 AM"));
    }

    #[test]
    fn parse_rollout_line_ignores_unrelated_response_items() {
        let line = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"just a normal reply"}]}}"#;
        assert!(parse_rollout_line(line).is_none());
    }

    #[test]
    fn parse_rollout_line_ignores_non_response_item_types() {
        let line = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"hi"}}"#;
        assert!(parse_rollout_line(line).is_none());
    }

    #[test]
    fn parse_credits_exhausted_detects_zero_balance() {
        let msg = parse_credits_exhausted(CREDITS_EXHAUSTED_LINE).expect("should detect");
        assert!(msg.contains("credits exhausted"));
    }

    #[test]
    fn parse_credits_exhausted_skips_healthy_token_count() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"credits":{"has_credits":true,"unlimited":false,"balance":"100"}}}}"#;
        assert!(parse_credits_exhausted(line).is_none());
    }

    #[test]
    fn parse_credits_exhausted_tolerates_null_credits() {
        // Mid-session token_count events carry `credits: null` — those
        // must not trip the exhaustion detector.
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"limit_id":"codex","primary":{"used_percent":85.0},"credits":null,"plan_type":"plus"}}}"#;
        assert!(parse_credits_exhausted(line).is_none());
    }

    #[test]
    fn parse_missing_final_message_detects_null_last_agent_message() {
        let msg = parse_missing_final_message(TASK_COMPLETE_NO_MESSAGE_LINE)
            .expect("should detect missing final message");
        assert!(msg.contains("without final assistant message"));
    }

    #[test]
    fn scan_rollout_content_prefers_subagent_error_over_credits() {
        let content = format!(
            "{}\n{}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\"}}}}",
            CREDITS_EXHAUSTED_LINE, SUBAGENT_ERRORED_LINE,
        );
        let msg = scan_rollout_content(&content).expect("should find error");
        assert!(msg.starts_with("codex subagent errored:"));
    }

    #[test]
    fn scan_rollout_content_falls_back_to_credits_when_no_subagent_error() {
        let content = format!(
            "{}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_complete\"}}}}",
            CREDITS_EXHAUSTED_LINE,
        );
        let msg = scan_rollout_content(&content).expect("should find credits signal");
        assert!(msg.contains("credits exhausted"));
    }

    #[test]
    fn scan_rollout_content_reports_missing_final_message() {
        let msg =
            scan_rollout_content(TASK_COMPLETE_NO_MESSAGE_LINE).expect("should find task_complete");
        assert!(msg.contains("without final assistant message"));
    }

    #[test]
    fn scan_rollout_content_returns_none_for_clean_run() {
        let content = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}
{"type":"event_msg","payload":{"type":"turn.completed"}}"#;
        assert!(scan_rollout_content(content).is_none());
    }

    #[test]
    fn scan_rollout_context_usage_reads_last_token_count() {
        let earlier = r#"{"timestamp":"2026-05-14T09:46:01.602Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":12094493,"cached_input_tokens":11403904,"output_tokens":57855,"reasoning_output_tokens":20011,"total_tokens":12152348},"last_token_usage":{"input_tokens":88362,"cached_input_tokens":87936,"output_tokens":37,"reasoning_output_tokens":0,"total_tokens":88399},"model_context_window":258400},"rate_limits":null}}"#;
        let latest = r#"{"timestamp":"2026-05-14T09:46:16.121Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":12184040,"cached_input_tokens":11479040,"output_tokens":57985,"reasoning_output_tokens":20011,"total_tokens":12242025},"last_token_usage":{"input_tokens":89547,"cached_input_tokens":75136,"output_tokens":130,"reasoning_output_tokens":0,"total_tokens":89677},"model_context_window":258400},"rate_limits":null}}"#;
        let content = format!("{earlier}\n{latest}");

        let usage = scan_rollout_context_usage(&content).expect("context usage");

        assert_eq!(usage.context_tokens, Some(89_677));
        assert_eq!(usage.context_window_tokens, Some(258_400));
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.cache_read_tokens, None);
    }

    #[test]
    fn attach_context_usage_preserves_cumulative_billing_fields() {
        let mut latest_usage = Some(ProviderUsage {
            input_tokens: Some(12_184_040),
            output_tokens: Some(77_996),
            cache_read_tokens: Some(11_479_040),
            ..Default::default()
        });
        let context_usage = ProviderUsage {
            context_tokens: Some(89_677),
            context_window_tokens: Some(258_400),
            ..Default::default()
        };

        attach_context_usage(&mut latest_usage, context_usage);

        let usage = latest_usage.expect("usage");
        assert_eq!(usage.input_tokens, Some(12_184_040));
        assert_eq!(usage.output_tokens, Some(77_996));
        assert_eq!(usage.cache_read_tokens, Some(11_479_040));
        assert_eq!(usage.context_tokens, Some(89_677));
        assert_eq!(usage.context_window_tokens, Some(258_400));
    }

    #[test]
    fn extract_subagent_error_tolerates_surrounding_whitespace() {
        let text = "<subagent_notification>\n{\"status\":{\"errored\":\"boom\"}}\n</subagent_notification>";
        assert_eq!(extract_subagent_error(text).as_deref(), Some("boom"));
    }

    #[test]
    fn extract_subagent_error_returns_none_without_errored_field() {
        let text = "<subagent_notification>{\"status\":{\"completed\":{}}}</subagent_notification>";
        assert!(extract_subagent_error(text).is_none());
    }

    /// Optional end-to-end probe: point `CODEX_ROLLOUT_FIXTURE` at a real
    /// failed rollout jsonl and run `cargo test -- --ignored` to confirm
    /// the scanner extracts a useful message. Skipped by default because
    /// it depends on a local file path.
    #[allow(clippy::print_stderr)]
    #[test]
    #[ignore]
    fn scan_rollout_content_on_real_fixture() {
        let Ok(path) = std::env::var("CODEX_ROLLOUT_FIXTURE") else {
            return;
        };
        let content =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        let msg =
            scan_rollout_content(&content).expect("fixture should contain a known failure signal");
        eprintln!("extracted: {msg}");
    }

    #[test]
    fn find_rollout_file_locates_by_thread_id_suffix() {
        let tmp = tempdir();
        let day = tmp.join("2026").join("04").join("19");
        std::fs::create_dir_all(&day).unwrap();
        let thread_id = "019da64f-b453-7710-a8ec-4a755faa1cdd";
        let name = format!("rollout-2026-04-19T23-15-34-{thread_id}.jsonl");
        let path = day.join(&name);
        std::fs::write(&path, "{}").unwrap();
        // A decoy file with a different thread_id must be ignored.
        std::fs::write(
            day.join("rollout-2026-04-19T00-00-00-deadbeef-0000-0000-0000-000000000000.jsonl"),
            "{}",
        )
        .unwrap();

        let found = find_rollout_file(&tmp, thread_id).expect("should locate rollout");
        assert_eq!(found, path);
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "gitim-codex-rollout-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}

#[cfg(test)]
mod usage_parse_tests {
    use super::*;

    #[test]
    fn parse_turn_completed_extracts_cumulative_usage() {
        // Fixture from codex CLI stdout: one `turn.completed` per
        // `codex exec` invocation, with `usage` at the top level (not
        // nested under `payload.info`). Counts are session-cumulative
        // across `codex exec resume` calls. We surface them as-is for
        // billing; the runtime's `normalize_to_delta(cumulative=true)`
        // path subtracts the per-session baseline so the accumulator
        // gets per-turn deltas. Context occupancy comes from rollout
        // `token_count`, not this stdout object.
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":45700,"cached_input_tokens":34048,"output_tokens":28,"reasoning_output_tokens":16}}"#;
        let parsed = parse_line(line).expect("turn.completed parses");
        match parsed {
            ParsedMessage::TurnCompleted { usage } => {
                let usage = usage.expect("usage present");
                assert_eq!(usage.input_tokens, Some(45_700));
                assert_eq!(usage.cache_read_tokens, Some(34_048));
                // output (28) + reasoning (16) collapsed — both consume context.
                assert_eq!(usage.output_tokens, Some(44));
                assert_eq!(usage.cache_creation_tokens, None);
                assert!(
                    usage.used_percent.is_none(),
                    "stdout usage does not carry context percentage"
                );
            }
            other => panic!("expected TurnCompleted with usage, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_handles_missing_usage_field() {
        // Defensive: older codex builds or error-path turns may emit
        // `turn.completed` with no `usage`. Stream-end detection still
        // needs to fire, but usage is None.
        let line = r#"{"type":"turn.completed"}"#;
        let parsed = parse_line(line).expect("turn.completed parses");
        match parsed {
            ParsedMessage::TurnCompleted { usage } => assert!(usage.is_none()),
            other => panic!("expected TurnCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_treats_empty_usage_object_as_no_data() {
        let line = r#"{"type":"turn.completed","usage":{}}"#;
        let parsed = parse_line(line).expect("turn.completed parses");
        match parsed {
            ParsedMessage::TurnCompleted { usage } => assert!(usage.is_none()),
            other => panic!("expected TurnCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_handles_zero_input_with_cache() {
        // Late-session shape: prompt fully cached, bare input_tokens drops to
        // 0 while cached_input_tokens carries the load. The accumulator still
        // gets a meaningful delta via `normalize_to_delta` (cache_read
        // baseline subtraction).
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":0,"cached_input_tokens":159500,"output_tokens":80,"reasoning_output_tokens":0}}"#;
        let parsed = parse_line(line).expect("turn.completed parses");
        match parsed {
            ParsedMessage::TurnCompleted { usage } => {
                let usage = usage.expect("usage present");
                assert_eq!(usage.input_tokens, Some(0));
                assert_eq!(usage.cache_read_tokens, Some(159_500));
                assert_eq!(usage.output_tokens, Some(80));
            }
            other => panic!("expected TurnCompleted with usage, got {other:?}"),
        }
    }

    #[test]
    fn parse_non_turn_completed_returns_none_or_other() {
        // Non-turn.completed events don't reach the usage path. `parse_line`
        // either returns a different variant or None — usage extraction
        // simply doesn't run.
        let line = r#"{"type":"response_item","payload":{"type":"message"}}"#;
        assert!(parse_line(line).is_none());
    }
}
