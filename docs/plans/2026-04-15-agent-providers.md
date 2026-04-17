# Agent Provider Expansion Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port gemini, openclaw, opencode, hermes provider implementations from multica reference, replacing stubs with working code.

**Architecture:** Each provider implements the `Provider` trait with `execute()` returning a `Session`. NDJSON providers (gemini, openclaw, opencode) follow the same pattern as existing claude.rs: spawn process, read stdout lines, parse into `Event`s, return `ExecResult`. Hermes uses JSON-RPC 2.0 handshake over stdin then reads notifications from stdout.

**Tech Stack:** Rust, tokio, serde_json, async-trait, tokio-util

**Reference:** `/Users/lewisliu/ateam/souls-nexus/.repos/multica/server/pkg/agent/` (Go implementations)

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/util.rs` | Create | Shared `which()` helper |
| `src/gemini.rs` | Create | Gemini CLI provider (NDJSON) |
| `src/openclaw.rs` | Create | OpenClaw CLI provider (NDJSON) |
| `src/opencode.rs` | Create | OpenCode CLI provider (NDJSON) |
| `src/hermes.rs` | Create | Hermes ACP provider (JSON-RPC 2.0) |
| `src/lib.rs` | Modify | Add new module exports |
| `src/provider.rs` | Modify | Register new providers in factory |
| `src/stubs.rs` | Modify | Remove OpencodeProvider (now real) |
| `src/claude.rs` | Modify | Use shared `which()` |
| `src/codex.rs` | Modify | Use shared `which()` |
| `tests/gemini_parse_test.rs` | Create | Gemini line parser tests |
| `tests/openclaw_parse_test.rs` | Create | OpenClaw line parser tests |
| `tests/opencode_parse_test.rs` | Create | OpenCode line parser tests |
| `tests/hermes_parse_test.rs` | Create | Hermes notification parser tests |
| `tests/factory_test.rs` | Modify | Add new provider factory tests |

---

### Task 1: Extract shared `which()` to util.rs

**Files:**
- Create: `src/util.rs`
- Modify: `src/lib.rs`
- Modify: `src/claude.rs`
- Modify: `src/codex.rs`

- [ ] **Step 1: Create `src/util.rs`**

```rust
use std::path::{Path, PathBuf};

/// Find an executable by name, checking absolute paths and PATH.
pub fn which(name: &str) -> Result<PathBuf, ()> {
    let path = Path::new(name);
    if path.is_absolute() && path.exists() {
        return Ok(path.to_path_buf());
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let full = Path::new(dir).join(name);
            if full.exists() {
                return Ok(full);
            }
        }
    }
    Err(())
}
```

- [ ] **Step 2: Add `mod util` to `src/lib.rs`**

Add `pub(crate) mod util;` to lib.rs (between `mod error` and `mod types`).

- [ ] **Step 3: Update `src/claude.rs` to use shared `which()`**

Remove the local `fn which()` definition (lines 317-331). Replace the call at line 38:
```rust
crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
```

- [ ] **Step 4: Update `src/codex.rs` to use shared `which()`**

Remove the local `fn which()` definition (lines 267-281). Replace the call at line 40:
```rust
crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All 16 tests pass (1 unit + 5 factory + 10 claude parse)

- [ ] **Step 6: Commit**

```bash
git add src/util.rs src/lib.rs src/claude.rs src/codex.rs
git commit -m "refactor: extract shared which() to util.rs"
```

---

### Task 2: Gemini provider (simplest NDJSON)

**Files:**
- Create: `tests/gemini_parse_test.rs`
- Create: `src/gemini.rs`

**Protocol:** `gemini -p "<prompt>" --yolo -o stream-json [-m model] [-r session]`

NDJSON events: `init`, `message`, `tool_use`, `tool_result`, `error`, `result`

- [ ] **Step 1: Write parse tests — `tests/gemini_parse_test.rs`**

