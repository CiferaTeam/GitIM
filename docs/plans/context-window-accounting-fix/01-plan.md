# Context Window Accounting Fix — Implementation Plan

> Follow-up to `session-context-tracking/`. Repro session: Claude sid `f6cf86eb-a78d-4d61-87b8-2edc2d1985ae` (framer-opus, gitim-company workspace, 2026-04-21 07:34 UTC).

**Goal:** Fix three defects observed in the context-window HUD + `[[RESET]]` handshake:

1. `used_percent` is ~3× inflated for Claude sessions that do multi-iteration tool-use chains
2. After `[[RESET]]` / abort, the WebUI HUD keeps displaying the stale pre-reset percentage and stays on "processing..."
3. The threshold preamble's step 1 ("停下所有新的工具调用") literally contradicts step 2 ("在记忆文件里留一段 orientation"), leading the agent into sloppy execution order

**Architecture:**

- **Fix #1** lives in the Claude provider parser. Claude CLI's `type:result` usage is an aggregate across all inner iterations in the CLI invocation (for `turns=N`, input + cache_read + cache_creation gets counted N times). Window occupancy is the **last** iteration's per-request usage, which is carried on each `type:assistant` message. We start capturing that and fall back to `result.usage` only when we saw no assistant message.
- **Fix #2** lives in `agent_loop.rs` reset + abort paths. `clear_session()` already clears disk state; we mirror that into the in-memory `RuntimeState.workspaces[slug].agents[handler].session_usage` and emit a terminal `"done"` activity event so reactive UI clears `processing...`.
- **Fix #3** is a preamble copy edit. Keep the one-shot delivery + `[[RESET]]` sentinel protocol; collapse the contradictory numbered list into a single imperative that resolves "orientation vs. no-tool-use" by naming the allowed tools explicitly.

**Tech Stack:** Rust, Tokio, `cargo test`, existing `tests/session_usage_e2e.rs` + `tests/threshold_injection_e2e.rs` harnesses.

---

## File Map

| File | Role | Change |
|------|------|--------|
| `crates/gitim-agent-provider/src/claude/mod.rs` | Claude CLI stream-json parser + session driver | Capture usage from the latest `assistant` message; use it in preference to `result.usage` |
| `crates/gitim-runtime/src/agent_loop.rs` | RESET + abort handling | Clear in-memory session_usage + emit "done" activity on RESET and on aborted/failed execute |
| `crates/gitim-runtime/src/agent_loop.rs` (preamble only) | `build_usage_notice_preamble` | Rewrite copy |
| `crates/gitim-agent-provider/src/claude/mod.rs` tests | New unit test | Triple-iteration fixture must yield last-iteration usage, not the triple sum |
| `crates/gitim-runtime/src/agent_loop.rs` tests | Updated test | Preamble text assertion updated; new assertion for `clear_session` + runtime patch |

---

## Task 1 — Capture per-iteration usage from assistant stream

**Files:**
- Modify: `crates/gitim-agent-provider/src/claude/mod.rs` (parse_line + drive_session)
- Test: `crates/gitim-agent-provider/src/claude/mod.rs` (inline `#[cfg(test)]`)

### Context the agent needs

- `parse_line` currently returns `ParsedMessage::AssistantEvents(Vec<Event>)` without exposing usage. `ParsedMessage::Result` carries the only `ProviderUsage`.
- In `drive_session`, `captured_usage` is set from `ParsedMessage::Result` only.
- Claude Code CLI emits a `type:assistant` line **per inference** (each iteration of a tool-use chain). Its `message.usage` is the per-request usage (`input_tokens` + `cache_read_input_tokens` + `cache_creation_input_tokens`) for that iteration. The final `type:result` has `usage` that **sums** across iterations — correct for billing totals, wrong as a window-occupancy denominator.
- Real fixture, T1 of sid f6cf86eb, three iterations with effective 57,936 / 59,561 / 59,886 tokens respectively. `result.usage` summed → 177,383 → 88.69 % of a 200 k window. True occupancy = 59,886 → 29.9 %.

