# Agent Steering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow urgent messages (@mention + "急急急!") to interrupt a running agent session and force immediate processing.

**Architecture:** Add `CancellationToken` to `Session` for graceful cancel. During provider execution, the agent loop periodically peeks for new messages. If a steering trigger is detected, it cancels the current session and resumes with the new context in the next cycle. The existing `session_token` / `--resume` mechanism handles continuity.

**Tech Stack:** tokio-util (CancellationToken), existing tokio async primitives

---

## File Structure

```
crates/gitim-agent-provider/
  Cargo.toml                 — add tokio-util dependency
  src/types.rs               — CancellationToken in Session, cancel() method
  src/claude.rs              — select! in drive_session for cancel
  src/codex.rs               — pass CancellationToken to Session::new (no select! yet)

crates/gitim-runtime/
  src/poller.rs              — peek() method
  src/agent_loop.rs          — detect_steering_trigger(), steering in run_once()

Tests:
  crates/gitim-agent-provider/tests/claude_parse_test.rs  — (no change, parse tests unaffected)
  crates/gitim-runtime/tests/agent_loop.rs                — detect_steering_trigger unit tests
  crates/gitim-runtime/tests/poller.rs                    — peek() tests
```

---

### Task 1: Add tokio-util dependency

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/gitim-agent-provider/Cargo.toml`

- [ ] **Step 1: Add tokio-util to workspace dependencies**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
tokio-util = { version = "0.7", features = ["rt"] }
```

- [ ] **Step 2: Add tokio-util to gitim-agent-provider**

In `crates/gitim-agent-provider/Cargo.toml`, add under `[dependencies]`:

```toml
tokio-util = { workspace = true }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p gitim-agent-provider`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/gitim-agent-provider/Cargo.toml Cargo.lock
git commit -m "chore: add tokio-util dependency for CancellationToken"
```

---

### Task 2: Add CancellationToken to Session

**Files:**
- Modify: `crates/gitim-agent-provider/src/types.rs`

- [ ] **Step 1: Add CancellationToken field and cancel() method**

In `types.rs`, add the import:

```rust
use tokio_util::sync::CancellationToken;
```

Add `cancel_token` field to `Session`:

```rust
pub struct Session {
    pub events: mpsc::Receiver<Event>,
    pub result: oneshot::Receiver<ExecResult>,
    abort_handle: AbortHandle,
    cancel_token: CancellationToken,
}
```

Update `Session::new` to accept the token:

```rust
pub fn new(
    events: mpsc::Receiver<Event>,
    result: oneshot::Receiver<ExecResult>,
    abort_handle: AbortHandle,
    cancel_token: CancellationToken,
) -> Self {
    Self {
        events,
        result,
        abort_handle,
        cancel_token,
    }
}
```

Add `cancel()` method. Update the existing `abort()` doc comment to clarify the distinction:

```rust
/// Gracefully cancel the running execution.
///
/// Signals the provider to stop at the next clean point (between tool calls).
/// The provider will send an ExecResult with status=Aborted and a valid
/// session_token for resumption. Prefer this over abort() for steering.
pub fn cancel(&self) {
    self.cancel_token.cancel();
}

/// Hard-abort the running execution.
///
/// Cancels the tokio task immediately via AbortHandle. The child process
/// is killed via kill_on_drop. result_tx may never send, so the caller
/// may get RecvError. Use cancel() for graceful interruption with a
/// proper ExecResult.
pub fn abort(&self) {
    self.abort_handle.abort();
}
```

- [ ] **Step 2: Verify it compiles (expect errors in claude.rs/codex.rs — Session::new arity changed)**

Run: `cargo check -p gitim-agent-provider 2>&1 | head -20`
Expected: compile errors in claude.rs and codex.rs about missing argument to `Session::new`. This confirms the API change propagates.

---

### Task 3: Wire CancellationToken into Claude provider

**Files:**
- Modify: `crates/gitim-agent-provider/src/claude.rs`

- [ ] **Step 1: Create CancellationToken in execute() and pass to drive_session**

In `claude.rs`, add the import at the top:

```rust
use tokio_util::sync::CancellationToken;
```

In the `execute` method, before spawning the task, create the token:

```rust
let cancel_token = CancellationToken::new();
let cancel_token_inner = cancel_token.clone();
```

Update the spawn and Session::new:

```rust
let join_handle = tokio::spawn(async move {
    drive_session(child, stdout, stdin, stderr, event_tx, result_tx, timeout, pid, cancel_token_inner).await;
});