```rust
use gitim_agent_provider::gemini::{parse_line, ParsedMessage};
use gitim_agent_provider::Event;
use serde_json::json;

#[test]
fn parse_init_extracts_session_id() {
    let line = json!({"type": "init", "session_id": "ses_123"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Init { session_id } if session_id == "ses_123"));
}

#[test]
fn parse_message_text() {
    let line = json!({
        "type": "message",
        "role": "assistant",
        "content": "Hello world"
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Text { content } if content == "Hello world"));
}

#[test]
fn parse_tool_use() {
    let line = json!({
        "type": "tool_use",
        "tool_name": "terminal",
        "tool_id": "tc-abc",
        "parameters": {"command": "ls"}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolUse { tool, call_id, .. }
        if tool == "terminal" && call_id == "tc-abc"));
}

#[test]
fn parse_tool_result() {
    let line = json!({
        "type": "tool_result",
        "tool_id": "tc-abc",
        "status": "completed",
        "output": "file1.rs"
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolResult { call_id, output }
        if call_id == "tc-abc" && output == "file1.rs"));
}

#[test]
fn parse_error_event() {
    let line = json!({
        "type": "error",
        "severity": "error",
        "message": "Model not found"
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Error { message } if message == "Model not found"));
}

#[test]
fn parse_result_completed() {
    let line = json!({
        "type": "result",
        "status": "completed",
        "stats": {"input_tokens": 100, "output_tokens": 50}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { status, .. } if status == "completed"));
}

#[test]
fn parse_result_failed() {
    let line = json!({"type": "result", "status": "error"}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { status, .. } if status == "error"));
}

#[test]
fn parse_empty_returns_none() {
    assert!(parse_line("").is_none());
}

#[test]
fn parse_unknown_type_returns_none() {
    let line = json!({"type": "debug", "data": "x"}).to_string();
    assert!(parse_line(&line).is_none());
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p gitim-agent-provider --test gemini_parse_test 2>&1`
Expected: Compilation error (module `gemini` doesn't exist)

- [ ] **Step 3: Implement `src/gemini.rs`**

```rust
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct GeminiProvider {
    config: ProviderConfig,
}

impl GeminiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "gemini".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--yolo".to_string(),
            "-o".to_string(),
            "stream-json".to_string(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["-m".to_string(), model.clone()]);
        }
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["-r".to_string(), resume_token.clone()]);
        }

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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "gemini started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(child, stdout, stderr, event_tx, result_tx, timeout, pid, cancel_token_inner).await;
        });

        Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
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
    cancel_token: CancellationToken,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut saw_result = false;

    let mut reader = BufReader::new(stdout).lines();

    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "gemini:stderr", "{}", line);
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
                            if line.is_empty() { continue; }

                            let parsed = match parse_line(&line) {
                                Some(p) => p,
                                None => {
                                    debug!(pid, line_len = line.len(), "unparsed line");
                                    continue;
                                }
                            };

                            match parsed {
                                ParsedMessage::Init { session_id: sid } => {
                                    session_id = sid;
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::Text { content } => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                ParsedMessage::ToolUse { tool, call_id, input } => {
                                    try_send_event(&event_tx, Event::ToolUse { tool, call_id, input });
                                }
                                ParsedMessage::ToolResult { call_id, output } => {
                                    try_send_event(&event_tx, Event::ToolResult { call_id, output });
                                }
                                ParsedMessage::Error { message } => {
                                    final_status = ExecStatus::Failed;
                                    final_error = Some(message.clone());
                                    try_send_event(&event_tx, Event::Error { content: message });
                                }
                                ParsedMessage::Result { status, .. } => {
                                    saw_result = true;
                                    if status == "error" || status == "failed" {
                                        final_status = ExecStatus::Failed;
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

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("gemini timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else if !saw_result && final_status == ExecStatus::Completed {
        final_status = ExecStatus::Failed;
        final_error = Some("gemini stream ended without a result message".to_string());
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("gemini exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for gemini: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "gemini finished");

    stderr_handle.abort();

    if final_status == ExecStatus::Failed
        && final_error.as_ref().map_or(true, |e| e.is_empty())
    {
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
        session_token: if session_id.is_empty() { None } else { Some(session_id) },
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

// ── Parsing ──

#[derive(Debug)]
pub enum ParsedMessage {
    Init { session_id: String },
    Text { content: String },
    ToolUse { tool: String, call_id: String, input: Value },
    ToolResult { call_id: String, output: String },
    Error { message: String },
    Result { status: String },
}

pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() { return None; }

    let raw: RawEvent = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "init" => Some(ParsedMessage::Init {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "message" => Some(ParsedMessage::Text {
            content: raw.content.unwrap_or_default(),
        }),
        "tool_use" => Some(ParsedMessage::ToolUse {
            tool: raw.tool_name.unwrap_or_default(),
            call_id: raw.tool_id.unwrap_or_default(),
            input: raw.parameters.unwrap_or(Value::Object(Default::default())),
        }),
        "tool_result" => Some(ParsedMessage::ToolResult {
            call_id: raw.tool_id.unwrap_or_default(),
            output: raw.output.unwrap_or_default(),
        }),
        "error" => Some(ParsedMessage::Error {
            message: raw.message.unwrap_or_default(),
        }),
        "result" => Some(ParsedMessage::Result {
            status: raw.status.unwrap_or_else(|| "completed".to_string()),
        }),
        _ => None,
    }
}

#[derive(Deserialize)]
struct RawEvent {
    r#type: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_id: Option<String>,
    #[serde(default)]
    parameters: Option<Value>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
}
```

- [ ] **Step 4: Add `pub mod gemini;` to `src/lib.rs`**

- [ ] **Step 5: Register in `src/provider.rs`**

Add to factory match:
```rust
"gemini" => Ok(Box::new(crate::gemini::GeminiProvider::new(config))),
```

- [ ] **Step 6: Add factory test to `tests/factory_test.rs`**

```rust
#[test]
fn create_gemini_returns_ok() {
    assert!(create("gemini", Default::default()).is_ok());
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass (previous + 9 gemini parse + 1 factory)

- [ ] **Step 8: Commit**

```bash
git add src/gemini.rs src/lib.rs src/provider.rs tests/gemini_parse_test.rs tests/factory_test.rs
git commit -m "feat: add gemini provider (NDJSON stream-json)"
```

---

### Task 3: OpenClaw provider (NDJSON with combined tool events)

**Files:**
- Create: `tests/openclaw_parse_test.rs`
- Create: `src/openclaw.rs`

**Protocol:** `openclaw agent --output-format stream-json --yes [--model M] [--system-prompt S] [--max-turns N] [--session ID] -p prompt`

Events use `data` wrapper, `sessionId` field, combined tool_call events.

- [ ] **Step 1: Write parse tests — `tests/openclaw_parse_test.rs`**

```rust
use gitim_agent_provider::openclaw::{parse_line, ParsedMessage};
use serde_json::json;

#[test]
fn parse_step_start() {
    let line = json!({"type": "step_start", "sessionId": "s-123", "data": {}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::StepStart { session_id } if session_id == "s-123"));
}

#[test]
fn parse_text() {
    let line = json!({"type": "text", "sessionId": "s-1", "data": {"text": "Hello"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Text { content } if content == "Hello"));
}

#[test]
fn parse_thinking() {
    let line = json!({"type": "thinking", "sessionId": "s-1", "data": {"text": "Hmm..."}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Thinking { content } if content == "Hmm..."));
}

#[test]
fn parse_tool_call_pending() {
    let line = json!({
        "type": "tool_call", "sessionId": "s-1",
        "data": {
            "name": "Bash", "callId": "c-1",
            "input": {"command": "ls"}, "status": "pending"
        }
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolCall {
        ref name, ref call_id, ref status, ..
    } if name == "Bash" && call_id == "c-1" && status == "pending"));
}

#[test]
fn parse_tool_call_completed_with_output() {
    let line = json!({
        "type": "tool_call", "sessionId": "s-1",
        "data": {
            "name": "Bash", "callId": "c-1",
            "input": {"command": "ls"}, "status": "completed",
            "output": "file1.rs"
        }
    }).to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::ToolCall { status, output, .. } => {
            assert_eq!(status, "completed");
            assert_eq!(output.as_deref(), Some("file1.rs"));
        }
        _ => panic!("expected ToolCall"),
    }
}

#[test]
fn parse_result_completed() {
    let line = json!({
        "type": "result", "sessionId": "s-1",
        "data": {"status": "completed"}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { is_error } if !is_error));
}

#[test]
fn parse_result_error() {
    let line = json!({
        "type": "result", "sessionId": "s-1",
        "data": {"status": "error"}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Result { is_error } if is_error));
}

#[test]
fn parse_error_event() {
    let line = json!({
        "type": "error", "sessionId": "s-1",
        "data": {"message": "boom", "code": "E001"}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Error { ref message } if message == "boom"));
}

#[test]
fn parse_empty_returns_none() {
    assert!(parse_line("").is_none());
}

#[test]
fn parse_unknown_type_returns_none() {
    assert!(parse_line(&json!({"type": "step_end", "data": {}}).to_string()).is_none());
}
```

- [ ] **Step 2: Run tests — verify they fail**

Run: `cargo test -p gitim-agent-provider --test openclaw_parse_test 2>&1`
Expected: Compilation error

- [ ] **Step 3: Implement `src/openclaw.rs`**

```rust
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const EVENT_CHANNEL_BUFFER: usize = 256;

pub struct OpenclawProvider {
    config: ProviderConfig,
}

impl OpenclawProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for OpenclawProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let exec_path = self
            .config
            .executable_path
            .clone()
            .unwrap_or_else(|| "openclaw".to_string());

        crate::util::which(&exec_path).map_err(|_| ProviderError::ExecutableNotFound {
            path: exec_path.clone(),
        })?;

        let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);

        let mut args = vec![
            "agent".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--yes".to_string(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(system_prompt) = &opts.system_prompt {
            args.extend(["--system-prompt".to_string(), system_prompt.clone()]);
        }
        if let Some(max_turns) = opts.max_turns {
            args.extend(["--max-turns".to_string(), max_turns.to_string()]);
        }
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["--session".to_string(), resume_token.clone()]);
        }
        args.extend(["-p".to_string(), prompt.to_string()]);

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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "openclaw started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(child, stdout, stderr, event_tx, result_tx, timeout, pid, cancel_token_inner).await;
        });

        Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
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
    cancel_token: CancellationToken,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;
    let mut saw_result = false;

    let mut reader = BufReader::new(stdout).lines();

    let stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_tail_clone = stderr_tail.clone();
    let stderr_handle = tokio::spawn(async move {
        const TAIL_LINES: usize = 20;
        let mut r = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = r.next_line().await {
            debug!(target: "openclaw:stderr", "{}", line);
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
                            if line.is_empty() { continue; }

                            let parsed = match parse_line(&line) {
                                Some(p) => p,
                                None => {
                                    debug!(pid, line_len = line.len(), "unparsed line");
                                    continue;
                                }
                            };

                            match parsed {
                                ParsedMessage::StepStart { session_id: sid } => {
                                    if session_id.is_empty() {
                                        session_id = sid;
                                    }
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::Text { content } => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                ParsedMessage::Thinking { content } => {
                                    try_send_event(&event_tx, Event::Thinking { content });
                                }
                                ParsedMessage::ToolCall { name, call_id, input, status, output: tool_output } => {
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool: name,
                                        call_id: call_id.clone(),
                                        input,
                                    });
                                    if (status == "completed" || status == "failed") {
                                        if let Some(out) = tool_output {
                                            try_send_event(&event_tx, Event::ToolResult {
                                                call_id,
                                                output: out,
                                            });
                                        }
                                    }
                                }
                                ParsedMessage::Error { message } => {
                                    final_status = ExecStatus::Failed;
                                    final_error = Some(message.clone());
                                    try_send_event(&event_tx, Event::Error { content: message });
                                }
                                ParsedMessage::Result { is_error } => {
                                    saw_result = true;
                                    if is_error {
                                        final_status = ExecStatus::Failed;
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

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("openclaw timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
    } else if !saw_result && final_status == ExecStatus::Completed {
        final_status = ExecStatus::Failed;
        final_error = Some("openclaw stream ended without a result message".to_string());
    }

    if final_status != ExecStatus::Timeout {
        match child.wait().await {
            Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("openclaw exited with status: {status}"));
            }
            Err(e) if final_status == ExecStatus::Completed => {
                final_status = ExecStatus::Failed;
                final_error = Some(format!("failed to wait for openclaw: {e}"));
            }
            _ => {}
        }
    }

    let duration = start.elapsed();
    info!(pid, ?final_status, ?duration, "openclaw finished");

    stderr_handle.abort();

    if final_status == ExecStatus::Failed
        && final_error.as_ref().map_or(true, |e| e.is_empty())
    {
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
        session_token: if session_id.is_empty() { None } else { Some(session_id) },
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

// ── Parsing ──

#[derive(Debug)]
pub enum ParsedMessage {
    StepStart { session_id: String },
    Text { content: String },
    Thinking { content: String },
    ToolCall { name: String, call_id: String, input: Value, status: String, output: Option<String> },
    Error { message: String },
    Result { is_error: bool },
}

pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() { return None; }

    let raw: RawEvent = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "step_start" => Some(ParsedMessage::StepStart {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "text" => {
            let data = raw.data?;
            Some(ParsedMessage::Text {
                content: data.text.unwrap_or_default(),
            })
        }
        "thinking" => {
            let data = raw.data?;
            Some(ParsedMessage::Thinking {
                content: data.text.unwrap_or_default(),
            })
        }
        "tool_call" => {
            let data = raw.data?;
            let output = data.output.map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            });
            Some(ParsedMessage::ToolCall {
                name: data.name.unwrap_or_default(),
                call_id: data.call_id.unwrap_or_default(),
                input: data.input.unwrap_or(Value::Object(Default::default())),
                status: data.status.unwrap_or_default(),
                output,
            })
        }
        "error" => {
            let data = raw.data?;
            Some(ParsedMessage::Error {
                message: data.message.unwrap_or_default(),
            })
        }
        "result" => {
            let data = raw.data?;
            let status = data.status.unwrap_or_default();
            Some(ParsedMessage::Result {
                is_error: status == "error" || status == "failed",
            })
        }
        _ => None,
    }
}

#[derive(Deserialize)]
struct RawEvent {
    r#type: String,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    data: Option<RawData>,
}

#[derive(Deserialize)]
struct RawData {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "callId")]
    call_id: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    message: Option<String>,
}
```

- [ ] **Step 4: Add `pub mod openclaw;` to `src/lib.rs`**

- [ ] **Step 5: Register in `src/provider.rs`**

Add to factory match:
```rust
"openclaw" => Ok(Box::new(crate::openclaw::OpenclawProvider::new(config))),
```

- [ ] **Step 6: Add factory test**

```rust
#[test]
fn create_openclaw_returns_ok() {
    assert!(create("openclaw", Default::default()).is_ok());
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
git add src/openclaw.rs src/lib.rs src/provider.rs tests/openclaw_parse_test.rs tests/factory_test.rs
git commit -m "feat: add openclaw provider (NDJSON stream-json)"
```

---

### Task 4: OpenCode provider (replace stub)

**Files:**
- Create: `tests/opencode_parse_test.rs`
- Create: `src/opencode.rs`
- Modify: `src/stubs.rs` (remove OpencodeProvider)

**Protocol:** `opencode run --format json [--model M] [--prompt sys] [--session ID] prompt`
Env: `OPENCODE_PERMISSION={"*":"allow"}`

Events use `part` wrapper, `sessionID` field, combined tool_use events.

- [ ] **Step 1: Write parse tests — `tests/opencode_parse_test.rs`**

```rust
use gitim_agent_provider::opencode::{parse_line, ParsedMessage};
use serde_json::json;

#[test]
fn parse_step_start() {
    let line = json!({"type": "step_start", "sessionID": "s-1", "part": {}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::StepStart { session_id } if session_id == "s-1"));
}

#[test]
fn parse_text() {
    let line = json!({"type": "text", "sessionID": "s-1", "part": {"text": "Hi"}}).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Text { content } if content == "Hi"));
}