### Step 1: Extend `RawMessage` + `MessageContent` to carry usage from assistant messages

Current `MessageContent` (grep `struct MessageContent` in `claude/mod.rs`):

```rust
#[derive(Deserialize)]
struct MessageContent {
    content: Vec<ContentBlock>,
}
```

Anthropic's assistant message JSON includes a sibling `usage` field next to `content`. Add:

```rust
#[derive(Deserialize)]
struct MessageContent {
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: Option<RawUsage>,
}
```

`RawUsage` already exists and is used by the `result` parser — reuse it.

### Step 2: Extend `ParsedMessage::AssistantEvents` to carry optional usage

```rust
pub enum ParsedMessage {
    System { session_id: String },
    AssistantEvents {
        events: Vec<Event>,
        usage: Option<ProviderUsage>,
    },
    UserEvents(Vec<Event>),
    Result {
        session_id: String,
        output: String,
        is_error: bool,
        usage: Option<ProviderUsage>,
    },
    ControlRequest { request_id: String, input: Value },
}
```

Update the `"assistant"` branch of `parse_line`:

```rust
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
        });
        Some(ParsedMessage::AssistantEvents { events, usage })
    }
}
```

### Step 3: In `drive_session`, prefer assistant usage over result usage

Locate the `ParsedMessage::AssistantEvents(events)` match arm (around the `num_turns += 1;` line). Update to:

```rust
ParsedMessage::AssistantEvents { events, usage } => {
    num_turns += 1;
    if usage.is_some() {
        captured_usage = usage;
    }
    for event in events {
        if let Event::Text { ref content } = event {
            output.push_str(content);
        }
        try_send_event(&event_tx, event);
    }
}
```

In the `ParsedMessage::Result` match arm, only overwrite `captured_usage` when we have **not** captured anything from an assistant message yet:

```rust
ParsedMessage::Result {
    session_id: sid,
    output: result_text,
    is_error,
    usage: result_usage,
} => {
    saw_result = true;
    session_id = sid;
    if captured_usage.is_none() {
        captured_usage = result_usage;
    }
    ...
}
```

That keeps the existing behaviour when assistant messages arrive without usage (older CLI versions), but always wins with the per-iteration value when available.

### Step 4: Write failing unit test — triple iteration must not triple-count

Add to the existing `#[cfg(test)] mod tests` in `claude/mod.rs`:

```rust
#[test]
fn assistant_message_usage_is_per_iteration_not_summed() {
    // T1 of sid f6cf86eb — three assistant iterations, then a result that
    // sums the three. The parser should surface the LAST iteration's usage,
    // since that reflects window occupancy. The result.usage is a billing
    // aggregate and must not be used as the denominator.
    let iter1 = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":5,"cache_read_input_tokens":31274,"cache_creation_input_tokens":26657,"output_tokens":196}},"session_id":"sid"}"#;
    let iter2 = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1,"cache_read_input_tokens":57931,"cache_creation_input_tokens":1629,"output_tokens":308}},"session_id":"sid"}"#;
    let iter3 = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":1,"cache_read_input_tokens":59560,"cache_creation_input_tokens":325,"output_tokens":34}},"session_id":"sid"}"#;

    let last = match parse_line(iter3).expect("parsed") {
        ParsedMessage::AssistantEvents { usage, .. } => usage.expect("usage present"),
        _ => panic!("expected AssistantEvents"),
    };
    assert_eq!(last.input_tokens, Some(1));
    assert_eq!(last.cache_read_tokens, Some(59560));
    assert_eq!(last.cache_creation_tokens, Some(325));
    // Sanity: effective = 59886, not 177383.
    let effective = last.input_tokens.unwrap_or(0)
        + last.cache_read_tokens.unwrap_or(0)
        + last.cache_creation_tokens.unwrap_or(0);
    assert_eq!(effective, 59_886);

    // iter1 is just here to prove we can parse earlier iterations too.
    let first = match parse_line(iter1).expect("parsed") {
        ParsedMessage::AssistantEvents { usage, .. } => usage.expect("usage"),
        _ => panic!(),
    };
    assert_eq!(first.cache_read_tokens, Some(31_274));
    let _ = iter2; // reserved for future streaming-order test
}
```