Ok(Session::new(event_rx, result_rx, join_handle.abort_handle(), cancel_token))
```

- [ ] **Step 2: Add cancel_token parameter to drive_session and implement select!**

Update `drive_session` signature — add `cancel_token: CancellationToken` as the last parameter:

```rust
async fn drive_session(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stdin: tokio::process::ChildStdin,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::Sender<Event>,
    result_tx: oneshot::Sender<ExecResult>,
    timeout: Duration,
    pid: u32,
    cancel_token: CancellationToken,
) {
```

Replace the read loop inside `tokio::time::timeout`. Change from:

```rust
let read_result = tokio::time::timeout(timeout, async {
    while let Ok(Some(line)) = reader.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        // ... parse and handle ...
    }
}).await;
```

To:

```rust
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
                            ParsedMessage::System { session_id: sid } => {
                                session_id = sid;
                                try_send_event(&event_tx, Event::Status {
                                    status: "running".to_string(),
                                });
                            }
                            ParsedMessage::AssistantEvents(events) => {
                                num_turns += 1;
                                for event in events {
                                    if let Event::Text { ref content } = event {
                                        output.push_str(content);
                                    }
                                    try_send_event(&event_tx, event);
                                }
                            }
                            ParsedMessage::UserEvents(events) => {
                                for event in events {
                                    try_send_event(&event_tx, event);
                                }
                            }
                            ParsedMessage::Result {
                                session_id: sid,
                                output: result_text,
                                is_error,
                            } => {
                                saw_result = true;
                                session_id = sid;
                                info!(
                                    pid,
                                    is_error,
                                    turns = num_turns,
                                    result_len = result_text.len(),
                                    "claude result received"
                                );
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
                            }
                            ParsedMessage::ControlRequest { request_id, input } => {
                                let response = build_auto_approve_response(&request_id, &input);
                                if let Ok(data) = serde_json::to_vec(&response) {
                                    let mut buf = data;
                                    buf.push(b'\n');
                                    if let Err(e) = stdin.write_all(&buf).await {
                                        warn!("failed to write control response: {e}");
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => break, // EOF
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
}).await;
```

- [ ] **Step 3: Handle Aborted status in post-loop cleanup**

After the timeout block, add the Aborted case. Replace the existing post-loop code:

```rust
if read_result.is_err() {
    final_status = ExecStatus::Timeout;
    final_error = Some(format!("claude timed out after {timeout:?}"));
    let _ = child.start_kill();
} else if !saw_result && final_status == ExecStatus::Completed {
    final_status = ExecStatus::Failed;
    final_error = Some("claude stream ended without a result message".to_string());
}
```

With:

```rust
if read_result.is_err() {
    final_status = ExecStatus::Timeout;
    final_error = Some(format!("claude timed out after {timeout:?}"));
    let _ = child.start_kill();
} else if final_status == ExecStatus::Aborted {
    let _ = child.start_kill();
} else if !saw_result && final_status == ExecStatus::Completed {
    final_status = ExecStatus::Failed;
    final_error = Some("claude stream ended without a result message".to_string());
}
```

Also update the `child.wait()` guard to skip overwriting for Aborted:

```rust
if final_status != ExecStatus::Timeout {
    match child.wait().await {
        Ok(status) if !status.success() && final_status == ExecStatus::Completed => {
            final_status = ExecStatus::Failed;
            final_error = Some(format!("claude exited with status: {status}"));
        }
        Err(e) if final_status == ExecStatus::Completed => {
            final_status = ExecStatus::Failed;
            final_error = Some(format!("failed to wait for claude: {e}"));
        }
        _ => {}
    }
}
```

This is unchanged — the `final_status == ExecStatus::Completed` guards already prevent overwriting Aborted. No modification needed here.

- [ ] **Step 4: Verify it compiles (expect error in codex.rs only)**

Run: `cargo check -p gitim-agent-provider 2>&1 | head -10`
Expected: error in codex.rs about Session::new missing argument. claude.rs should be clean.

---

### Task 4: Update Codex provider for new Session API

**Files:**
- Modify: `crates/gitim-agent-provider/src/codex.rs`

- [ ] **Step 1: Pass a dummy CancellationToken to Session::new**

In `codex.rs`, add the import:

```rust
use tokio_util::sync::CancellationToken;
```

In the `execute` method, update the `Session::new` call (around line 83):

```rust
Ok(Session::new(
    event_rx,
    result_rx,
    join_handle.abort_handle(),
    CancellationToken::new(),
))
```

The codex drive_session does not watch this token. Calling `cancel()` on a codex session is a no-op — the session runs to completion. This is acceptable for MVP.

- [ ] **Step 2: Verify full provider crate compiles**

Run: `cargo build -p gitim-agent-provider`
Expected: compiles with no errors

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p gitim-agent-provider`
Expected: all tests pass (parse tests don't touch Session)

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-agent-provider/
git commit -m "feat(provider): add CancellationToken to Session for graceful cancel

Session now supports cancel() (graceful, sends Aborted result with session_token)
alongside abort() (hard kill). Claude provider watches the token via select! in
drive_session. Codex provider passes a dummy token for now."
```

---

### Task 5: Add peek() to Poller

**Files:**
- Modify: `crates/gitim-runtime/src/poller.rs`
- Test: `crates/gitim-runtime/tests/poller.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/gitim-runtime/tests/poller.rs`:

```rust
#[tokio::test]
async fn test_peek_does_not_advance_cursor() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // Init cursor
    poller.poll().await.unwrap();
    let cursor_before = poller.cursor().unwrap().to_string();

    // Send a message
    client
        .send("general", "peek test message", None, None)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Peek: should see the message
    let peek_result = poller.peek().await.unwrap();
    assert!(!peek_result.changes.is_empty(), "peek should detect new message");

    // Cursor should NOT have advanced
    let cursor_after = poller.cursor().unwrap().to_string();
    assert_eq!(cursor_before, cursor_after, "peek must not advance cursor");

    // Poll: should also see the same message (cursor didn't move)
    let poll_result = poller.poll().await.unwrap();
    assert!(!poll_result.changes.is_empty(), "poll should still get the message");

    // Now cursor has advanced
    let cursor_final = poller.cursor().unwrap().to_string();
    assert_ne!(cursor_before, cursor_final, "poll should advance cursor");

    stop_daemon(&repo_root).await;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p gitim-runtime --test poller -- test_peek_does_not_advance_cursor --nocapture 2>&1 | tail -5`
Expected: compile error — `peek()` method does not exist

- [ ] **Step 3: Implement peek()**

In `crates/gitim-runtime/src/poller.rs`, add the `peek` method to `impl Poller`:

```rust
/// Check for new changes without advancing the cursor.
///
/// Same as `poll()` but does not update the internal cursor.
/// Used by steering detection to check for urgent messages
/// while the provider is executing.
pub async fn peek(&self) -> Result<PollResult, RuntimeError> {
    let resp = self
        .client
        .poll(self.cursor.as_deref())
        .await
        .map_err(|e| RuntimeError::PollFailed(e.to_string()))?;

    if !resp.ok {
        let msg = resp.error.unwrap_or_else(|| "poll failed".into());
        return Err(RuntimeError::PollFailed(msg));
    }

    let data = resp.data.ok_or_else(|| {
        RuntimeError::PollFailed("poll response missing data".into())
    })?;

    // Note: we intentionally do NOT update self.cursor here.

    let changes = data["changes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let channel = c["channel"].as_str()?.to_string();
                    let kind = c["kind"].as_str()?.to_string();
                    let entries = c["entries"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    Some(ChannelChange { channel, kind, entries })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(PollResult { changes })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gitim-runtime --test poller -- test_peek_does_not_advance_cursor --nocapture`
Expected: PASS

- [ ] **Step 5: Run all poller tests**

Run: `cargo test -p gitim-runtime --test poller --nocapture`
Expected: all tests pass (regression check)

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/poller.rs crates/gitim-runtime/tests/poller.rs
git commit -m "feat(poller): add peek() for non-advancing poll

peek() calls the daemon poll API with the current cursor but does not
advance it. Used by steering to check for urgent messages during
provider execution."
```

---

### Task 6: Add detect_steering_trigger()

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Test: `crates/gitim-runtime/tests/agent_loop.rs`

- [ ] **Step 1: Write the failing tests**

Add to the TOP of `crates/gitim-runtime/tests/agent_loop.rs` (before the existing `test_agent_loop_end_to_end`):

```rust
use gitim_runtime::agent_loop::detect_steering_trigger;
use gitim_runtime::poller::ChannelChange;

fn make_entry(author: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "author": author,
        "body": body,
        "line_number": 1,
        "point_to": 0,
        "timestamp": "2026-04-14T00:00:00Z"
    })
}

fn make_changes(entries: Vec<(&str, &str)>) -> Vec<ChannelChange> {
    vec![ChannelChange {
        channel: "general".to_string(),
        kind: "message".to_string(),
        entries: entries
            .into_iter()
            .map(|(author, body)| make_entry(author, body))
            .collect(),
    }]
}

#[test]
fn test_steering_trigger_mention_and_keyword() {
    let changes = make_changes(vec![("alice", "@bot 急急急! 快来看这个bug")]);
    assert!(detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_mention_without_keyword() {
    let changes = make_changes(vec![("alice", "@bot 你好，有空帮忙看看吗")]);
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_keyword_without_mention() {
    let changes = make_changes(vec![("alice", "急急急! 有个紧急问题")]);
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_self_authored_ignored() {
    let changes = make_changes(vec![("bot", "@bot 急急急!")]);
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_empty_changes() {
    let changes: Vec<ChannelChange> = vec![];
    assert!(!detect_steering_trigger(&changes, "bot"));
}

#[test]
fn test_steering_trigger_channel_meta_skipped() {
    let changes = vec![ChannelChange {
        channel: "general".to_string(),
        kind: "channel_meta".to_string(),
        entries: vec![make_entry("alice", "@bot 急急急!")],
    }];
    assert!(!detect_steering_trigger(&changes, "bot"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p gitim-runtime --test agent_loop -- test_steering_trigger --nocapture 2>&1 | tail -5`
Expected: compile error — `detect_steering_trigger` not found

- [ ] **Step 3: Implement detect_steering_trigger()**

In `crates/gitim-runtime/src/agent_loop.rs`, add the function (after `format_changes_as_prompt`):

```rust
/// Check whether any change contains a steering trigger.
///
/// Trigger condition: message from another user that @mentions self_handler
/// AND contains "急急急". Self-authored messages are ignored.
pub fn detect_steering_trigger(changes: &[ChannelChange], self_handler: &str) -> bool {
    let mention = format!("@{self_handler}");
    for change in changes {
        if change.kind == "channel_meta" {
            continue;
        }
        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("");
            if author == self_handler {
                continue;
            }
            let body = entry["body"].as_str().unwrap_or("");
            if body.contains(&mention) && body.contains("急急急") {
                return true;
            }
        }
    }
    false
}
```

- [ ] **Step 4: Export the function**

In `crates/gitim-runtime/src/lib.rs`, update the `agent_loop` re-export:

```rust
pub use agent_loop::{AgentLoop, build_system_prompt, detect_steering_trigger, format_changes_as_prompt};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p gitim-runtime --test agent_loop -- test_steering_trigger -v`
Expected: all 6 tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/tests/agent_loop.rs
git commit -m "feat(agent-loop): add detect_steering_trigger()

Checks poll results for @mention + 急急急 pattern from non-self authors.
Pure function, tested with 6 cases covering match, no-match, self-authored,
empty, and channel_meta edge cases."
```

---

### Task 7: Wire steering into run_once()

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Add tokio-util dependency to gitim-runtime**

In `crates/gitim-runtime/Cargo.toml`, this is NOT needed — the runtime doesn't use CancellationToken directly. It calls `session.cancel()` which is on the provider crate's type. No new dependency for runtime.

- [ ] **Step 2: Refactor run_once() to support steering**

In `crates/gitim-runtime/src/agent_loop.rs`, replace the existing `run_once` method.

Current event drain + result await (lines ~450-502):

```rust
let opts = self.build_exec_options();
let session = self
    .provider
    .execute(&prompt, opts)
    .await
    .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

// Drain events (log + broadcast)
let mut events = session.events;
while let Some(event) = events.recv().await {
    // ... log events ...
}

// Await final result
let exec_result = session.result.await ...
```

Replace with:

```rust
let opts = self.build_exec_options();
let mut session = self
    .provider
    .execute(&prompt, opts)
    .await
    .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

// Drain events with periodic steering check
let mut steering_check = tokio::time::interval(Duration::from_secs(5));
steering_check.tick().await; // consume the immediate first tick

let steered = loop {
    tokio::select! {
        event = session.events.recv() => {
            match event {
                Some(event) => {
                    match &event {
                        gitim_agent_provider::Event::Text { content } => {
                            tracing::debug!(text_len = content.len(), "agent text");
                        }
                        gitim_agent_provider::Event::ToolUse { tool, input, .. } => {
                            let snippet = summarize_tool_input(tool, input);
                            info!(tool = %tool, input = %snippet, "agent tool use");
                            self.emit_activity("tool_use", &format!("{tool}: {snippet}"));
                        }
                        gitim_agent_provider::Event::ToolResult { call_id, output } => {
                            tracing::debug!(call_id = %call_id, output_len = output.len(), "tool result");
                        }
                        gitim_agent_provider::Event::Error { content } => {
                            tracing::warn!(error = %content, "agent error event");
                            self.emit_activity("error", content);
                        }
                        _ => {}
                    }
                }
                None => break false, // event channel closed, normal completion
            }
        }
        _ = steering_check.tick() => {
            match self.poller.peek().await {
                Ok(peek_result) if !peek_result.changes.is_empty() => {
                    if detect_steering_trigger(&peek_result.changes, &self.handler) {
                        info!("steering trigger detected, cancelling session");
                        self.emit_activity("steering", "urgent message detected, interrupting");
                        session.cancel();
                        break true;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "steering peek failed, continuing");
                }
                _ => {}
            }
        }
    }
};

// Await final result
let exec_result = session
    .result
    .await
    .map_err(|_| RuntimeError::ProviderFailed("result channel closed".into()))?;
```

The rest of `run_once()` (result handling, state saving) stays the same. The existing code already handles `ExecStatus::Failed` by clearing `session_token`. For `Aborted`, we want to KEEP the session_token so the next cycle can resume. Add this case in the match:

Replace the existing result handling block:

```rust
let duration_s = exec_result.duration_ms as f64 / 1000.0;
if exec_result.status == ExecStatus::Failed {
    tracing::error!(
        duration_ms = exec_result.duration_ms,
        error = ?exec_result.error,
        output = %exec_result.output.chars().take(300).collect::<String>(),
        "provider failed"
    );
    self.emit_activity("error", "execution failed");
    self.session_token = None;
} else {
    info!(
        duration_ms = exec_result.duration_ms,
        output = %exec_result.output.chars().take(100).collect::<String>(),
        "provider ok"
    );
    self.emit_activity("done", &format!("done ({duration_s:.1}s)"));
    if let Some(token) = exec_result.session_token {
        self.session_token = Some(token);
    }
}
```

With:

```rust
let duration_s = exec_result.duration_ms as f64 / 1000.0;
match exec_result.status {
    ExecStatus::Failed => {
        tracing::error!(
            duration_ms = exec_result.duration_ms,
            error = ?exec_result.error,
            output = %exec_result.output.chars().take(300).collect::<String>(),
            "provider failed"
        );
        self.emit_activity("error", "execution failed");
        self.session_token = None;
    }
    ExecStatus::Aborted => {
        info!(
            duration_ms = exec_result.duration_ms,
            "provider aborted by steering"
        );
        self.emit_activity("steered", &format!("interrupted ({duration_s:.1}s)"));
        // Keep session_token for resume in next cycle
        if let Some(token) = exec_result.session_token {
            self.session_token = Some(token);
        }
    }
    _ => {
        info!(
            duration_ms = exec_result.duration_ms,
            output = %exec_result.output.chars().take(100).collect::<String>(),
            "provider ok"
        );
        self.emit_activity("done", &format!("done ({duration_s:.1}s)"));
        if let Some(token) = exec_result.session_token {
            self.session_token = Some(token);
        }
    }
}
```

- [ ] **Step 3: Add ExecStatus import**

At the top of `agent_loop.rs`, update the import:

```rust
use gitim_agent_provider::{ExecOptions, ExecStatus, Provider, ProviderConfig, create};
```

This import already includes `ExecStatus`, so no change needed. Verify it's there.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p gitim-runtime`
Expected: compiles with no errors

- [ ] **Step 5: Run all existing tests**

Run: `cargo test`
Expected: all tests pass (regression check)

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(agent-loop): wire steering into run_once()

During provider execution, peek for new messages every 5 seconds.
If @mention + 急急急 is detected, cancel the session gracefully.
The session_token is preserved so the next run_once() cycle resumes
the conversation with the urgent message as new context."
```