#[test]
fn parse_tool_use_pending() {
    let line = json!({
        "type": "tool_use", "sessionID": "s-1",
        "part": {
            "tool": "Bash", "callID": "c-1",
            "state": {"status": "pending", "input": {"command": "ls"}}
        }
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::ToolUse {
        ref tool, ref call_id, ref status, ..
    } if tool == "Bash" && call_id == "c-1" && status == "pending"));
}

#[test]
fn parse_tool_use_completed() {
    let line = json!({
        "type": "tool_use", "sessionID": "s-1",
        "part": {
            "tool": "Bash", "callID": "c-1",
            "state": {"status": "completed", "input": {"command": "ls"}, "output": "file.rs"}
        }
    }).to_string();
    let msg = parse_line(&line).unwrap();
    match msg {
        ParsedMessage::ToolUse { status, output, .. } => {
            assert_eq!(status, "completed");
            assert_eq!(output.as_deref(), Some("file.rs"));
        }
        _ => panic!("expected ToolUse"),
    }
}

#[test]
fn parse_error() {
    let line = json!({
        "type": "error", "sessionID": "s-1",
        "error": {"name": "InvalidModel", "data": {"message": "not found"}}
    }).to_string();
    let msg = parse_line(&line).unwrap();
    assert!(matches!(msg, ParsedMessage::Error { ref message } if message == "not found"));
}