### Step 5: Run the new test — must fail before implementation

```bash
cargo test -p gitim-agent-provider assistant_message_usage_is_per_iteration_not_summed
```

Expected: compile error (`ParsedMessage::AssistantEvents` variant shape) OR assertion failure (`usage` returns `None`) until Step 1-3 are applied.

### Step 6: Apply Step 1-3, rerun test — must pass

```bash
cargo test -p gitim-agent-provider assistant_message_usage_is_per_iteration_not_summed
```

Expected: PASS.

### Step 7: Run the rest of the crate to catch breakage

```bash
cargo test -p gitim-agent-provider
```

Fix any fallout from the `ParsedMessage::AssistantEvents` variant shape change (destructuring in other match arms, if any).

### Step 8: Run runtime crate tests (consumes the parser)

```bash
cargo test -p gitim-runtime
```

Expected: session_usage + threshold e2e tests still pass. If `session_usage_e2e.rs` asserts on summed usage, update to match per-iteration semantics.

### Step 9: Commit

```bash
git add crates/gitim-agent-provider/src/claude/mod.rs
git commit -m "fix(runtime): use per-iteration usage for Claude window accounting

Claude CLI's type:result usage is the billing aggregate across every
inner iteration in a tool-use chain. Dividing that by max_tokens
triple-counts cached context and inflates the window-occupancy
percentage (observed: 88.69% for a session whose real occupancy was
29.9%).

Capture usage from each type:assistant message instead; fall back to
the result event only when no assistant usage was seen."
```

---

## Task 2 — Clear stale HUD on RESET + emit terminal activity

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

### Context the agent needs

- `AgentState::clear_session()` already nukes `session_token`, `session_usage`, `estimated_tokens`, `usage_notice_pending` on disk.
- The in-memory mirror lives in `self.runtime_state.lock().workspaces[workspace_id].agents[handler].session_usage` and is patched by `update_session_usage` (lines ~279-291) **only** when a new snapshot is `Some`. RESET produces no new snapshot → patch never fires → WebUI keeps showing the pre-reset percentage.
- `emit_activity` sends `AgentActivityEvent` to SSE subscribers; currently it is called with `("thinking", "processing...")` at run_once start. There is no symmetric "done"/"reset" event when the run ends — normal completion, reset, abort, and failure all look the same to the HUD, which is why it sticks on `processing...`.

### Step 1: Read the current reset path to anchor the edit

Open `crates/gitim-runtime/src/agent_loop.rs`. Locate the `[[RESET]]` detection branch — search for `"context reset complete, clearing session_token"`. Nearby you'll find:

```rust
self.session_token = None;
// ... save state ...
tracing::info!(session_id = ?sid_for_log, reason = "agent_emitted_reset", "session_reset");
```

And the execute-result error branches around `self.session_token = None;` following a `ProviderFailed` / aborted result.

### Step 2: Add a helper that clears the in-memory snapshot

Add at the end of the `impl AgentLoop` block (after `update_session_usage`):

```rust
/// Clear the runtime's in-memory `session_usage` mirror so the WebUI
/// HUD stops showing the pre-reset percentage. Pairs with
/// `AgentState::clear_session()` on disk.
fn clear_runtime_session_usage(&self) {
    if let Some(rs) = &self.runtime_state {
        if let Ok(mut s) = rs.lock() {
            if let Some(ctx) = s.workspaces.get_mut(&self.workspace_id) {
                if let Some(info) = ctx.agents.get_mut(&self.handler) {
                    info.session_usage = None;
                }
            }
        }
    }
    // Emit an empty usage event so reactive clients patch their store.
    self.emit_activity("usage", "");
}
```

### Step 3: Call it from the RESET branch

In the `[[RESET]]` handling block (right after `self.session_token = None;` and before the `session_reset` tracing line), add:

```rust
self.clear_runtime_session_usage();
self.emit_activity("done", "reset");
```

### Step 4: Call it from the abort / failure branches

