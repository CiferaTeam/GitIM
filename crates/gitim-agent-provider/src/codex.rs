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
    Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session,
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

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "codex finished");

    stderr_handle.abort();

    let _ = result_tx.send(ExecResult {
        status: final_status,
        output,
        error: final_error,
        duration_ms: duration.as_millis() as u64,
        session_token: thread_id,
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