#[test]
fn parse_empty_returns_none() {
    assert!(parse_line("").is_none());
}

#[test]
fn parse_step_finish_returns_none() {
    let line = json!({"type": "step_finish", "sessionID": "s-1", "part": {}}).to_string();
    assert!(parse_line(&line).is_none());
}
```

- [ ] **Step 2: Run tests — verify they fail**

- [ ] **Step 3: Implement `src/opencode.rs`**

```rust
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

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

        let mut args = vec![
            "run".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(system_prompt) = &opts.system_prompt {
            args.extend(["--prompt".to_string(), system_prompt.clone()]);
        }
        if let Some(resume_token) = &opts.resume_token {
            args.extend(["--session".to_string(), resume_token.clone()]);
        }
        args.push(prompt.to_string());

        let mut cmd = Command::new(&exec_path);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .env("OPENCODE_PERMISSION", r#"{"*":"allow"}"#);

        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, "opencode started");

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();

        let cancel_token = CancellationToken::new();
        let cancel_token_inner = cancel_token.clone();

        let join_handle = tokio::spawn(async move {
            drive_session(child, stdout, stderr, event_tx, result_tx, timeout, pid, cancel_token_inner).await;
        });

        Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
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
    cancel_token: CancellationToken,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;

    let mut reader = BufReader::new(stdout).lines();

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
                            if line.is_empty() { continue; }

                            let parsed = match parse_line(&line) {
                                Some(p) => p,
                                None => {
                                    debug!(pid, line_len = line.len(), "unparsed line");
                                    continue;
                                }
                            };

                            match parsed {
                                ParsedMessage::StepStart { session_id: sid } => {
                                    if session_id.is_empty() {
                                        session_id = sid;
                                    }
                                    try_send_event(&event_tx, Event::Status {
                                        status: "running".to_string(),
                                    });
                                }
                                ParsedMessage::Text { content } => {
                                    output.push_str(&content);
                                    try_send_event(&event_tx, Event::Text { content });
                                }
                                ParsedMessage::ToolUse { tool, call_id, input, status, output: tool_output } => {
                                    try_send_event(&event_tx, Event::ToolUse {
                                        tool,
                                        call_id: call_id.clone(),
                                        input,
                                    });
                                    if (status == "completed" || status == "failed") {
                                        if let Some(out) = tool_output {
                                            try_send_event(&event_tx, Event::ToolResult {
                                                call_id,
                                                output: out,
                                            });
                                        }
                                    }
                                }
                                ParsedMessage::Error { message } => {
                                    final_status = ExecStatus::Failed;
                                    final_error = Some(message.clone());
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

    if final_status == ExecStatus::Failed
        && final_error.as_ref().map_or(true, |e| e.is_empty())
    {
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
        session_token: if session_id.is_empty() { None } else { Some(session_id) },
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

// ── Parsing ──

#[derive(Debug)]
pub enum ParsedMessage {
    StepStart { session_id: String },
    Text { content: String },
    ToolUse { tool: String, call_id: String, input: Value, status: String, output: Option<String> },
    Error { message: String },
}

pub fn parse_line(line: &str) -> Option<ParsedMessage> {
    let line = line.trim();
    if line.is_empty() { return None; }

    let raw: RawEvent = serde_json::from_str(line).ok()?;

    match raw.r#type.as_str() {
        "step_start" => Some(ParsedMessage::StepStart {
            session_id: raw.session_id.unwrap_or_default(),
        }),
        "text" => {
            let part = raw.part?;
            Some(ParsedMessage::Text {
                content: part.text.unwrap_or_default(),
            })
        }
        "tool_use" => {
            let part = raw.part?;
            let state = part.state?;
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
            let message = err.data
                .and_then(|d| d.get("message").and_then(|v| v.as_str().map(String::from)))
                .or(err.name)
                .unwrap_or_default();
            Some(ParsedMessage::Error { message })
        }
        _ => None,
    }
}

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

#[derive(Deserialize)]
struct RawPart {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default, rename = "callID")]
    call_id: Option<String>,
    #[serde(default)]
    state: Option<RawToolState>,
}

#[derive(Deserialize)]
struct RawToolState {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    output: Option<Value>,
}

#[derive(Deserialize)]
struct RawError {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}
```

- [ ] **Step 4: Remove `OpencodeProvider` from `src/stubs.rs`**

Only keep `CursorProvider` in stubs.rs.

- [ ] **Step 5: Add `pub mod opencode;` to `src/lib.rs`**

- [ ] **Step 6: Update `src/provider.rs`**

Change:
```rust
"opencode" => Ok(Box::new(crate::stubs::OpencodeProvider::new(config))),
```
To:
```rust
"opencode" => Ok(Box::new(crate::opencode::OpencodeProvider::new(config))),
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
git add src/opencode.rs src/stubs.rs src/lib.rs src/provider.rs tests/opencode_parse_test.rs
git commit -m "feat: add opencode provider, replace stub with real implementation"
```

---

### Task 5: Hermes provider (JSON-RPC 2.0 handshake + notifications)

**Files:**
- Create: `tests/hermes_parse_test.rs`
- Create: `src/hermes.rs`

**Protocol:** `hermes acp` with `HERMES_YOLO_MODE=1`

JSON-RPC 2.0: 3-step handshake (initialize, session/new, session/prompt), then `session/update` notifications.

- [ ] **Step 1: Write parse tests — `tests/hermes_parse_test.rs`**

```rust
use gitim_agent_provider::hermes::{parse_notification, ParsedNotification};
use serde_json::json;

#[test]
fn parse_text_chunk() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "Hello"}
        }
    });
    let msg = parse_notification(&params).unwrap();
    assert!(matches!(msg, ParsedNotification::Text { content } if content == "Hello"));
}

