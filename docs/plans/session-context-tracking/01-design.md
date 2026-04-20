# Session Context Window Tracking — Design

**Status**: Draft · 2026-04-20
**Scope**: gitim-runtime + gitim-agent-provider
**Feature**: Per-session context window usage tracking, 80% threshold one-shot injection, and runtime-side tokenizer estimation for validation.

---

## 1. Problem & Motivation

Agents in GitIM run as long-lived CLI sessions (`claude --resume <token>` / `codex exec resume <thread_id>`). Claude's context is capped (~200k for Sonnet, ~1M for Opus 4.7[1m]). When a session approaches the limit, the agent does not know — Claude CLI silently truncates or errors out late, and the runtime has no visibility into window pressure either.

We want a runtime-internal mechanism that:

1. Reads the current window usage from each provider turn and records it per session.
2. Exposes this snapshot via HTTP + SSE so WebUI can display "@handler: context 47%".
3. When the 80% threshold is crossed, injects a one-shot system notice into the **next** user prompt telling the agent to finalize and emit `[[RESET]]`. The existing `[[RESET]]` mechanism then clears the session on the runtime side and the next cycle cold-starts with a fresh window.
4. Also runs a **runtime-side tokenizer estimate in parallel** (tiktoken) so we can log `provider_reported_pct` vs `estimated_pct` side by side. Goal: validate that the estimator is accurate enough to drive threshold logic for future providers that don't self-report usage.

This collectively enables a "feels infinite" context experience: the agent is guided to checkpoint memory and hand off to a fresh window before the wall is hit.

## 2. Goals / Non-Goals

### Goals
- Parse usage from Claude CLI `result` messages and Codex CLI `token_count` events.
- Maintain per-agent, per-session `SessionUsageSnapshot` in `AgentState`, persisted to `.gitim/agent-state.json`.
- Compute `used_percent` from provider-reported data as the authoritative source for threshold logic.
- Run a tiktoken-rs estimator in parallel on the same session, persisted internally only (no HTTP exposure).
- Inject a one-shot "please finalize and emit `[[RESET]]`" notice when `used_percent` crosses 80%.
- Expose snapshot via `GET /agents/:id` (field `session_usage`) and SSE `AgentActivityEvent` (event_type `"usage"`).
- Log a comparison line at `info` level whenever the 80% threshold is crossed; log per-turn usage at `debug`.
- **Surface session_id + used_percent in the WebUI `AgentStatusPanel` expansion popover** (top-right of the "Recent Activity" header). Folded view stays unchanged.

### Non-Goals (explicitly deferred)
- `me.json`-level user-configurable thresholds or `max_tokens` override. v1 ships hardcoded per-provider/model defaults; the override story ships with the future "edit agent config" UX (not this feature).
- Historical session log (`.gitim/session-history.jsonl` or similar). Only current session is tracked; `[[RESET]]` clears state.
- Forced auto-reset at any threshold. The agent is the one who emits `[[RESET]]`; runtime only sends the notice.
- Multi-tier thresholds (70% warn / 90% critical). Single 80% cliff only.
- CLI surface (`gitim status` showing usage). HTTP + SSE + WebUI popover only.
- Changing the `[[RESET]]` detection protocol.
- Estimator-driven threshold decisions. Even with tiktoken data flowing, threshold is always driven by `provider_reported` when present. Estimator is observation-only in v1.

