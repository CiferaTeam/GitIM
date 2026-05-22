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
    ProviderUsage, ProviderUsageReport, Session, preconditions,
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

        let stdout = preconditions::take_tokio_piped_stdout(&mut child);
        let stderr = preconditions::take_tokio_piped_stderr(&mut child);

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
    let mut billing_usage: Option<ProviderUsage> = None;
    let mut context_usage: Option<ProviderUsage> = None;

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
            let mut tail = preconditions::mutex_lock_arc(&stderr_tail_clone);
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
                                ParsedMessage::StepFinish { usage, reason } => {
                                    accumulate_opencode_usage(&mut billing_usage, &usage);
                                    context_usage = Some(usage);
                                    if reason.as_deref() == Some("stop") {
                                        break;
                                    }
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

    if final_status != ExecStatus::Timeout && final_status != ExecStatus::Aborted {
        // Graceful wait with a short window; if the child does not exit after
        // receiving the stop signal, force-kill it so we don't leak processes.
        match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
            Ok(Ok(status)) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("opencode exited with status: {status}"));
            }
            Ok(Err(e)) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for opencode: {e}"));
            }
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "opencode finished");

    stderr_handle.abort();

    // If failed with no error message, fall back to stderr tail
    if final_status == ExecStatus::Failed && final_error.as_ref().is_none_or(|e| e.is_empty()) {
        let tail = preconditions::mutex_lock_arc(&stderr_tail);
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
        usage_report: ProviderUsageReport::new(billing_usage.clone(), context_usage),
        usage: billing_usage,
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

fn accumulate_opencode_usage(accumulated: &mut Option<ProviderUsage>, next: &ProviderUsage) {
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
    StepFinish {
        usage: ProviderUsage,
        reason: Option<String>,
    },
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
            let part = raw.part?;
            let tokens = part.tokens?;
            let reason = part.reason;
            Some(ParsedMessage::StepFinish {
                usage: ProviderUsage {
                    input_tokens: Some(tokens.input),
                    output_tokens: Some(tokens.output.saturating_add(tokens.reasoning)),
                    used_percent: None,
                    cache_read_tokens: Some(tokens.cache.read),
                    cache_creation_tokens: Some(tokens.cache.write),
                    context_tokens: None,
                    context_window_tokens: None,
                },
                reason,
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
    #[serde(default)]
    reason: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::sync::Mutex;
    use tokio::time::{timeout, Duration};

    // Serialise fake-opencode tests so they do not compete for processes/stdout
    // and flake under parallel execution.
    static FAKE_OPENCODE_LOCK: Mutex<()> = Mutex::const_new(());

    /// Create a fake opencode binary that emits the given NDJSON lines then hangs.
    fn fake_opencode_with_script(tmp: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = tmp.join("fake-opencode");
        let script = format!(
            r#"#!/bin/bash
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --format|--model|--agent|--session) shift 2 ;; 
        --dangerously-skip-permissions) shift ;;
        --) shift; break ;;
        *) shift ;;
    esac
done
{}
while true; do sleep 1; done
"#,
            body
        );
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    async fn run_fake(
        body: &str,
        provider_timeout: Duration,
        test_timeout: Duration,
    ) -> (ExecResult, tokio::task::JoinHandle<()>) {
        let tmp = std::env::temp_dir().join(format!(
            "gitim-opencode-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let fake = fake_opencode_with_script(&tmp, body);

        let provider = OpencodeProvider::new(ProviderConfig {
            executable_path: Some(fake.to_string_lossy().to_string()),
            ..Default::default()
        });

        let session = provider
            .execute(
                "test prompt",
                ExecOptions {
                    timeout: Some(provider_timeout),
                    ..Default::default()
                },
            )
            .await
            .expect("execute should start");

        let mut events = session.events;
        let drain = tokio::spawn(async move { while events.recv().await.is_some() {} });

        let result = timeout(test_timeout, session.result)
            .await
            .expect("result should arrive before test timeout")
            .expect("result channel should not close");

        (result, drain)
    }

    #[tokio::test]
    async fn provider_times_out_when_opencode_never_exits() {
        let _guard = FAKE_OPENCODE_LOCK.lock().await;
        let (result, drain) = run_fake(
            r#"echo '{"type":"step_start","sessionID":"test-session"}'
echo '{"type":"text","part":{"text":"hello"}}'"#,
            Duration::from_secs(2),
            Duration::from_secs(10),
        )
        .await;

        assert_eq!(
            result.status,
            ExecStatus::Timeout,
            "expected Timeout when no step_finish is emitted; got {:?}",
            result
        );

        let _ = drain.await;
    }

    #[tokio::test]
    async fn provider_should_complete_after_full_ndjson_without_waiting_for_process_exit() {
        let _guard = FAKE_OPENCODE_LOCK.lock().await;
        let (result, drain) = run_fake(
            r#"echo '{"type":"step_start","sessionID":"test-session"}'
echo '{"type":"text","part":{"text":"hello from fake opencode"}}'
echo '{"type":"step_finish","part":{"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"reason":"stop"}}'"#,
            Duration::from_secs(30),
            Duration::from_secs(5),
        ).await;

        assert_eq!(
            result.status,
            ExecStatus::Completed,
            "expected Completed once step_finish(reason=stop) received; got {:?}",
            result
        );

        let _ = drain.await;
    }

    #[tokio::test]
    async fn provider_should_not_complete_on_tool_calls_without_final_stop() {
        let _guard = FAKE_OPENCODE_LOCK.lock().await;
        let (result, drain) = run_fake(
            r#"echo '{"type":"step_start","sessionID":"test-session"}'
echo '{"type":"tool_use","part":{"tool":"read","callID":"c1","state":{"status":"completed","input":{"filePath":"/tmp/test"}}}}'
echo '{"type":"step_finish","part":{"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"reason":"tool-calls"}}'"#,
            Duration::from_secs(2),
            Duration::from_secs(10),
        ).await;

        assert_eq!(
            result.status,
            ExecStatus::Timeout,
            "expected Timeout when only tool-calls stop is emitted; got {:?}",
            result
        );

        let _ = drain.await;
    }

    #[tokio::test]
    async fn provider_should_wait_for_stop_after_tool_calls() {
        let _guard = FAKE_OPENCODE_LOCK.lock().await;
        let (result, drain) = run_fake(
            r#"echo '{"type":"step_start","sessionID":"test-session"}'
echo '{"type":"tool_use","part":{"tool":"read","callID":"c1","state":{"status":"completed","input":{"filePath":"/tmp/test"}}}}'
echo '{"type":"step_finish","part":{"tokens":{"input":10,"output":5,"reasoning":2,"cache":{"read":100,"write":1}},"reason":"tool-calls"}}'
sleep 0.5
echo '{"type":"step_start","sessionID":"test-session"}'
echo '{"type":"text","part":{"text":"done"}}'
echo '{"type":"step_finish","part":{"tokens":{"input":5,"output":3,"reasoning":1,"cache":{"read":50,"write":2}},"reason":"stop"}}'"#,
            Duration::from_secs(30),
            Duration::from_secs(5),
        ).await;

        assert_eq!(
            result.status,
            ExecStatus::Completed,
            "expected Completed after final stop following tool-calls; got {:?}",
            result
        );

        let expected_billing = ProviderUsage {
            input_tokens: Some(15),
            output_tokens: Some(11),
            used_percent: None,
            cache_read_tokens: Some(150),
            cache_creation_tokens: Some(3),
            context_tokens: None,
            context_window_tokens: None,
        };
        let expected_context = ProviderUsage {
            input_tokens: Some(5),
            output_tokens: Some(4),
            used_percent: None,
            cache_read_tokens: Some(50),
            cache_creation_tokens: Some(2),
            context_tokens: None,
            context_window_tokens: None,
        };
        assert_eq!(result.usage, Some(expected_billing.clone()));
        assert_eq!(result.usage_report.billing, Some(expected_billing));
        assert_eq!(result.usage_report.context, Some(expected_context));

        let _ = drain.await;
    }
}