#[test]
fn parse_thinking_chunk() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "Let me think..."}
        }
    });
    let msg = parse_notification(&params).unwrap();
    assert!(matches!(msg, ParsedNotification::Thinking { content } if content == "Let me think..."));
}

#[test]
fn parse_tool_call() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-1",
            "title": "terminal: ls -la",
            "kind": "execute",
            "status": "pending",
            "rawInput": {"command": "ls -la"}
        }
    });
    let msg = parse_notification(&params).unwrap();
    match msg {
        ParsedNotification::ToolCall { tool, call_id, .. } => {
            assert_eq!(tool, "terminal");
            assert_eq!(call_id, "tc-1");
        }
        _ => panic!("expected ToolCall"),
    }
}

#[test]
fn parse_tool_call_update_completed() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-1",
            "status": "completed",
            "rawOutput": "file1.rs\nfile2.rs"
        }
    });
    let msg = parse_notification(&params).unwrap();
    match msg {
        ParsedNotification::ToolResult { call_id, output } => {
            assert_eq!(call_id, "tc-1");
            assert_eq!(output, "file1.rs\nfile2.rs");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn parse_tool_call_update_pending_returns_none() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-1",
            "status": "pending"
        }
    });
    assert!(parse_notification(&params).is_none());
}

#[test]
fn parse_usage_update_returns_none() {
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "usage_update",
            "usage": {"inputTokens": 100}
        }
    });
    // We don't track usage yet — should return None
    assert!(parse_notification(&params).is_none());
}