## 3. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  agent_loop::run_once()                                         │
│                                                                 │
│   poll → format_changes_as_prompt ─┐                            │
│                                    ▼                            │
│                        ┌───────────────────────┐                │
│                        │ inject_usage_notice_  │  if pending    │
│                        │ if_pending(prompt)    │───────►prepend │
│                        └───────────────────────┘                │
│                                    │                            │
│                                    ▼                            │
│                        build_exec_options ── tokenize prompt ──┐│
│                                    │                           ││
│                                    ▼                           ▼│
│                        provider.execute(prompt, opts)          ││
│                                    │                           ││
│                                    ▼                           ││
│               drain events ─ collect assistant text ───────────┤│
│                                    │                           ││
│                                    ▼                           ▼│
│                    provider.extract_usage(&ExecResult)         ││
│                              ↓                                  │
│                   ┌──────────┴──────────┐                       │
│                   │                     │                       │
│                   ▼                     ▼                       │
│            provider-reported     estimated_tokens               │
│            (authoritative)       (+= tokenize(assistant_text))  │
│                   │                     │                       │
│                   └──────────┬──────────┘                       │
│                              ▼                                  │
│           compute snapshot → save_state → emit SSE              │
│                              │                                  │
│                              ▼                                  │
│           threshold_crossed_80pct? ── yes ──► set               │
│                                             usage_notice_       │
│                                             pending=true        │
│                                             log comparison      │
└─────────────────────────────────────────────────────────────────┘
```

## 4. Detailed Design

### 4.1 Provider Usage Extraction

Usage is plumbed through `ExecResult` directly — no new trait method. Each provider's existing `drive_session` is the accumulator: it already sees the stream, so it fills in `usage` before sending `ExecResult` back on `result_tx`.

```rust
// gitim-agent-provider::types
pub struct ExecResult {
    pub status: ExecStatus,
    pub output: String,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub session_token: Option<String>,
    pub usage: Option<ProviderUsage>,   // NEW
}

pub struct ProviderUsage {
    pub input_tokens: Option<u64>,   // Claude: from result message's usage block
    pub output_tokens: Option<u64>,  // Claude: from result message's usage block
    pub used_percent: Option<f64>,   // Codex: rate_limits.primary.used_percent
}
```

Claude and Codex fill different subsets — only `input_tokens` (Claude) or only `used_percent` (Codex). The runtime's snapshot-computation logic (§4.5) knows how to produce a unified `used_percent` from either shape.

**Claude implementation** (`crates/gitim-agent-provider/src/claude.rs`):
- Extend `RawMessage` with `usage: Option<ClaudeUsage>`.
- Extend `ParsedMessage::Result` with `usage: Option<ClaudeUsage>`.
- `drive_session` captures the last `ParsedMessage::Result`'s `usage` and puts it in `ExecResult.usage` before sending.

**Codex implementation** (`crates/gitim-agent-provider/src/codex.rs`):
- The event parser already recognizes `event_msg` with `payload.type = "token_count"`. Add a pass in `drive_session` that, on each such event, overwrites a local `latest_used_percent: Option<f64>`.
- On successful completion, pack that value into `ExecResult.usage.used_percent`.

**Mock implementation**:
- Accept a test-configurable synthetic `ProviderUsage` via the mock's config so threshold behavior can be unit-tested end-to-end without real CLIs.

### 4.2 Runtime-Side Estimation (tiktoken)

**Crate**: `tiktoken-rs` (added to `crates/gitim-runtime/Cargo.toml`).

**Encoder selection**:
- Claude agent → `cl100k_base` (closest reasonable open reference; Anthropic's tokenizer is not open)
- Codex agent → `o200k_base` (GPT-4o/5 family)
- Unknown provider → `cl100k_base`

**Initialization**: process-wide `OnceLock<Arc<CoreBPE>>` per encoding, lazy-init on first use. Cold-start cost ~100ms, amortized across all agent loops in the runtime process.

**Accumulation pattern** (in `agent_loop::run_once`):

The tokenizer is applied twice per turn — once **before** calling the provider (for the input side), once **after** the stream ends (for the freshly-received assistant text). The state is updated in place.

```
// At start of run_once:
cold_start = state.session_token.is_none()

// Before provider.execute:
if cold_start { state.estimated_tokens = 0 }                    // reset budget
state.estimated_tokens += tokenize(prompt)
if cold_start {
    state.estimated_tokens += tokenize(system_prompt_string)     // one-off
}

// During event drain: accumulate all Text event payloads into assistant_text_buf