For the `ExecStatus::Aborted` and `ExecStatus::Failed` / `ExecStatus::Timeout` arms that already set `self.session_token = None;`, add the same two calls so the HUD does not hang on `processing...` when the session dies without a normal completion.

If there is a single post-execute branch that sets `session_token = None;` on failure, adding the calls there is enough; do not duplicate.

### Step 5: Emit "done" on normal completion

Find the end-of-`run_once` success path (after `update_session_usage` returns `Ok`) and add:

```rust
self.emit_activity("done", "");
```

This gives the HUD a deterministic terminal signal for every run_once outcome.

### Step 6: Update `tests/session_usage_e2e.rs` to cover the clear path

Add a test case that:

1. Seeds `AgentState` with `session_usage = Some(snapshot(pct: 89.0))`, `usage_notice_pending = true`.
2. Patches the `runtime_state` agent's `session_usage` to match.
3. Invokes the RESET-handling path (mock provider returns text containing `[[RESET]]`).
4. Asserts `runtime_state.lock().workspaces[..].agents[..].session_usage` is `None` afterwards.
5. Asserts an activity event of type `"done"` was sent.

Sketch:

```rust
#[tokio::test]
async fn reset_clears_runtime_session_usage_and_emits_done() {
    // ... existing harness setup ...
    let tx = ctx.activity_tx.clone();
    let mut rx = tx.subscribe();

    // Seed as if we crossed 80% already.
    let mut state = AgentState::load(&repo).unwrap();
    state.session_usage = Some(SessionUsageSnapshot {
        session_id: "sid".into(),
        input_tokens: Some(7),
        output_tokens: Some(100),
        max_tokens: Some(200_000),
        used_percent: 89.0,
        source: UsageSource::ProviderReported,
        updated_at: "2026-04-21T07:34:02Z".into(),
    });
    state.usage_notice_pending = false;
    state.save(&repo).unwrap();
    // mirror in memory
    ctx.agents.get_mut(&handler).unwrap().session_usage = state.session_usage.clone();

    // Drive a run_once whose mock provider emits [[RESET]].
    run_once_with_reset(&mut agent_loop).await;

    // In-memory must be cleared.
    let after = state_lock.lock().unwrap();
    let info = after.workspaces[SLUG].agents.get(&handler).unwrap();
    assert!(info.session_usage.is_none(), "RESET must clear HUD snapshot");

    // A "done" event must have been emitted.
    let saw_done = collect_events(&mut rx).await.iter().any(|e| e.event_type == "done");
    assert!(saw_done, "RESET must emit a terminal done event");
}
```

Adapt to the actual helpers present in the e2e file; do not invent helpers that do not exist.

### Step 7: Run the new test alone — must fail before Step 2-5 are applied

```bash
cargo test -p gitim-runtime reset_clears_runtime_session_usage_and_emits_done
```

Expected: assertion failure on "RESET must clear HUD snapshot" OR "must emit done event".

### Step 8: Apply Step 2-5, rerun — must pass

```bash
cargo test -p gitim-runtime reset_clears_runtime_session_usage_and_emits_done
```

Expected: PASS.

### Step 9: Re-run existing e2e + unit tests for the crate

```bash
cargo test -p gitim-runtime
```

Fix fallout — `threshold_injection_e2e.rs` and `session_usage_e2e.rs` may observe the new `"done"` event; update any strict activity-event assertions.

### Step 10: Commit

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/tests/session_usage_e2e.rs
git commit -m "fix(runtime): clear HUD session_usage on RESET and emit terminal activity

On [[RESET]], abort, and normal completion, the WebUI HUD was left
showing the stale pre-reset percentage and stuck on processing...
Clear the in-memory mirror to match the on-disk clear_session(), and
emit a 'done' activity event so reactive clients can terminate the
spinner."
```

---

## Task 3 — Rewrite the threshold preamble

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs::build_usage_notice_preamble`
- Update: any test that asserts on preamble text (grep for `对话窗口已用` and `系统通知`)

### Context the agent needs