#[test]
fn parse_unknown_update_returns_none() {
    let params = json!({
        "sessionId": "s-1",
        "update": {"sessionUpdate": "something_new"}
    });
    assert!(parse_notification(&params).is_none());
}

#[test]
fn parse_tool_title_extracts_name() {
    // "terminal: ls -la" -> tool = "terminal"
    let params = json!({
        "sessionId": "s-1",
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-1",
            "title": "file_edit: path/to/file.rs",
            "kind": "edit",
            "status": "pending",
            "rawInput": {}
        }
    });
    let msg = parse_notification(&params).unwrap();
    assert!(matches!(msg, ParsedNotification::ToolCall { ref tool, .. } if tool == "file_edit"));
}
```

- [ ] **Step 2: Run tests — verify they fail**

- [ ] **Step 3: Implement `src/hermes.rs`**

```rust
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

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

        let mut cmd = Command::new(&exec_path);
        cmd.arg("acp")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
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

        let cwd = opts.cwd.clone().unwrap_or_else(|| ".".into());
        let resume_token = opts.resume_token.clone();
        let prompt = prompt.to_string();

        let join_handle = tokio::spawn(async move {
            drive_session(
                child, stdout, stdin, stderr, event_tx, result_tx,
                timeout, pid, cancel_token_inner,
                cwd.to_string_lossy().to_string(), prompt, resume_token,
            ).await;
        });

        Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_session(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    mut stdin: tokio::process::ChildStdin,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    timeout: Duration,
    pid: u32,
    cancel_token: CancellationToken,
    cwd: String,
    prompt: String,
    resume_token: Option<String>,
) {
    let start = Instant::now();
    let mut output = String::new();
    let mut session_id = String::new();
    let mut final_status = ExecStatus::Completed;
    let mut final_error: Option<String> = None;

    let mut reader = BufReader::new(stdout).lines();

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

    // ── JSON-RPC Handshake ──

    // Helper: send a JSON-RPC request and read the response
    async fn rpc_call(
        stdin: &mut tokio::process::ChildStdin,
        reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut buf = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        buf.push(b'\n');
        stdin.write_all(&buf).await.map_err(|e| format!("stdin write: {e}"))?;

        // Read lines until we get the response with matching id
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.is_empty() { continue; }
                    if let Ok(resp) = serde_json::from_str::<RpcResponse>(&line) {
                        if resp.id == Some(id) {
                            if let Some(err) = resp.error {
                                return Err(format!("{method}: {} (code={})",
                                    err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown"),
                                    err.get("code").and_then(|v| v.as_i64()).unwrap_or(0),
                                ));
                            }
                            return Ok(resp.result.unwrap_or(Value::Null));
                        }
                    }
                    // Not our response — skip (could be a notification during handshake)
                }
                Ok(None) => return Err(format!("{method}: stream ended")),
                Err(e) => return Err(format!("{method}: read error: {e}")),
            }
        }
    }

    // Step 1: initialize
    let init_result = rpc_call(&mut stdin, &mut reader, 0, "initialize", json!({
        "protocolVersion": 1,
        "clientInfo": {"name": "gitim-agent-sdk", "version": "0.1.0"},
        "clientCapabilities": {},
    })).await;

    if let Err(e) = init_result {
        final_status = ExecStatus::Failed;
        final_error = Some(format!("handshake failed: {e}"));
        let _ = child.start_kill();
        send_result(result_tx, final_status, output, final_error, start, &session_id);
        stderr_handle.abort();
        return;
    }

    // Step 2: session/new or session/resume
    let session_method = if resume_token.is_some() { "session/resume" } else { "session/new" };
    let session_params = if let Some(ref token) = resume_token {
        json!({"cwd": cwd, "sessionId": token})
    } else {
        json!({"cwd": cwd, "mcpServers": []})
    };

    let session_result = rpc_call(&mut stdin, &mut reader, 1, session_method, session_params).await;
    match session_result {
        Ok(result) => {
            session_id = result.get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            info!(pid, session_id = %session_id, "hermes session created");
        }
        Err(e) => {
            final_status = ExecStatus::Failed;
            final_error = Some(format!("session creation failed: {e}"));
            let _ = child.start_kill();
            send_result(result_tx, final_status, output, final_error, start, &session_id);
            stderr_handle.abort();
            return;
        }
    }

    try_send_event(&event_tx, Event::Status { status: "running".to_string() });

    // Step 3: session/prompt (non-blocking — response arrives after notifications)
    let prompt_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt}],
        },
    });
    if let Ok(mut buf) = serde_json::to_vec(&prompt_req) {
        buf.push(b'\n');
        if let Err(e) = stdin.write_all(&buf).await {
            warn!("failed to send prompt: {e}");
            final_status = ExecStatus::Failed;
            final_error = Some(format!("failed to send prompt: {e}"));
            let _ = child.start_kill();
            send_result(result_tx, final_status, output, final_error, start, &session_id);
            stderr_handle.abort();
            return;
        }
    }

    // ── Event Loop: read notifications + final prompt response ──

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
                                None => { debug!(pid, "unparsed line"); continue; }
                            };

                            // Check if this is the prompt response (id=2)
                            if raw.get("id").and_then(|v| v.as_u64()) == Some(2) {
                                // Final prompt response
                                if let Some(err) = raw.get("error") {
                                    final_status = ExecStatus::Failed;
                                    final_error = Some(
                                        err.get("message").and_then(|v| v.as_str())
                                            .unwrap_or("prompt failed").to_string()
                                    );
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
                                            ParsedNotification::ToolResult { call_id, output } => {
                                                try_send_event(&event_tx, Event::ToolResult { call_id, output });
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

    // Close stdin to signal shutdown
    drop(stdin);

    if read_result.is_err() {
        final_status = ExecStatus::Timeout;
        final_error = Some(format!("hermes timed out after {timeout:?}"));
        let _ = child.start_kill();
    } else if final_status == ExecStatus::Aborted {
        let _ = child.start_kill();
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

    if final_status == ExecStatus::Failed
        && final_error.as_ref().map_or(true, |e| e.is_empty())
    {
        let tail = stderr_tail.lock().unwrap();
        if !tail.is_empty() {
            final_error = Some(format!("(stderr) {}", tail.join("\n")));
        }
    }

    send_result(result_tx, final_status, output, final_error, start, &session_id);
}

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
        session_token: if session_id.is_empty() { None } else { Some(session_id.to_string()) },
    });
}