// After provider returns successfully:
state.estimated_tokens += tokenize(assistant_text_buf)
```

"Cold start" here means `session_token == None` at entry of `run_once`, i.e. the previous cycle either had no session or reset it. The assistant text collected during a turn is folded in **before** the snapshot is computed, so the snapshot for turn N already reflects everything Claude saw or wrote in turns 1..N.

Tool call inputs and tool-result payloads are out of scope for v1 estimation — they can be large but parsing them uniformly across providers is more surface than the validation ROI justifies. Logged delta_pp will reflect this omission; it's an honest data point.

**Persistence**:
- `AgentState.estimated_tokens: u64` — running sum for the current session. Reset to 0 when `session_token` is cleared on `[[RESET]]`.
- Stored alongside `session_usage` in `.gitim/agent-state.json`. Not exposed via HTTP.

**Accuracy budget**: cl100k vs real Claude tokenization typically differs by 5–15%. Good enough for a v2 fallback driver; we'll see the distribution from logs before trusting it.

### 4.3 Session Usage State

Extensions to `AgentState` (in `crates/gitim-runtime/src/state.rs`):

```rust
pub struct AgentState {
    pub cursor: Option<String>,
    pub session_token: Option<String>,

    // NEW
    pub session_usage: Option<SessionUsageSnapshot>,
    pub estimated_tokens: u64,        // runtime tiktoken running sum (internal)
    pub usage_notice_pending: bool,   // 80% crossed, notice not yet injected
}

pub struct SessionUsageSnapshot {
    pub session_id: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub max_tokens: Option<u64>,      // claude: from default table; codex: None
    pub used_percent: f64,            // authoritative value driving threshold
    pub source: UsageSource,
    pub updated_at: String,           // RFC3339
}

pub enum UsageSource {
    ProviderReported,
    RuntimeEstimated,
}
```

**Reset triggers** (when `[[RESET]]` is detected or session fails): `session_token`, `session_usage`, `estimated_tokens`, `usage_notice_pending` all cleared/zeroed as a single atomic `save_state()` call.

**Backward compatibility**: `.gitim/agent-state.json` is written by `serde_json` with field-level `#[serde(default)]` already. New fields default to `None` / `0` / `false`; old state files load cleanly.

### 4.4 Default Max Tokens Table

New module: `crates/gitim-runtime/src/context_window.rs`.

```rust
pub const WARN_AT_PERCENT: f64 = 80.0;

pub fn default_max_tokens(provider: &str, model: &str) -> Option<u64> {
    match provider {
        "claude" => {
            if model.contains("opus-4-7") && model.contains("1m") {
                Some(1_000_000)
            } else {
                Some(200_000)
            }
        }
        "codex" => None,  // codex reports used_percent directly
        "mock"  => Some(10_000),  // small for unit-testable scenarios
        _       => Some(200_000), // conservative fallback for unknown providers
    }
}
```

This is internal; no configuration knob in v1.

### 4.5 Threshold Detection & Injection

**Crossing detection** (computed after each provider turn completes):

```
prev_percent = state.session_usage.map(|s| s.used_percent).unwrap_or(0.0)
new_percent  = compute_authoritative_percent(provider_reported, estimated, max_tokens)

just_crossed = prev_percent < WARN_AT_PERCENT && new_percent >= WARN_AT_PERCENT

if just_crossed {
    state.usage_notice_pending = true;
    log::info!(threshold_crossed_80pct, provider={..}, estimated={..}, delta_pp={..})
}
```

Note: crossing is detected **once per session**. After the notice is dispatched (next turn), `usage_notice_pending` is cleared. It will not re-fire unless the session is reset and reaches 80% again.

**Injection point** (top of `run_once`, after `format_changes_as_prompt` returns):