- Current copy (see existing function): numbered list of 3 steps. Step 1 says "停下所有新的工具调用和任务步骤"; step 2 says "在你的记忆文件里留一段 orientation". Editing a memory file requires Read + Edit tool calls, so step 1 literally blocks step 2.
- In the repro session (sid f6cf86eb), the agent honored step 2/3 and emitted `[[RESET]]` — but also fired two misdirected DMs to `flame4--maker-01` in between. That is classic context-pressure slop; a contradictory prompt compounds it.
- The new copy should:
  - Use one flowing imperative, not a numbered list
  - Name the allowed tool surface explicitly (Read + Edit of memory files, then the sentinel)
  - Preserve the one-shot delivery notice ("本提醒仅发送一次") so the agent does not expect another nudge
  - Preserve the `[[RESET]]` sentinel contract — the runtime's text_tail matcher looks for exactly that string

### Step 1: Write the new preamble text

Replace the body of `build_usage_notice_preamble` with:

```rust
pub fn build_usage_notice_preamble(used_percent: f64) -> String {
    format!(
        "[系统通知] 对话窗口已用 {pct:.0}%。\n\
         \n\
         此刻对你最有价值的动作是给下一个窗口的你做一次干净的交接 —— \
         注意力被稀释后继续推进新任务的边际收益很低。\n\
         \n\
         请只做这一件事：在你的记忆文件里写一段 orientation（方向感，不是流水账）—— \
         当前任务位置 / 下一步该做什么 / 已经形成但还没落笔的判断、用户偏好、\
         关键未决 tension —— 让冷启动的你能快速接回。\n\
         \n\
         允许的工具：Read 和 Edit 记忆文件。不要发消息、不要回复用户、不要启动新任务。\
         写完后，在输出末尾附加 [[RESET]]，runtime 会给你开一个干净的新窗口。\n\
         \n\
         新窗口的你会读这些记忆文件冷启动 —— 你此刻留下什么，它就从什么开始。\
         本提醒仅发送一次。",
        pct = used_percent,
    )
}
```

### Step 2: Update text-level tests that assert on the old body

Search:

```bash
grep -rn "对话窗口已用\|窗口容量\|容量接近上限" crates/
```

For each hit, update the assertion to match the new copy. Keep assertions that check pct formatting (`{pct:.0}%`), the presence of `[[RESET]]`, and "本提醒仅发送一次" — those are contract points.

### Step 3: Run preamble-scoped tests

```bash
cargo test -p gitim-runtime build_usage_notice_preamble
cargo test -p gitim-runtime threshold
```

Expected: PASS.

### Step 4: Commit

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "fix(runtime): rewrite context-window preamble to remove self-contradiction

The old 'stop all tool calls' step literally contradicted the 'write
orientation memory' step, which requires Read + Edit. Under context
pressure, agents interpreted the conflict loosely and fired
misdirected side actions (e.g. wrong-channel DMs) before emitting
[[RESET]].

Collapse the numbered list into one flowing imperative that names the
allowed tool surface (Read + Edit of memory files) explicitly."
```

---

## Task 4 — Full regression

```bash
cargo test
```

Expected: all 700+ tests pass. If any unrelated tests fail, they were red before this plan — diff against the baseline from the start of the worktree. Do not fix unrelated red tests here; call them out and leave them alone.

---

## Self-review notes

- **Coverage:** Task 1 fixes Issue #1 (calc). Task 2 fixes Issue #3 (stale HUD + stuck processing). Task 3 fixes Issue #2 (preamble contradiction).
- **Placeholders:** None. Every step has exact file paths, code, commands.
- **Types:** `ProviderUsage`, `ParsedMessage::AssistantEvents` variant shape, and `SessionUsageSnapshot` match their existing definitions; only `MessageContent.usage` and the `AssistantEvents` variant fields change shape. All call-sites of `ParsedMessage::AssistantEvents(events)` in the crate need to be audited in Task 1 Step 7.
- **Open risk:** older Claude CLI versions may not emit `usage` on every `assistant` message. The fallback in Task 1 Step 3 ("keep `result.usage` if no assistant usage seen") handles that. This is tested implicitly by any existing unit test that only supplies a `result` event.
