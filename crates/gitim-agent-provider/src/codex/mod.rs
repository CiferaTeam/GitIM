use std::path::{Path, PathBuf};
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
    Event, ExecOptions, ExecResult, ExecStatus, PromptContext, Provider, ProviderConfig,
    ProviderError, ProviderUsage, Session,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

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
    fn prompt_memory(&self, _ctx: &PromptContext) -> String {
        crate::prompts::default_memory(_ctx).replace("CLAUDE.md", "AGENTS.md")
    }

    fn prompt_cold_start(&self, _ctx: &PromptContext) -> String {
        crate::prompts::default_cold_start(_ctx).replace("CLAUDE.md", "AGENTS.md")
    }

    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "codex".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let prompt = build_prompt(prompt, opts.system_prompt.as_deref());

        let mut args = vec!["exec".to_string()];
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["resume".to_string(), resume_token.clone()]);
        }
        args.push("--json".to_string());
        // bypass sandbox — agents need to run gitim commands without approval prompts
        args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        if let Some(model) = &opts.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        args.push(prompt);

        let mut cmd = Command::new(&exec_path);
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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "codex started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let join_handle = tokio::spawn(async move {
            drive_session(child, stdout, stderr, event_tx, result_tx, timeout, pid).await;
        });

        Ok(Session::new(
            event_rx,
            result_rx,
            join_handle.abort_handle(),
            CancellationToken::new(),
        ))
    }
}

async fn drive_session(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    timeout: Duration,
    pid: u32,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut thread_id: Option<String> = None;
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut saw_turn_completed = false;
    let mut latest_used_percent: Option<f64> = None;

    let mut reader = BufReader::new(stdout).lines();

    let stderr_handle = tokio::spawn(async move {
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "codex:stderr", "{}", line);
        }
    });

    let read_result = tokio::time::timeout(timeout, async {
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            // Codex streams token_count events mid-session; capture the
            // latest used_percent seen. Runs alongside parse_line since
            // token_count is an event_msg type parse_line doesn't handle.
            if let Some(pct) = parse_used_percent(&line) {
                latest_used_percent = Some(pct);
            }

            let parsed = match parse_line(&line) {
                Some(parsed) => parsed,
                None => continue,
            };

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
                ParsedMessage::TurnCompleted => {
                    saw_turn_completed = true;
                }
            }
        }
    })
    .await;

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("codex timed out after {timeout:?}"));
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

    // If codex failed and we have a thread_id, try to enrich the error by
    // scanning the session rollout file — codex writes richer diagnostics
    // there (subagent_notification errors, credit-exhaustion token_count)
    // than it streams to stdout.
    if final_status == ExecStatus::Failed {
        if let Some(tid) = thread_id.as_deref() {
            if let Some(reason) = diagnose_rollout_failure(tid) {
                final_error = Some(match final_error {
                    Some(prev) => format!("{prev} — {reason}"),
                    None => reason,
                });
            }
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "codex finished");

    stderr_handle.abort();

    let _ = result_tx.send(ExecResult {
        status: final_status,
        output,
        error: final_error,
        duration_ms: duration.as_millis() as u64,
        session_token: thread_id,
        usage: latest_used_percent.map(|p| ProviderUsage {
            input_tokens: None,
            output_tokens: None,
            used_percent: Some(p),
            ..Default::default()
        }),
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

#[derive(Debug)]
enum ParsedMessage {
    ThreadStarted { id: String },
    Text { content: String },
    ToolUse { call_id: String, command: String },
    ToolResult { call_id: String, output: String },
    TurnCompleted,
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
        "turn.completed" => Some(ParsedMessage::TurnCompleted),
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
}

/// Look up the codex session rollout file for `thread_id` and extract a
/// human-readable failure reason if one of the known patterns appears.
/// Returns None when nothing matches — callers fall back to the generic
/// "exit status" message.
fn diagnose_rollout_failure(thread_id: &str) -> Option<String> {
    let codex_home = codex_home()?;
    let rollout = find_rollout_file(&codex_home.join("sessions"), thread_id)?;
    let content = std::fs::read_to_string(&rollout).ok()?;
    scan_rollout_content(&content)
}

fn codex_home() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CODEX_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".codex"))
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
    for line in lines[start..].iter().rev() {
        if let Some(msg) = parse_rollout_line(line) {
            return Some(msg);
        }
        if credits_exhausted.is_none() {
            credits_exhausted = parse_credits_exhausted(line);
        }
    }
    credits_exhausted
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

/// Extract `rate_limits.primary.used_percent` from an `event_msg` of type `token_count`.
/// Returns `None` for other event types or malformed lines.
fn parse_used_percent(line: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload_type = v.pointer("/payload/type")?.as_str()?;
    if payload_type != "token_count" {
        return None;
    }
    v.pointer("/payload/rate_limits/primary/used_percent")?.as_f64()
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
    fn scan_rollout_content_returns_none_for_clean_run() {
        let content = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}
{"type":"event_msg","payload":{"type":"turn.completed"}}"#;
        assert!(scan_rollout_content(content).is_none());
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
    #[test]
    #[ignore]
    fn scan_rollout_content_on_real_fixture() {
        let Ok(path) = std::env::var("CODEX_ROLLOUT_FIXTURE") else {
            return;
        };
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        let msg = scan_rollout_content(&content)
            .expect("fixture should contain a known failure signal");
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
    fn parse_token_count_used_percent() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"limit_id":"codex","primary":{"used_percent":47.5},"credits":null,"plan_type":"plus"}}}"#;
        assert_eq!(parse_used_percent(line), Some(47.5));
    }

    #[test]
    fn parse_token_count_without_primary_returns_none() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{},"rate_limits":{"credits":{"has_credits":true}}}}"#;
        assert_eq!(parse_used_percent(line), None);
    }

    #[test]
    fn parse_non_token_count_returns_none() {
        let line = r#"{"type":"event_msg","payload":{"type":"agent_message"}}"#;
        assert_eq!(parse_used_percent(line), None);
    }
}