```rust
let mut prompt = base_prompt;
if state.usage_notice_pending {
    prompt = format!(
        "[系统通知] 对话窗口已用 {pct:.0}%，容量接近上限。\n\
         \n\
         你手里大概同时压着好几件事。继续在这个窗口里推进的边际收益已经很低 —— \
         注意力被稀释，新细节越来越难稳定保持。此刻最有价值的动作不是把手头的事收尾，\
         而是给下一个窗口的你做一次干净的交接。\n\
         \n\
         请立即：\n\
         1. 停下所有新的工具调用和任务步骤\n\
         2. 在你的记忆文件里留一段 orientation（方向感，不是流水账）：\n\
            - 当前任务位置 / 下一步该做什么\n\
            - 已经形成但还没落笔的判断、用户偏好、关键未决 tension\n\
            - 让冷启动的你能快速接回当前位置的最小定向信息\n\
         3. 输出末尾附加 [[RESET]]，runtime 会给你开一个干净的新窗口\n\
         \n\
         新窗口的你会读这些记忆文件冷启动。你此刻留下什么，它就从什么开始。\n\
         \n\
         本提醒仅发送一次。\n\n---\n\n{prompt}",
        pct = state.session_usage.unwrap().used_percent
    );
    state.usage_notice_pending = false;
    state.save()?;
}
```

Save **before** calling `provider.execute()` so a crash mid-turn doesn't cause the notice to re-fire.

**Framing note.** The preamble is deliberately written as agent-to-agent, not a procedural checklist. It acknowledges what context pressure *feels like* from inside the model (attention diluting, new details becoming unstable), reframes the goal as **handoff, not completion**, and privileges orientation over inventory in the memory handoff. This is locked by a `preamble_frames_as_handoff_not_completion` test that asserts the absence of "finish your work first" wording.

**Authoritative value policy**:

```
1. provider_reported.used_percent             (Codex path — takes it as-is)
2. provider_reported.input_tokens / max_tokens (Claude path; requires max_tokens Some)
3. estimated_tokens / max_tokens              (fallback when provider reports nothing
                                               AND max_tokens is Some)
4. no snapshot                                 (no provider data, no max — e.g. a future
                                               Codex-style provider that stopped emitting
                                               used_percent)
```

`source` field records which rung was used. In v1:
- Claude consistently reports — rung 2.
- Codex consistently reports `used_percent` — rung 1.
- Rung 3 and 4 are not reachable under normal operation; they're defined so the logic is total, and so the first provider that ever drops usage still produces *something* usable.

The estimator is computed on every turn regardless of which rung was taken, and both values land in the log line (§4.7). The HTTP snapshot, however, carries only the authoritative number.

### 4.6 API Surface

**`AgentInfo` extension** (in `crates/gitim-runtime/src/http.rs`):

```rust
pub struct AgentInfo {
    // existing fields ...

    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<SessionUsageSnapshot>,
}
```

Populated from `AgentState.session_usage` at recovery time and kept in-memory-in-sync by the agent loop after each turn.

**SSE event** (extending `AgentActivityEvent`):

A new `event_type = "usage"` with `detail` carrying a JSON-serialized `SessionUsageSnapshot`. This piggybacks on the existing `/agents/events` stream — no new route, no new consumer plumbing.

```json
{
  "agent_id": "alice",
  "workspace_id": "acme",
  "event_type": "usage",
  "detail": "{\"session_id\":\"...\",\"input_tokens\":124000,\"used_percent\":62.0,\"source\":\"provider_reported\",\"updated_at\":\"...\"}",
  "timestamp": "2026-04-20T12:34:56Z"
}
```