fn try_send_event(tx: &mpsc::Sender<Event>, event: Event) {
    if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) = tx.try_send(event) {
        warn!("event channel full, dropping event");
    }
}

// ── Notification Parsing (public for tests) ──

#[derive(Debug)]
pub enum ParsedNotification {
    Text { content: String },
    Thinking { content: String },
    ToolCall { tool: String, call_id: String, input: Value },
    ToolResult { call_id: String, output: String },
}

/// Parse a `session/update` notification's params object.
pub fn parse_notification(params: &Value) -> Option<ParsedNotification> {
    let update = params.get("update")?;
    let update_type = update.get("sessionUpdate")?.as_str()?;

    match update_type {
        "agent_message_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?;
            if text.is_empty() { return None; }
            Some(ParsedNotification::Text { content: text.to_string() })
        }
        "agent_thought_chunk" => {
            let text = update.get("content")?.get("text")?.as_str()?;
            if text.is_empty() { return None; }
            Some(ParsedNotification::Thinking { content: text.to_string() })
        }
        "tool_call" => {
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let title = update.get("title").and_then(|v| v.as_str()).unwrap_or("");
            // Extract tool name from title "terminal: ls -la" -> "terminal"
            let tool = title.split(':').next().unwrap_or("unknown").trim().to_string();
            let input = update.get("rawInput").cloned().unwrap_or(Value::Object(Default::default()));
            Some(ParsedNotification::ToolCall { tool, call_id, input })
        }
        "tool_call_update" => {
            let status = update.get("status")?.as_str()?;
            if status != "completed" && status != "failed" { return None; }
            let call_id = update.get("toolCallId")?.as_str()?.to_string();
            let output = update.get("rawOutput")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            Some(ParsedNotification::ToolResult { call_id, output })
        }
        _ => None,
    }
}

// ── JSON-RPC types ──

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}
```

- [ ] **Step 4: Add `pub mod hermes;` to `src/lib.rs`**

- [ ] **Step 5: Register in `src/provider.rs`**

```rust
"hermes" => Ok(Box::new(crate::hermes::HermesProvider::new(config))),
```

- [ ] **Step 6: Add factory test**

```rust
#[test]
fn create_hermes_returns_ok() {
    assert!(create("hermes", Default::default()).is_ok());
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
git add src/hermes.rs src/lib.rs src/provider.rs tests/hermes_parse_test.rs tests/factory_test.rs
git commit -m "feat: add hermes provider (ACP JSON-RPC 2.0)"
```

---

### Task 6: Final wiring and cleanup

**Files:**
- Modify: `src/lib.rs`
- Modify: `src/provider.rs`
- Modify: `tests/factory_test.rs`

- [ ] **Step 1: Verify final `src/lib.rs`**

Should have:
```rust
pub mod claude;
pub mod codex;
pub mod gemini;
pub mod hermes;
pub mod mock;
pub mod openclaw;
pub mod opencode;
mod error;
mod stubs;
mod types;
pub(crate) mod util;

pub use error::ProviderError;
pub use provider::{create, Provider};
pub use types::{Event, ExecOptions, ExecResult, ExecStatus, ProviderConfig, Session};
```

- [ ] **Step 2: Verify final `src/provider.rs` factory**

```rust
pub fn create(
    provider_type: &str,
    config: ProviderConfig,
) -> Result<Box<dyn Provider>, ProviderError> {
    match provider_type {
        "claude" => Ok(Box::new(crate::claude::ClaudeProvider::new(config))),
        "codex" => Ok(Box::new(crate::codex::CodexProvider::new(config))),
        "gemini" => Ok(Box::new(crate::gemini::GeminiProvider::new(config))),
        "hermes" => Ok(Box::new(crate::hermes::HermesProvider::new(config))),
        "mock" => Ok(Box::new(crate::mock::MockProvider::new(config))),
        "openclaw" => Ok(Box::new(crate::openclaw::OpenclawProvider::new(config))),
        "opencode" => Ok(Box::new(crate::opencode::OpencodeProvider::new(config))),
        "cursor" => Ok(Box::new(crate::stubs::CursorProvider::new(config))),
        _ => Err(ProviderError::UnknownProvider(provider_type.to_string())),
    }
}
```

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass — parse tests for claude/gemini/openclaw/opencode/hermes + factory tests

- [ ] **Step 4: Run `cargo clippy` and fix warnings**

Run: `cargo clippy -p gitim-agent-provider -- -D warnings`

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore: final wiring and clippy fixes for new providers"
```