**WebUI v2 integration** (part of this feature's scope):

- `webui-v2/src/lib/types.ts`: extend `Agent` with
  ```ts
  sessionUsage?: {
    sessionId: string;
    usedPercent: number;
    source: "provider_reported" | "runtime_estimated";
    updatedAt: string;
  };
  ```
- `webui-v2/src/lib/client.ts`: parse `session_usage` from `GET /agents/:id` responses and camelCase it into the store.
- `webui-v2/src/hooks/use-agent-activity.ts` (or wherever SSE is consumed): on receiving an event with `event_type === "usage"`, JSON-parse `detail` and patch the corresponding agent's `sessionUsage` in the agent store — do not append to the activity log.
- `webui-v2/src/components/chat/agent-status-panel.tsx` — modify only the expanded popover header row (current single `<p>Recent Activity</p>` line at ~line 88-90). After the change:
  ```
  ┌ name — Recent Activity ·············  sid:abc123 · 47% ┐
  ├─── activity line 1 ──────────────────────────────────────┤
  ├─── activity line 2 ──────────────────────────────────────┤
  ```
  - Left: existing `{name} — Recent Activity`
  - Right: `sid:{sessionId.slice(0, 8)} · {usedPercent.toFixed(0)}%`
  - Show the percent in a soft-warning color (yellow/orange token) when `usedPercent >= 80`; default muted color otherwise.
  - If `sessionUsage` is absent (no usage data yet / first turn not complete), render nothing on the right — no placeholder, no dash.
- **Folded row stays identical.** No percent, no session_id in the folded view.

No new component file. The change is confined to the header row of the existing popover plus a small type + data-flow tail.

### 4.7 Logging

All log calls use `tracing`, matching project convention.

**Per turn** (`debug` level):
```
turn_usage session_id=<sid> provider_pct=42.1 estimated_pct=39.8 delta_pp=-2.3 source=provider_reported
```

**80% crossed** (`info` level — the comparison line is the main reason we're carrying the estimator):
```
threshold_crossed_80pct session_id=<sid>
  provider_input_tokens=164000 provider_used_pct=82.0
  estimated_tokens=156200 estimated_used_pct=78.1
  delta_pp=-3.9 max_tokens=200000
  provider=claude model=claude-sonnet-4-6
```

**Reset** (`info` level):
```
session_reset session_id=<sid> reason=agent_emitted_reset
```

## 5. Error Handling

| Failure | Behavior |
|---|---|
| Provider returns no usage (malformed stream, old CLI) | `ProviderUsage = None`. Fall back to estimated. If estimated also unavailable (first turn of first session and tokenizer crate init failed), skip usage update for this turn; no snapshot written; no threshold check. |
| Tiktoken init fails (unlikely, file-system issue loading BPE) | Log `warn` once, disable estimator for the process. Provider-reported path continues unaffected. |
| `max_tokens` table lookup returns `None` for a Claude call (shouldn't happen — table always matches on `claude` prefix) | Log `warn`, skip usage snapshot for the turn. |
| State save fails mid-turn | Existing error path (`?` propagation) applies; loop surfaces the error to caller as today. Notice-pending flag may be out-of-sync on next start; worst case, notice re-fires once, which is harmless. |
| Provider reports `used_percent > 100` (e.g. Codex after a credit-exhausted turn) | Treat as 100, allow threshold logic to fire normally. Don't panic or clamp silently — log `warn` if > 110 as a signal of provider protocol drift. |

## 6. Testing Plan

**Unit tests** (crate-local):

- `gitim-agent-provider::claude::parse_line` — parse a real `result` message with `usage` block, assert input_tokens / output_tokens extracted.
- `gitim-agent-provider::codex` — parse `token_count` event with non-null `rate_limits.primary.used_percent`, assert captured.
- `gitim-runtime::context_window::default_max_tokens` — table coverage for each `(provider, model)` branch.
- `gitim-runtime::agent_loop` — crossing detection: feed synthetic `ProviderUsage` with pct 70 → 75 → 82 → 95, assert `usage_notice_pending` flips to true only once (on 70→82 transition), stays false on subsequent 82→95.
- `gitim-runtime::agent_loop` — injection: given `usage_notice_pending=true`, assert next prompt starts with `[系统通知]` and flag is cleared after dispatch.
- `gitim-runtime::agent_loop` — reset path: `[[RESET]]` clears all four fields (`session_token`, `session_usage`, `estimated_tokens`, `usage_notice_pending`) atomically.

**Integration tests** (`crates/gitim-runtime/tests/`):

- `session_usage_reported_and_exposed`: run agent loop with mock provider that returns synthetic `ProviderUsage`, verify `GET /agents/:id` returns `session_usage` with correct `used_percent`, verify SSE broadcasts `"usage"` event.
- `threshold_injection_e2e`: mock provider returns 82% on turn 2; assert turn 3's prompt contains the system notice prefix.
- `estimator_logs_match_provider_reported`: capture `tracing` output, verify `delta_pp` stays within ±20pp band for a scripted scenario.

**WebUI** (no test framework installed in `webui-v2` today — rely on TypeScript + manual verification, matching current project convention):

- `tsc -b` in `webui-v2` passes with no new errors after type extensions.
- `eslint` clean.
- The four visual states (no usage / 47% muted / 85% warning / reset → empty) are exercised as part of the manual validation checklist below.

**Manual validation**:

- Point a real Claude agent at an artificial pressure scenario (long input → watch log). Collect ~10 samples over a few days; compare distribution of `delta_pp` values. Target: median |delta_pp| ≤ 10.
- Visual verification in the running WebUI: expand an agent row, confirm the header-right badge updates live as turns progress (SSE path), survives a full page reload (HTTP path), and disappears cleanly after the agent emits `[[RESET]]`.

## 7. File Touch List

### Added
- `docs/plans/session-context-tracking/01-design.md` (this file)
- `crates/gitim-runtime/src/context_window.rs` — `WARN_AT_PERCENT`, `default_max_tokens`, tokenizer helpers

### Modified
- `crates/gitim-agent-provider/src/types.rs` — add `ProviderUsage`, extend `ExecResult` with `usage`
- `crates/gitim-agent-provider/src/provider.rs` — `AgentProvider::extract_usage` trait method (with default `None` for backward compat)
- `crates/gitim-agent-provider/src/claude.rs` — extend `RawMessage` / `ParsedMessage::Result` with `usage`; plumb to `ExecResult`
- `crates/gitim-agent-provider/src/codex.rs` — capture `token_count` event's `used_percent` into accumulator → `ExecResult.usage`
- `crates/gitim-agent-provider/src/mock.rs` — allow synthetic usage via test config
- `crates/gitim-runtime/src/state.rs` — extend `AgentState` with `session_usage`, `estimated_tokens`, `usage_notice_pending`; add `SessionUsageSnapshot`, `UsageSource`
- `crates/gitim-runtime/src/agent_loop.rs` — tokenization calls, crossing detection, notice injection, SSE emit
- `crates/gitim-runtime/src/http.rs` — `AgentInfo.session_usage` field; ensure recovery populates from state
- `crates/gitim-runtime/Cargo.toml` — add `tiktoken-rs`

### Added / Modified (WebUI)
- `webui-v2/src/lib/types.ts` — extend `Agent` with optional `sessionUsage`
- `webui-v2/src/lib/client.ts` — map `session_usage` from HTTP response
- `webui-v2/src/hooks/use-agent-activity.ts` (or store equivalent) — handle SSE `"usage"` event
- `webui-v2/src/components/chat/agent-status-panel.tsx` — header row of the expansion popover shows `sid:xxxxxxxx · NN%`

### Untouched
- `gitim-daemon`: no change. Daemon doesn't speak to providers.
- `gitim-core`, `gitim-sync`, `gitim-index`, `gitim-cli`: no change.

## 8. Rollout

Single-PR deliverable. No migrations needed (state file is forward-compatible via `#[serde(default)]`). After merge:
- Existing running agents continue without usage data until they cold-start.
- First turn after restart populates `session_usage` on the real state.
- Log dashboards / `rg 'threshold_crossed_80pct' logs/` can begin collecting comparison data immediately.

## 9. Open Questions

None blocking v1. The following are future work, already scoped out of this feature:
- `me.json` override for `max_tokens` / `warn_at_percent` — ships with agent-config edit UX.
- Historical session archive for QBR-style usage analytics.
- Session-graph features (e.g. "agent X has had 7 sessions averaging 55% utilization this week").
- Retire the estimator if logs show the provider-reported path is 100% reliable after a few months — or conversely, promote the estimator to threshold driver if a no-reporting provider is added.
