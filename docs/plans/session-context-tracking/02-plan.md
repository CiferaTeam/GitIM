# Session Context Window Tracking — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-session context window usage tracking, 80% one-shot threshold injection, parallel tiktoken estimator for validation, and a WebUI badge in the agent status popover.

**Architecture:** Usage data flows `provider CLI → ExecResult.usage → agent_loop snapshot computation → AgentState persistence → AgentInfo HTTP + SSE → WebUI`. The 80% crossing is detected post-turn and injects a one-shot prompt preamble on the next turn.

**Tech Stack:** Rust (gitim-runtime, gitim-agent-provider) · TypeScript + React 19 + Zustand (webui-v2) · tiktoken-rs for tokenizer estimation.

**Spec:** [01-design.md](./01-design.md)

---

## File Structure

### New files
- `crates/gitim-runtime/src/context_window.rs` — constants, `default_max_tokens` table, tokenizer helpers
- `crates/gitim-agent-provider/tests/usage_parsing_test.rs` — provider usage extraction unit tests

### Modified files — Rust
- `crates/gitim-agent-provider/src/types.rs` — `ProviderUsage` type, `ExecResult.usage` field
- `crates/gitim-agent-provider/src/mock.rs` — `with_usage()` builder
- `crates/gitim-agent-provider/src/claude.rs` — parse `result.usage`
- `crates/gitim-agent-provider/src/codex.rs` — capture `token_count.used_percent`
- `crates/gitim-runtime/Cargo.toml` — add `tiktoken-rs`
- `crates/gitim-runtime/src/lib.rs` — export `context_window` module
- `crates/gitim-runtime/src/state.rs` — extend `AgentState`, add `SessionUsageSnapshot`, `UsageSource`
- `crates/gitim-runtime/src/agent_loop.rs` — snapshot computation, crossing detection, injection, logging
- `crates/gitim-runtime/src/http.rs` — `AgentInfo.session_usage`, recovery, SSE emit
- `crates/gitim-runtime/tests/agent_loop.rs` — pure-function unit tests

### Modified files — WebUI
- `webui-v2/src/lib/types.ts` — `Agent.sessionUsage`
- `webui-v2/src/lib/client.ts` — `mapBackendAgent` maps `session_usage`
- `webui-v2/src/hooks/use-agent-activity.ts` — filter `"usage"` SSE events → `useAgentStore.updateAgent`
- `webui-v2/src/components/chat/agent-status-panel.tsx` — popover header-right badge

---

## Phase 1 — Provider Usage Extraction

### Task 1: Add `ProviderUsage` type and extend `ExecResult`

**Files:**
- Modify: `crates/gitim-agent-provider/src/types.rs`

- [ ] **Step 1: Add `ProviderUsage` struct**

In `crates/gitim-agent-provider/src/types.rs`, after the `ExecStatus` enum (around line 121), add:

```rust
/// Per-turn usage as reported by a provider.
///
/// Providers fill different subsets:
/// - Claude populates `input_tokens` / `output_tokens`; `used_percent` is `None`.
/// - Codex populates `used_percent`; token counts are `None`.
/// - Mock fills whatever the test configures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub used_percent: Option<f64>,
}
```

Then extend `ExecResult` at `crates/gitim-agent-provider/src/types.rs:99-112` by adding a final field:

```rust
pub struct ExecResult {
    pub status: ExecStatus,
    pub output: String,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub session_token: Option<String>,
    pub usage: Option<ProviderUsage>,   // NEW
}
```

- [ ] **Step 2: Re-export `ProviderUsage` from crate root**

In `crates/gitim-agent-provider/src/lib.rs`, add to the `pub use` block for `types`:

```rust
pub use types::{
    // ... existing exports ...
    ProviderUsage,
};
```

- [ ] **Step 3: Fix all constructors of `ExecResult`**

Add `usage: None` to every existing `ExecResult { ... }` literal. Run:

```bash
cargo check -p gitim-agent-provider 2>&1 | grep "missing field" | head -20
```

Expected sites:
- `crates/gitim-agent-provider/src/claude.rs` — `drive_session` result build (near line 278)
- `crates/gitim-agent-provider/src/codex.rs` — result builds (two sites: around lines 215–225 and early-exit around line 90)
- `crates/gitim-agent-provider/src/mock.rs` — `execute` result send
- Any other provider stubs (`gemini.rs`, `hermes.rs`, `openclaw.rs`, `opencode.rs`, `stubs.rs`)

For each site, insert `usage: None,` as the last field.

- [ ] **Step 4: Verify build**

Run:

```bash
cargo build -p gitim-agent-provider
```

Expected: clean build, no warnings about missing fields or unused imports.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-agent-provider/src/types.rs crates/gitim-agent-provider/src/lib.rs crates/gitim-agent-provider/src/*.rs
git commit -m "feat(provider): add ProviderUsage type and ExecResult.usage field

Defines the shape of per-turn usage data. All existing ExecResult
construction sites default to usage: None; subsequent commits wire
each provider's actual data in."
```

---

### Task 2: Mock provider synthetic usage

**Files:**
- Modify: `crates/gitim-agent-provider/src/mock.rs`
- Test: `crates/gitim-agent-provider/tests/factory_test.rs` (if existing) or inline `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/gitim-agent-provider/src/mock.rs`, add:

```rust
#[cfg(test)]
mod usage_tests {
    use super::*;

    #[tokio::test]
    async fn mock_provider_emits_configured_usage() {
        let provider = MockProvider::with_response("ok".to_string())
            .with_usage(ProviderUsage {
                input_tokens: Some(42_000),
                output_tokens: Some(800),
                used_percent: None,
            });

        let session = provider
            .execute("hi", ExecOptions::default())
            .await
            .expect("execute");

        // Drain events so the result channel fires.
        let mut events = session.events;
        while events.recv().await.is_some() {}

        let result = session.result.await.expect("result");
        assert_eq!(
            result.usage,
            Some(ProviderUsage {
                input_tokens: Some(42_000),
                output_tokens: Some(800),
                used_percent: None,
            })
        );
    }

    #[tokio::test]
    async fn mock_provider_default_usage_is_none() {
        let provider = MockProvider::with_response("ok".to_string());
        let session = provider
            .execute("hi", ExecOptions::default())
            .await
            .expect("execute");

        let mut events = session.events;
        while events.recv().await.is_some() {}

        let result = session.result.await.expect("result");
        assert!(result.usage.is_none());
    }
}
```

Import at the top of the file if not already present:

```rust
use crate::ProviderUsage;
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-agent-provider mock::usage_tests -- --nocapture
```

Expected: FAIL (`no method named with_usage`, and later an assertion failure).

- [ ] **Step 3: Implement `with_usage()` and wire it into execute**

Extend `MockProvider` struct and its methods. Near the top of `crates/gitim-agent-provider/src/mock.rs`:

```rust
pub struct MockProvider {
    #[allow(dead_code)]
    config: ProviderConfig,
    default_response: String,
    usage: Option<ProviderUsage>,   // NEW
}

impl MockProvider {
    pub fn new(_config: ProviderConfig) -> Self {
        Self {
            config: _config,
            default_response: "mock-response: acknowledged".to_string(),
            usage: None,
        }
    }

    pub fn with_response(response: String) -> Self {
        Self {
            config: ProviderConfig::default(),
            default_response: response,
            usage: None,
        }
    }

    pub fn with_usage(mut self, usage: ProviderUsage) -> Self {
        self.usage = Some(usage);
        self
    }
}
```

Then in the `execute` body, replace the existing `usage: None` line (added in Task 1) with:

```rust
usage: self.usage.clone(),
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p gitim-agent-provider mock::usage_tests
```

Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-agent-provider/src/mock.rs
git commit -m "feat(provider): mock provider supports synthetic ProviderUsage

Tests for runtime's usage pipeline need a way to simulate provider-
reported usage without invoking a real CLI. with_usage(builder) lets
tests dial any combination of input_tokens / output_tokens / used_percent."
```

---

### Task 3: Claude provider parses `result.usage`

**Files:**
- Modify: `crates/gitim-agent-provider/src/claude.rs`
- Test: add `#[cfg(test)] mod usage_parse_tests` at the bottom of the file

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/gitim-agent-provider/src/claude.rs`, add:

```rust
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
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-agent-provider --lib usage_parse_tests
```

Expected: FAIL (compile error — `ParsedMessage::Result` has no `usage` field).

- [ ] **Step 3: Add usage to `RawMessage` and `ParsedMessage::Result`**

In `crates/gitim-agent-provider/src/claude.rs`:

1. Add a new deserialize helper struct near the other SDK types (near line 490):

```rust
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
```

2. Extend `RawMessage` (around line 447) with a `usage` field:

```rust
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
    usage: Option<ClaudeUsage>,   // NEW
    // ... other existing fields stay as-is
```

3. Extend `ParsedMessage::Result` (around line 325):

```rust
pub enum ParsedMessage {
    // ... other variants
    Result {
        session_id: String,
        output: String,
        is_error: bool,
        usage: Option<ProviderUsage>,   // NEW
    },
    // ...
}
```

4. In `parse_line` (around line 367), map the usage:

```rust
"result" => Some(ParsedMessage::Result {
    session_id: raw.session_id.unwrap_or_default(),
    output: raw.result.unwrap_or_default(),
    is_error: raw.is_error.unwrap_or(false),
    usage: raw.usage.map(|u| ProviderUsage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        used_percent: None,
    }),
}),
```

Make sure to `use crate::ProviderUsage;` at the top of the file if not already imported.

- [ ] **Step 4: Update `drive_session` to capture usage into `ExecResult`**

In `crates/gitim-agent-provider/src/claude.rs`, inside `drive_session`:

1. Declare a mutable holder near the top of the function (before the select loop, around line 117):

```rust
let mut captured_usage: Option<ProviderUsage> = None;
```

2. In the match arm that handles `ParsedMessage::Result` (around line 182), destructure the `usage` field and assign:

```rust
ParsedMessage::Result {
    output: result_text,
    session_id: sid,
    is_error,
    usage: result_usage,   // NEW
} => {
    // ... existing code ...
    captured_usage = result_usage;
    // ... rest of existing code for setting saw_result, final_status, etc.
}
```

3. At the final `result_tx.send(ExecResult { ... })` (around line 278), replace `usage: None,` with:

```rust
usage: captured_usage,
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p gitim-agent-provider --lib usage_parse_tests
cargo test -p gitim-agent-provider
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-agent-provider/src/claude.rs
git commit -m "feat(provider): claude parses result.usage into ExecResult

Claude CLI's final stream-json 'result' message carries a usage block
with input_tokens / output_tokens / cache_* counts. parse_line now
surfaces these; drive_session captures them into ExecResult.usage."
```

---

### Task 4: Codex provider captures `token_count` events

**Files:**
- Modify: `crates/gitim-agent-provider/src/codex.rs`

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/gitim-agent-provider/src/codex.rs`, add:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-agent-provider --lib codex::usage_parse_tests
```

Expected: FAIL (`parse_used_percent` undefined).

- [ ] **Step 3: Implement `parse_used_percent` helper**

Near the other line-parsing helpers in `crates/gitim-agent-provider/src/codex.rs` (alongside `parse_credits_exhausted`, around line 393), add:

```rust
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
```

- [ ] **Step 4: Wire it into `drive_session`**

In `crates/gitim-agent-provider/src/codex.rs`, within `drive_session`:

1. Declare a mutable holder near the top (before the event loop):

```rust
let mut latest_used_percent: Option<f64> = None;
```

2. In the per-line handler (where `parse_rollout_line` or similar is called on stdout lines — the stream-reading loop around lines 150–200), add a pass to update the holder:

```rust
if let Some(pct) = parse_used_percent(&line) {
    latest_used_percent = Some(pct);
}
```

(This runs alongside, not replacing, existing parsing.)

3. At the `ExecResult` build site (around lines 215–225 and the early-exit around line 90), replace `usage: None,` with:

```rust
usage: latest_used_percent.map(|p| ProviderUsage {
    input_tokens: None,
    output_tokens: None,
    used_percent: Some(p),
}),
```

For the early-exit site, `latest_used_percent` is not in scope — set `usage: None,` there (early exits are pre-stream).

Add `use crate::ProviderUsage;` at the top if needed.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p gitim-agent-provider
```

Expected: all green, including the three new unit tests.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-agent-provider/src/codex.rs
git commit -m "feat(provider): codex captures token_count.used_percent into ExecResult

Every few turns codex emits event_msg/token_count with rate_limits.
primary.used_percent — a direct 0-100 percentage. drive_session tracks
the latest value across the stream and packs it into ExecResult.usage."
```

---

## Phase 2 — Runtime State + context_window Module

### Task 5: `SessionUsageSnapshot`, `UsageSource`, extend `AgentState`

**Files:**
- Modify: `crates/gitim-runtime/src/state.rs`

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/gitim-runtime/src/state.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn state_roundtrips_new_fields() {
        let dir = TempDir::new().expect("tempdir");
        let gitim_dir = dir.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).expect("mkdir");

        let original = AgentState {
            cursor: Some("c1".into()),
            session_token: Some("sess-abc".into()),
            session_usage: Some(SessionUsageSnapshot {
                session_id: "sess-abc".into(),
                input_tokens: Some(128_000),
                output_tokens: Some(512),
                max_tokens: Some(200_000),
                used_percent: 64.0,
                source: UsageSource::ProviderReported,
                updated_at: "2026-04-20T12:00:00Z".into(),
            }),
            estimated_tokens: 125_400,
            usage_notice_pending: false,
        };

        original.save(dir.path()).expect("save");
        let loaded = AgentState::load(dir.path()).expect("load");
        assert_eq!(loaded.session_token, original.session_token);
        let snap = loaded.session_usage.expect("snapshot present");
        assert_eq!(snap.session_id, "sess-abc");
        assert_eq!(snap.used_percent, 64.0);
        assert!(matches!(snap.source, UsageSource::ProviderReported));
        assert_eq!(loaded.estimated_tokens, 125_400);
    }

    #[test]
    fn legacy_state_without_new_fields_loads() {
        let dir = TempDir::new().expect("tempdir");
        let gitim_dir = dir.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).expect("mkdir");
        let legacy = r#"{"cursor":"old","session_token":"sess-old"}"#;
        std::fs::write(gitim_dir.join("agent-state.json"), legacy).expect("write");

        let state = AgentState::load(dir.path()).expect("load");
        assert_eq!(state.cursor.as_deref(), Some("old"));
        assert!(state.session_usage.is_none());
        assert_eq!(state.estimated_tokens, 0);
        assert!(!state.usage_notice_pending);
    }
}
```

Add `tempfile` to `[dev-dependencies]` in `crates/gitim-runtime/Cargo.toml` if not already present:

```bash
grep -q "^tempfile" crates/gitim-runtime/Cargo.toml || \
  cargo add --dev tempfile -p gitim-runtime
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-runtime --lib state::tests
```

Expected: FAIL (types `SessionUsageSnapshot` / `UsageSource` not found; `AgentState` missing fields).

- [ ] **Step 3: Add the new types and extend `AgentState`**

Replace the contents of `crates/gitim-runtime/src/state.rs`:

```rust
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    ProviderReported,
    RuntimeEstimated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUsageSnapshot {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    pub used_percent: f64,
    pub source: UsageSource,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<SessionUsageSnapshot>,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub estimated_tokens: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub usage_notice_pending: bool,
}

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

impl AgentState {
    pub fn state_path(repo_root: &Path) -> PathBuf {
        repo_root.join(".gitim/agent-state.json")
    }

    pub fn load(repo_root: &Path) -> Result<Self, RuntimeError> {
        let path = Self::state_path(repo_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)
            .map_err(|e| RuntimeError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
    }

    pub fn save(&self, repo_root: &Path) -> Result<(), RuntimeError> {
        let path = Self::state_path(repo_root);
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| RuntimeError::Io(std::io::Error::other(e)))?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Clear all fields tied to the current provider session. Called on
    /// `[[RESET]]` detection and on session failure.
    pub fn clear_session(&mut self) {
        self.session_token = None;
        self.session_usage = None;
        self.estimated_tokens = 0;
        self.usage_notice_pending = false;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p gitim-runtime --lib state::tests
```

Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/state.rs crates/gitim-runtime/Cargo.toml
git commit -m "feat(runtime): extend AgentState with session_usage + tiktoken estimate

SessionUsageSnapshot captures per-session usage info (input_tokens,
used_percent, source). estimated_tokens holds the runtime tiktoken
running sum. usage_notice_pending is the one-shot 80%-crossing flag.
clear_session() resets all four fields atomically on [[RESET]].
Legacy state files without the new fields load cleanly via
serde(default)."
```

---

### Task 6: `context_window` module — constants + default max table

**Files:**
- Create: `crates/gitim-runtime/src/context_window.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/gitim-runtime/src/context_window.rs`:

```rust
#[cfg(test)]
mod default_max_tests {
    use super::*;

    #[test]
    fn claude_sonnet_defaults_to_200k() {
        assert_eq!(default_max_tokens("claude", "claude-sonnet-4-6"), Some(200_000));
    }

    #[test]
    fn claude_opus_1m_variant_defaults_to_1m() {
        assert_eq!(default_max_tokens("claude", "claude-opus-4-7[1m]"), Some(1_000_000));
    }

    #[test]
    fn claude_opus_default_is_200k() {
        assert_eq!(default_max_tokens("claude", "claude-opus-4-7"), Some(200_000));
    }

    #[test]
    fn codex_returns_none() {
        assert_eq!(default_max_tokens("codex", "gpt-5"), None);
    }

    #[test]
    fn mock_returns_10k() {
        assert_eq!(default_max_tokens("mock", "any"), Some(10_000));
    }

    #[test]
    fn unknown_provider_conservative_fallback() {
        assert_eq!(default_max_tokens("future", "some-model"), Some(200_000));
    }
}
```

Also add the top of the file:

```rust
//! Context window tracking: per-provider default budgets and tokenizer helpers.

pub const WARN_AT_PERCENT: f64 = 80.0;

/// Default max-context-tokens for the given provider/model pair.
///
/// Returns `None` when the provider reports `used_percent` directly and no
/// token count is meaningful at the runtime layer (currently: Codex).
pub fn default_max_tokens(provider: &str, model: &str) -> Option<u64> {
    match provider {
        "claude" => {
            if model.contains("opus-4-7") && model.contains("1m") {
                Some(1_000_000)
            } else {
                Some(200_000)
            }
        }
        "codex" => None,
        "mock" => Some(10_000),
        _ => Some(200_000),
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/gitim-runtime/src/lib.rs`, add:

```rust
pub mod context_window;
```

next to the other `pub mod` lines.

- [ ] **Step 3: Run test to verify it fails, then passes**

```bash
cargo test -p gitim-runtime --lib context_window::default_max_tests
```

Expected: PASS (all 6 tests). The test and impl were added together because they're tiny and inseparable.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/context_window.rs crates/gitim-runtime/src/lib.rs
git commit -m "feat(runtime): add context_window module with default max tokens

Hardcoded per-provider/model default max_tokens table. WARN_AT_PERCENT
is a single 80% constant — no multi-tier thresholds. User-configurable
override is explicitly out of scope for v1 (see 01-design.md §4.4)."
```

---

### Task 7: Tokenizer helpers in `context_window`

**Files:**
- Modify: `crates/gitim-runtime/Cargo.toml`
- Modify: `crates/gitim-runtime/src/context_window.rs`

- [ ] **Step 1: Add `tiktoken-rs` dependency**

```bash
cargo add tiktoken-rs -p gitim-runtime
```

Verify `crates/gitim-runtime/Cargo.toml` now shows:
```
tiktoken-rs = "0.x"
```

- [ ] **Step 2: Write the failing test**

Append to `crates/gitim-runtime/src/context_window.rs`:

```rust
#[cfg(test)]
mod tokenize_tests {
    use super::*;

    #[test]
    fn tokenize_claude_short_text() {
        let n = tokenize_for_provider("claude", "hello world");
        assert!(n > 0 && n < 20, "got {n}");
    }

    #[test]
    fn tokenize_codex_short_text() {
        let n = tokenize_for_provider("codex", "hello world");
        assert!(n > 0 && n < 20, "got {n}");
    }

    #[test]
    fn tokenize_empty_returns_zero() {
        assert_eq!(tokenize_for_provider("claude", ""), 0);
    }

    #[test]
    fn tokenize_same_text_is_stable() {
        let a = tokenize_for_provider("claude", "repeatable input");
        let b = tokenize_for_provider("claude", "repeatable input");
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p gitim-runtime --lib context_window::tokenize_tests
```

Expected: FAIL (`tokenize_for_provider` undefined).

- [ ] **Step 4: Implement tokenizer helpers**

Append to `crates/gitim-runtime/src/context_window.rs` (below `default_max_tokens`):

```rust
use std::sync::OnceLock;
use tiktoken_rs::{cl100k_base, o200k_base, CoreBPE};

static CL100K: OnceLock<CoreBPE> = OnceLock::new();
static O200K: OnceLock<CoreBPE> = OnceLock::new();

fn cl100k() -> Option<&'static CoreBPE> {
    match CL100K.get() {
        Some(b) => Some(b),
        None => cl100k_base().ok().map(|b| CL100K.get_or_init(|| b)),
    }
}

fn o200k() -> Option<&'static CoreBPE> {
    match O200K.get() {
        Some(b) => Some(b),
        None => o200k_base().ok().map(|b| O200K.get_or_init(|| b)),
    }
}

/// Count tokens in `text` using the encoder best suited for the given provider.
///
/// Returns 0 if the encoder fails to initialize (logged once by the caller) or
/// if the text is empty. Always succeeds for non-empty inputs once the encoder
/// is warm.
pub fn tokenize_for_provider(provider: &str, text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let bpe = match provider {
        "codex" => o200k(),
        _ => cl100k(),
    };
    match bpe {
        Some(b) => b.encode_with_special_tokens(text).len() as u64,
        None => 0,
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p gitim-runtime --lib context_window::tokenize_tests
```

Expected: all 4 tests PASS. First run will be slow (tiktoken BPE loads on cold start); subsequent runs are instant.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/Cargo.toml crates/gitim-runtime/src/context_window.rs
git commit -m "feat(runtime): tokenize_for_provider via tiktoken-rs

cl100k_base for Claude (closest open reference), o200k_base for Codex.
Process-wide OnceLock cache — first call pays ~100ms for BPE load,
subsequent calls are O(n) over the input. Returns 0 on empty input or
if the encoder fails to initialize; production callers log at warn
once and skip the estimator path (see later tasks)."
```

---

## Phase 3 — Agent Loop Pure Functions

### Task 8: `compute_snapshot()` pure helper

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Test: `crates/gitim-runtime/tests/agent_loop.rs`

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/gitim-runtime/tests/agent_loop.rs`, add:

```rust
use gitim_agent_provider::ProviderUsage;
use gitim_runtime::agent_loop::compute_snapshot;
use gitim_runtime::state::UsageSource;

#[test]
fn snapshot_from_claude_provider_reported() {
    let snap = compute_snapshot(
        "sess-abc",
        Some(&ProviderUsage { input_tokens: Some(160_000), output_tokens: Some(500), used_percent: None }),
        42_000,
        Some(200_000),
        "2026-04-20T10:00:00Z",
    ).expect("snapshot");

    assert_eq!(snap.session_id, "sess-abc");
    assert_eq!(snap.input_tokens, Some(160_000));
    assert!((snap.used_percent - 80.0).abs() < 0.01);
    assert!(matches!(snap.source, UsageSource::ProviderReported));
}

#[test]
fn snapshot_from_codex_used_percent() {
    let snap = compute_snapshot(
        "sess-xyz",
        Some(&ProviderUsage { input_tokens: None, output_tokens: None, used_percent: Some(47.5) }),
        0,
        None,
        "2026-04-20T10:00:00Z",
    ).expect("snapshot");

    assert!((snap.used_percent - 47.5).abs() < 0.01);
    assert!(matches!(snap.source, UsageSource::ProviderReported));
    assert!(snap.max_tokens.is_none());
}

#[test]
fn snapshot_falls_back_to_estimator() {
    let snap = compute_snapshot(
        "sess-fut",
        None,
        80_000,
        Some(100_000),
        "2026-04-20T10:00:00Z",
    ).expect("snapshot");

    assert!((snap.used_percent - 80.0).abs() < 0.01);
    assert!(matches!(snap.source, UsageSource::RuntimeEstimated));
}

#[test]
fn snapshot_returns_none_when_no_data_available() {
    let snap = compute_snapshot("sess", None, 0, None, "2026-04-20T10:00:00Z");
    assert!(snap.is_none());
}

#[test]
fn snapshot_clamps_above_100_with_warning_signal() {
    let snap = compute_snapshot(
        "sess",
        Some(&ProviderUsage { input_tokens: None, output_tokens: None, used_percent: Some(115.0) }),
        0,
        None,
        "2026-04-20T10:00:00Z",
    ).expect("snapshot");
    // The pure function doesn't log — it just clamps to 100.
    assert!((snap.used_percent - 100.0).abs() < 0.01);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-runtime --test agent_loop snapshot_
```

Expected: FAIL (`compute_snapshot` undefined).

- [ ] **Step 3: Implement `compute_snapshot`**

At a new section near the bottom of `crates/gitim-runtime/src/agent_loop.rs` (before any `#[cfg(test)]`), add:

```rust
use gitim_agent_provider::ProviderUsage;
use crate::state::{SessionUsageSnapshot, UsageSource};

/// Compute a `SessionUsageSnapshot` from available usage signals.
///
/// Authoritative-value policy (matches 01-design.md §4.5):
/// 1. provider_reported.used_percent (Codex)
/// 2. provider_reported.input_tokens / max_tokens (Claude)
/// 3. estimated_tokens / max_tokens (fallback)
/// 4. None (no data available)
///
/// `used_percent` is clamped to `[0, 100]`. Callers are responsible for
/// logging unusual values (e.g. >110 as a protocol-drift signal).
pub fn compute_snapshot(
    session_id: &str,
    provider_reported: Option<&ProviderUsage>,
    estimated_tokens: u64,
    max_tokens: Option<u64>,
    updated_at: &str,
) -> Option<SessionUsageSnapshot> {
    let (used_percent, source, input_tokens, output_tokens) =
        if let Some(pu) = provider_reported {
            if let Some(pct) = pu.used_percent {
                (pct, UsageSource::ProviderReported, pu.input_tokens, pu.output_tokens)
            } else if let (Some(input), Some(max)) = (pu.input_tokens, max_tokens) {
                let pct = (input as f64) / (max as f64) * 100.0;
                (pct, UsageSource::ProviderReported, pu.input_tokens, pu.output_tokens)
            } else {
                return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
            }
        } else {
            return compute_from_estimate(session_id, estimated_tokens, max_tokens, updated_at);
        };

    let used_percent = used_percent.clamp(0.0, 100.0);
    Some(SessionUsageSnapshot {
        session_id: session_id.to_string(),
        input_tokens,
        output_tokens,
        max_tokens,
        used_percent,
        source,
        updated_at: updated_at.to_string(),
    })
}

fn compute_from_estimate(
    session_id: &str,
    estimated_tokens: u64,
    max_tokens: Option<u64>,
    updated_at: &str,
) -> Option<SessionUsageSnapshot> {
    let max = max_tokens?;
    if estimated_tokens == 0 {
        return None;
    }
    let pct = ((estimated_tokens as f64) / (max as f64) * 100.0).clamp(0.0, 100.0);
    Some(SessionUsageSnapshot {
        session_id: session_id.to_string(),
        input_tokens: None,
        output_tokens: None,
        max_tokens: Some(max),
        used_percent: pct,
        source: UsageSource::RuntimeEstimated,
        updated_at: updated_at.to_string(),
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p gitim-runtime --test agent_loop snapshot_
```

Expected: all 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/tests/agent_loop.rs
git commit -m "feat(runtime): compute_snapshot pure function

Totally pure — no I/O, no time, no tracing. Takes provider data +
estimator + max and emits a SessionUsageSnapshot or None. Implements
the four-rung authoritative-value policy from 01-design.md §4.5."
```

---

### Task 9: `just_crossed_threshold()` pure helper

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Test: `crates/gitim-runtime/tests/agent_loop.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/gitim-runtime/tests/agent_loop.rs`:

```rust
use gitim_runtime::agent_loop::just_crossed_threshold;
use gitim_runtime::context_window::WARN_AT_PERCENT;

#[test]
fn crossed_on_first_observation_above_threshold() {
    assert!(just_crossed_threshold(None, 85.0));
}

#[test]
fn not_crossed_below_threshold() {
    assert!(!just_crossed_threshold(Some(45.0), 62.0));
    assert!(!just_crossed_threshold(None, 30.0));
}

#[test]
fn crossed_when_previous_below_and_new_above() {
    assert!(just_crossed_threshold(Some(78.0), 82.0));
    assert!(just_crossed_threshold(Some(79.99), WARN_AT_PERCENT));
}

#[test]
fn not_crossed_when_already_above() {
    assert!(!just_crossed_threshold(Some(82.0), 90.0));
    assert!(!just_crossed_threshold(Some(WARN_AT_PERCENT), 95.0));
}

#[test]
fn not_crossed_when_dropping() {
    // Shouldn't happen in practice, but the function must be total.
    assert!(!just_crossed_threshold(Some(90.0), 40.0));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-runtime --test agent_loop crossed_ not_crossed_
```

Expected: FAIL (`just_crossed_threshold` undefined).

- [ ] **Step 3: Implement the function**

Append to `crates/gitim-runtime/src/agent_loop.rs`:

```rust
use crate::context_window::WARN_AT_PERCENT;

/// `true` iff this turn is the first to observe `new_pct >= WARN_AT_PERCENT`
/// in the current session. Never returns `true` twice for the same session
/// (subsequent turns see `prev_pct >= WARN_AT_PERCENT`).
pub fn just_crossed_threshold(prev_pct: Option<f64>, new_pct: f64) -> bool {
    if new_pct < WARN_AT_PERCENT {
        return false;
    }
    match prev_pct {
        None => true,
        Some(p) => p < WARN_AT_PERCENT,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p gitim-runtime --test agent_loop crossed_ not_crossed_
```

Expected: all 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/tests/agent_loop.rs
git commit -m "feat(runtime): just_crossed_threshold pure function

One-shot crossing detection. Edge case: previous None counts as below
threshold so first observation above 80 fires once. Monotonic above
threshold — no re-firing as percent climbs further."
```

---

### Task 10: `build_usage_notice_preamble()` helper

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Test: `crates/gitim-runtime/tests/agent_loop.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/gitim-runtime/tests/agent_loop.rs`:

```rust
use gitim_runtime::agent_loop::build_usage_notice_preamble;

#[test]
fn preamble_contains_percentage() {
    let p = build_usage_notice_preamble(82.4);
    assert!(p.contains("82"), "preamble: {p}");
}

#[test]
fn preamble_mentions_reset_marker() {
    let p = build_usage_notice_preamble(85.0);
    assert!(p.contains("[[RESET]]"));
}

#[test]
fn preamble_marks_as_system_notice() {
    let p = build_usage_notice_preamble(85.0);
    assert!(p.starts_with("[系统通知]"));
}

#[test]
fn preamble_says_only_once() {
    let p = build_usage_notice_preamble(85.0);
    assert!(p.contains("仅发送一次"));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p gitim-runtime --test agent_loop preamble_
```

Expected: FAIL (`build_usage_notice_preamble` undefined).

- [ ] **Step 3: Implement the helper**

Append to `crates/gitim-runtime/src/agent_loop.rs`:

```rust
/// The one-shot preamble inserted before the next user prompt when
/// `used_percent` first crosses `WARN_AT_PERCENT`. Content is deliberately
/// firm — no tiered wording, no retry guidance — per 01-design.md §4.5.
pub fn build_usage_notice_preamble(used_percent: f64) -> String {
    format!(
        "[系统通知] 你的对话窗口使用率已达 {pct:.0}%。请在本轮完成手头任务后立即结束本次对话：\n\
         1. 把所有需要长期记忆的内容（重要决定、用户偏好、未完成事项、进度交接）写入你的记忆文件\n\
         2. 在输出末尾附加标记 [[RESET]]，runtime 会在下一轮为你开启全新窗口\n\
         本提醒仅发送一次。",
        pct = used_percent,
    )
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p gitim-runtime --test agent_loop preamble_
```

Expected: all 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/tests/agent_loop.rs
git commit -m "feat(runtime): build_usage_notice_preamble helper

The one-shot Chinese preamble prepended to the next user prompt when
80% is crossed. Instructs the agent to checkpoint memory and emit
[[RESET]]. Content is locked here — tests assert the presence of key
phrases so future edits can't silently weaken the instruction."
```

---

## Phase 4 — Agent Loop Wiring

### Task 11: Wire provider-reported usage into snapshot + state

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Add an `AgentLoop` method for post-turn usage update**

In `crates/gitim-runtime/src/agent_loop.rs`, near the other `impl AgentLoop` methods, add:

```rust
/// After `provider.execute()` returns, update state.session_usage based on
/// whatever the provider reported plus the current estimator. Persists state.
fn update_session_usage(
    &self,
    state: &mut AgentState,
    provider_reported: Option<&ProviderUsage>,
    session_id: &str,
) -> Result<(), RuntimeError> {
    let max = crate::context_window::default_max_tokens(&self.provider_type, &self.model);
    let now = chrono::Utc::now().to_rfc3339();
    let new_snapshot = compute_snapshot(
        session_id,
        provider_reported,
        state.estimated_tokens,
        max,
        &now,
    );

    let prev_pct = state.session_usage.as_ref().map(|s| s.used_percent);
    if let Some(snap) = &new_snapshot {
        if just_crossed_threshold(prev_pct, snap.used_percent) {
            state.usage_notice_pending = true;
            let est_pct = max
                .map(|m| (state.estimated_tokens as f64) / (m as f64) * 100.0)
                .unwrap_or(0.0);
            tracing::info!(
                session_id = %session_id,
                provider_input_tokens = ?provider_reported.and_then(|p| p.input_tokens),
                provider_used_pct = snap.used_percent,
                estimated_tokens = state.estimated_tokens,
                estimated_used_pct = est_pct,
                delta_pp = snap.used_percent - est_pct,
                max_tokens = ?max,
                provider = %self.provider_type,
                model = %self.model,
                "threshold_crossed_80pct"
            );
        }
        if snap.used_percent > 110.0 {
            tracing::warn!(session_id = %session_id, used_percent = snap.used_percent, "provider reported >110% — protocol drift signal");
        }
    }

    state.session_usage = new_snapshot;
    state.save(&self.repo_root)?;
    Ok(())
}
```

Ensure `self.provider_type`, `self.model`, and `self.repo_root` are already accessible on `AgentLoop` — if any field isn't, it should be pulled from the closest existing field (e.g. `self.config.provider` from `AgentLoopConfig`). Check `crates/gitim-runtime/src/agent_loop.rs:20-35` for the struct layout.

- [ ] **Step 2: Call it from `run_once` after the provider returns**

The existing `run_once` has TWO branches that keep the session_token live (both retain partial-or-full output for which usage was reported):

- **Success** (Completed, around `crates/gitim-runtime/src/agent_loop.rs:336-338`)
- **Steered** (Aborted via cancel, around line 347-349)

Both should update usage. Just before each `self.session_token = Some(token)` assignment, insert:

```rust
// Extract session_id from the just-completed turn. For Claude the
// session_token and session_id are the same opaque string; for Codex
// it's the thread_id. In either case it's exec_result.session_token.
if let Some(sid) = exec_result.session_token.as_deref() {
    let mut state = AgentState::load(&self.repo_root)?;
    self.update_session_usage(&mut state, exec_result.usage.as_ref(), sid)?;
}
```

Do NOT add this on the Failed branch — Task 15 clears session state there instead.

- [ ] **Step 3: Run full test suite**

```bash
cargo test -p gitim-runtime
```

Expected: all existing tests + Task 5/8/9/10 tests PASS. No new integration test here yet — this wiring is covered by Task 21 E2E.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): wire provider usage into session snapshot after each turn

update_session_usage() computes the snapshot via the pure helper,
detects 80% crossing (sets usage_notice_pending + logs comparison
line), and persists state. Called from run_once's success branch
only — aborted/failed turns don't poison the snapshot."
```

---

### Task 12: Tiktoken estimator accumulation

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Add pre-execute accumulation**

In `run_once`, just after `prompt` is finalized and before `provider.execute()` (around `crates/gitim-runtime/src/agent_loop.rs:215-220`):

```rust
let cold_start = self.session_token.is_none();
let mut state = AgentState::load(&self.repo_root)?;
if cold_start {
    state.estimated_tokens = 0;
}
state.estimated_tokens += crate::context_window::tokenize_for_provider(&self.provider_type, &prompt);
if cold_start {
    if let Some(sp) = opts.system_prompt.as_deref() {
        state.estimated_tokens += crate::context_window::tokenize_for_provider(&self.provider_type, sp);
    }
}
state.save(&self.repo_root)?;
```

- [ ] **Step 2: Accumulate assistant text during the drain loop**

Find the existing event-drain loop (around `crates/gitim-runtime/src/agent_loop.rs:232-280`). Add a buffer declaration near `text_tail`:

```rust
let mut assistant_text_buf = String::new();
```

In the match arm for `Event::Text { content }`, after the existing `text_tail.push_str(content)`, add:

```rust
assistant_text_buf.push_str(content);
```

- [ ] **Step 3: Add assistant tokens on successful completion**

In the `Ok(exec_result)` branch after `update_session_usage` was placed in Task 11 — reorder so the estimator update happens **before** snapshot computation (so the snapshot reflects tokens including the just-finished turn's assistant output):

Replace the Task 11 block with:

```rust
if let Some(sid) = exec_result.session_token.as_deref() {
    let mut state = AgentState::load(&self.repo_root)?;
    state.estimated_tokens += crate::context_window::tokenize_for_provider(&self.provider_type, &assistant_text_buf);
    self.update_session_usage(&mut state, exec_result.usage.as_ref(), sid)?;
}
```

`update_session_usage` already saves state internally — no separate save needed.

- [ ] **Step 4: Verify compile**

```bash
cargo build -p gitim-runtime
```

Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): accumulate tiktoken estimate across turn

Cold start resets estimated_tokens to 0 and seeds with system prompt +
user prompt. Every turn adds the assistant text (collected during
event drain) before snapshot computation. Used solely as an observable
shadow value today; logged in delta_pp against provider-reported for
offline validation."
```

---

### Task 13: Per-turn debug logging

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Add the turn_usage log inside `update_session_usage`**

In `update_session_usage` (Task 11), at the bottom of the function — before `state.session_usage = new_snapshot;` — add:

```rust
if let Some(snap) = &new_snapshot {
    let est_pct = max
        .map(|m| (state.estimated_tokens as f64) / (m as f64) * 100.0)
        .unwrap_or(0.0);
    tracing::debug!(
        session_id = %session_id,
        provider_pct = snap.used_percent,
        estimated_pct = est_pct,
        delta_pp = snap.used_percent - est_pct,
        source = ?snap.source,
        "turn_usage"
    );
}
```

- [ ] **Step 2: Verify**

```bash
cargo build -p gitim-runtime
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): per-turn debug log of usage + estimator delta

Emits on every successful turn, independent of threshold crossing.
Gives us the dataset to characterize cl100k/o200k accuracy against
provider-reported numbers before trusting the estimator as a driver."
```

---

### Task 14: Inject notice on next turn

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Prepend the notice when `usage_notice_pending` is set**

In `run_once`, right after `prompt` is assembled from `format_changes_as_prompt()` (around line 204) and **before** the estimator pre-execute accumulation added in Task 12:

```rust
let mut state = AgentState::load(&self.repo_root)?;
let prompt = if state.usage_notice_pending {
    let pct = state.session_usage.as_ref().map(|s| s.used_percent).unwrap_or(80.0);
    let preamble = build_usage_notice_preamble(pct);
    state.usage_notice_pending = false;
    state.save(&self.repo_root)?;
    format!("{preamble}\n\n---\n\n{prompt}")
} else {
    prompt
};
```

Since Task 12 also does `AgentState::load` right after this, consolidate: keep the single `load` + save here, and have Task 12's additions extend this same `state` variable rather than reload. Adjust Task 12's code to remove the second load:

```rust
let cold_start = self.session_token.is_none();
if cold_start {
    state.estimated_tokens = 0;
}
state.estimated_tokens += crate::context_window::tokenize_for_provider(&self.provider_type, &prompt);
if cold_start {
    if let Some(sp) = opts.system_prompt.as_deref() {
        state.estimated_tokens += crate::context_window::tokenize_for_provider(&self.provider_type, sp);
    }
}
state.save(&self.repo_root)?;
```

- [ ] **Step 2: Verify build**

```bash
cargo build -p gitim-runtime
```

Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): inject 80% notice as user-prompt preamble on next turn

usage_notice_pending flag (set in Task 11 when crossing detected) is
consumed here — prompt gets the system notice prepended, flag is
cleared, state saved before provider.execute so a mid-turn crash
doesn't cause the notice to re-fire."
```

---

### Task 15: Clear usage state on [[RESET]]

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Replace targeted field clears with `clear_session()`**

Locate the `[[RESET]]` handling (around `crates/gitim-runtime/src/agent_loop.rs:300-315`) — specifically the code that sets `self.session_token = None`. Before or after that in-memory clear, load state and call `clear_session`:

```rust
info!(
    handler = %self.handler,
    "context reset complete, clearing session_token"
);
self.session_token = None;
let mut state = AgentState::load(&self.repo_root)?;
state.clear_session();
state.save(&self.repo_root)?;
tracing::info!(
    session_id = ?self.last_session_id,    // or whatever variable carries it
    reason = "agent_emitted_reset",
    "session_reset"
);
```

If `self.last_session_id` doesn't exist, use the snapshot's `session_id` from the loaded state **before** calling `clear_session()`:

```rust
let mut state = AgentState::load(&self.repo_root)?;
let sid_for_log = state.session_usage.as_ref().map(|s| s.session_id.clone());
state.clear_session();
state.save(&self.repo_root)?;
tracing::info!(session_id = ?sid_for_log, reason = "agent_emitted_reset", "session_reset");
```

- [ ] **Step 2: Also clear on session failure (Task 11 updated the Aborted/Failed paths are already skipped for usage, but we should still clear estimator/notice state so next session starts clean)**

In the Failed branch (around line 325-330), where `self.session_token = None` is already set, add:

```rust
let mut state = AgentState::load(&self.repo_root)?;
state.clear_session();
state.save(&self.repo_root)?;
```

- [ ] **Step 3: Verify build**

```bash
cargo build -p gitim-runtime && cargo test -p gitim-runtime
```

Expected: clean build, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): clear session usage state on [[RESET]] and failure

AgentState::clear_session() zeros session_token, session_usage,
estimated_tokens, usage_notice_pending atomically. Called from the
[[RESET]] path (info-level session_reset log) and the failure path
(silent cleanup — error is logged elsewhere)."
```

---

## Phase 5 — HTTP + SSE Surface

### Task 16: `AgentInfo.session_usage` field + recovery wiring

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: Extend `AgentInfo`**

In `crates/gitim-runtime/src/http.rs` (the `AgentInfo` struct at line 84), add a final field:

```rust
pub struct AgentInfo {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_usage: Option<crate::state::SessionUsageSnapshot>,
}
```

Import at the top if not already present:
```rust
// (SessionUsageSnapshot is accessed via full path; no import needed if qualified)
```

- [ ] **Step 2: Populate at recovery**

In `recover_agents_for_workspace` (around `crates/gitim-runtime/src/http.rs:1605-1733`), at the site where `AgentInfo { ... }` is constructed (there are two such sites around lines 1666-1726), add:

```rust
session_usage: crate::state::AgentState::load(&repo_path)
    .ok()
    .and_then(|s| s.session_usage),
```

Repeat for both sites.

- [ ] **Step 3: Populate at agent creation (`POST /agents/add`)**

Around line 1185 where `ctx.agents.insert(req.handler.clone(), info)` is called, set `session_usage: None` on `info` (it's the default for a fresh agent).

- [ ] **Step 4: Add a method to push snapshot updates from agent_loop into `AgentInfo`**

agent_loop currently holds `activity_tx` for SSE. We also need a way to patch the in-memory `AgentInfo.session_usage`. Add a second `mpsc`/`broadcast` channel OR a shared `Arc<Mutex<HashMap<String, SessionUsageSnapshot>>>`.

Simplest: pass a second `tx: broadcast::Sender<AgentUsageUpdate>` into `AgentLoop`, where `AgentUsageUpdate { agent_id, snapshot }`. The consumer in `http.rs` spawns a task that drains it and patches the agent map.

Concretely, in `crates/gitim-runtime/src/http.rs`, add:

```rust
#[derive(Clone, Debug)]
pub struct AgentUsageUpdate {
    pub agent_id: String,
    pub workspace_id: String,
    pub snapshot: crate::state::SessionUsageSnapshot,
}
```

Extend `WorkspaceContext` to hold a second `broadcast::Sender<AgentUsageUpdate>` next to the existing activity sender. (Find the `WorkspaceContext` definition — likely in `crates/gitim-runtime/src/workspace.rs` — and add the field.)

Also add a field on `AgentLoop` itself:

```rust
// in crates/gitim-runtime/src/agent_loop.rs on the AgentLoop struct
usage_tx: Option<broadcast::Sender<AgentUsageUpdate>>,
```

And thread it through `AgentLoop::with_config()` alongside the existing `activity_tx`.

In `start_agent_loop` (around line 1368), pass both senders into `AgentLoop::with_config`.

In `AgentLoop::update_session_usage` (Task 11), after `state.save(...)`, if `new_snapshot.is_some()`, also send on the usage channel:

```rust
if let Some(snap) = &new_snapshot {
    if let Some(tx) = &self.usage_tx {
        let _ = tx.send(AgentUsageUpdate {
            agent_id: self.handler.clone(),
            workspace_id: self.workspace_id.clone(),
            snapshot: snap.clone(),
        });
    }
}
```

In `http.rs`, spawn a task on server startup that drains the usage channel and calls a small helper:

```rust
fn apply_usage_update(state: &SharedRuntimeState, upd: AgentUsageUpdate) {
    let mut s = state.lock().unwrap();
    if let Some(ctx) = s.workspaces.get_mut(&upd.workspace_id) {
        if let Some(info) = ctx.agents.get_mut(&upd.agent_id) {
            info.session_usage = Some(upd.snapshot);
        }
    }
}
```

- [ ] **Step 5: Verify build + smoke test**

```bash
cargo build -p gitim-runtime && cargo test -p gitim-runtime
```

Expected: clean build, existing tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/src/workspace.rs
git commit -m "feat(runtime): AgentInfo.session_usage + per-turn in-memory patch

GET /agents/:id now returns session_usage when known. Recovery reads
from .gitim/agent-state.json. The running agent loop patches in-memory
AgentInfo via a dedicated broadcast channel (one drain task per
workspace) so polling sees fresh data without re-reading disk."
```

---

### Task 17: Emit SSE `"usage"` event on snapshot update

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Modify: `crates/gitim-runtime/src/http.rs` (only if the SSE emit helper needs changes)

- [ ] **Step 1: Emit `AgentActivityEvent` with `event_type = "usage"`**

In `AgentLoop::update_session_usage` (Task 11), after the in-memory patch channel send, also emit on the existing activity channel:

```rust
if let Some(snap) = &new_snapshot {
    // Emit SSE usage event (detail is JSON-encoded snapshot).
    let detail = serde_json::to_string(snap).unwrap_or_default();
    self.emit_activity("usage", &detail);
}
```

The existing `emit_activity` helper (around line 134) already broadcasts on `activity_tx`. No change needed there.

- [ ] **Step 2: Verify the payload is valid JSON**

Add a targeted unit test to `crates/gitim-runtime/src/state.rs`:

```rust
#[test]
fn snapshot_serializes_without_detail_fields_when_absent() {
    let snap = SessionUsageSnapshot {
        session_id: "sid".into(),
        input_tokens: None,
        output_tokens: None,
        max_tokens: None,
        used_percent: 47.5,
        source: UsageSource::ProviderReported,
        updated_at: "2026-04-20T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&snap).expect("serialize");
    assert!(json.contains("\"session_id\":\"sid\""));
    assert!(json.contains("\"used_percent\":47.5"));
    assert!(json.contains("\"source\":\"provider_reported\""));
    assert!(!json.contains("input_tokens"));  // skipped when None
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p gitim-runtime
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs crates/gitim-runtime/src/state.rs
git commit -m "feat(runtime): broadcast usage snapshot as SSE event_type='usage'

Piggy-backs on the existing /agents/events SSE stream. detail payload
is the serialized SessionUsageSnapshot. WebUI consumes in later task."
```

---

## Phase 6 — WebUI

### Task 18: `Agent.sessionUsage` type

**Files:**
- Modify: `webui-v2/src/lib/types.ts`

- [ ] **Step 1: Extend the `Agent` interface**

In `webui-v2/src/lib/types.ts`, replace the `Agent` interface (lines 5-17) with:

```ts
export interface SessionUsageSnapshot {
  sessionId: string;
  inputTokens?: number;
  outputTokens?: number;
  maxTokens?: number;
  usedPercent: number;
  source: "provider_reported" | "runtime_estimated";
  updatedAt: string;
}

export interface Agent {
  id: string;
  name: string;
  status: AgentStatus;
  provider?: ProviderId;
  systemPrompt: string;
  model?: string;
  env?: Record<string, string>;
  repoPath: string;
  lastActivity?: string; // ISO8601
  messagesProcessed: number;
  errorMessage?: string;
  sessionUsage?: SessionUsageSnapshot;
}
```

Also extend `AgentActivityEvent` to permit the new `"usage"` event type:

```ts
export interface AgentActivityEvent {
  agent_id: string;
  event_type: "tool_use" | "thinking" | "done" | "error" | "usage";
  detail: string;
  timestamp: string;
}
```

- [ ] **Step 2: Verify TypeScript compile**

```bash
cd webui-v2 && npx tsc -b 2>&1 | head -30
```

Expected: no new errors. (Existing errors unrelated to this change may linger — only new ones matter.)

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/lib/types.ts
git commit -m "feat(webui): add SessionUsageSnapshot type and Agent.sessionUsage

Camel-cased mirror of the runtime's SessionUsageSnapshot. Extends
AgentActivityEvent to permit 'usage' as a valid event_type."
```

---

### Task 19: `mapBackendAgent` maps `session_usage`

**Files:**
- Modify: `webui-v2/src/lib/client.ts`

- [ ] **Step 1: Extend `mapBackendAgent`**

In `webui-v2/src/lib/client.ts`, locate `mapBackendAgent` (around lines 467-481). Replace with:

```ts
function mapBackendAgent(raw: Record<string, unknown>): Agent {
  const rawUsage = raw.session_usage as Record<string, unknown> | undefined;
  const sessionUsage: Agent["sessionUsage"] = rawUsage
    ? {
        sessionId: (rawUsage.session_id as string) ?? "",
        inputTokens: rawUsage.input_tokens as number | undefined,
        outputTokens: rawUsage.output_tokens as number | undefined,
        maxTokens: rawUsage.max_tokens as number | undefined,
        usedPercent: (rawUsage.used_percent as number) ?? 0,
        source: (rawUsage.source as "provider_reported" | "runtime_estimated") ?? "provider_reported",
        updatedAt: (rawUsage.updated_at as string) ?? "",
      }
    : undefined;

  return {
    id: (raw.id ?? raw.handler) as string,
    name: (raw.display_name ?? raw.handler) as string,
    status: ((raw.status as string) === "idle" ? "offline" : raw.status) as Agent["status"],
    provider: (raw.provider as ProviderId) ?? undefined,
    systemPrompt: (raw.system_prompt as string) ?? "",
    model: (raw.model as string) ?? undefined,
    env: (raw.env as Record<string, string>) ?? undefined,
    repoPath: (raw.repo_path as string) ?? "",
    messagesProcessed: (raw.messages_processed as number) ?? 0,
    lastActivity: raw.last_activity as string | undefined,
    errorMessage: (raw.error_message as string) ?? undefined,
    sessionUsage,
  };
}
```

- [ ] **Step 2: Verify compile**

```bash
cd webui-v2 && npx tsc -b 2>&1 | head -30
```

Expected: no new errors.

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/lib/client.ts
git commit -m "feat(webui): map session_usage from /agents HTTP responses

snake_case → camelCase projection. Absent session_usage stays
undefined; no placeholder data."
```

---

### Task 20: SSE handler patches store on `"usage"` events

**Files:**
- Modify: `webui-v2/src/hooks/use-agent-activity.ts`

- [ ] **Step 1: Branch on event type**

Replace the `es.onmessage` block in `webui-v2/src/hooks/use-agent-activity.ts` (around lines 47-54) with:

```ts
es.onmessage = (e) => {
  try {
    const event: AgentActivityEvent = JSON.parse(e.data);
    if (event.event_type === "usage") {
      try {
        const snap = JSON.parse(event.detail);
        useAgentStore.getState().updateAgent(event.agent_id, {
          sessionUsage: {
            sessionId: snap.session_id ?? "",
            inputTokens: snap.input_tokens,
            outputTokens: snap.output_tokens,
            maxTokens: snap.max_tokens,
            usedPercent: snap.used_percent ?? 0,
            source: snap.source ?? "provider_reported",
            updatedAt: snap.updated_at ?? "",
          },
        });
      } catch {
        // malformed usage payload — ignore
      }
      return;  // do NOT push usage events to the activity log
    }
    push(event);
  } catch {
    // ignore malformed events
  }
};
```

Add the import at the top of the file:

```ts
import { useAgentStore } from "./use-agent-store";
```

- [ ] **Step 2: Verify compile**

```bash
cd webui-v2 && npx tsc -b 2>&1 | head -30
```

Expected: no new errors.

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/hooks/use-agent-activity.ts
git commit -m "feat(webui): route SSE 'usage' events into the agent store

Usage snapshots patch useAgentStore.updateAgent instead of being
pushed into the activity log — they're not user-visible activity,
just state. Malformed detail payloads are swallowed silently."
```

---

### Task 21: Popover header-right badge

**Files:**
- Modify: `webui-v2/src/components/chat/agent-status-panel.tsx`

- [ ] **Step 1: Render the badge**

In `webui-v2/src/components/chat/agent-status-panel.tsx`, update `AgentRow` component. Replace the existing expanded popover (lines 86-95):

```tsx
{expanded && activities.length > 0 && (
  <div className="absolute left-0 top-full z-50 w-72 max-h-52 overflow-y-auto rounded-md border border-border bg-popover shadow-xl p-2 mt-1">
    <p className="text-[11px] font-semibold uppercase text-text-muted tracking-wider mb-1">
      {name} — Recent Activity
    </p>
    {activities.map((evt, i) => (
      <ActivityLine key={`${evt.timestamp}-${i}`} event={evt} />
    ))}
  </div>
)}
```

with:

```tsx
{expanded && activities.length > 0 && (
  <div className="absolute left-0 top-full z-50 w-72 max-h-52 overflow-y-auto rounded-md border border-border bg-popover shadow-xl p-2 mt-1">
    <div className="flex items-baseline justify-between gap-2 mb-1">
      <p className="text-[11px] font-semibold uppercase text-text-muted tracking-wider truncate">
        {name} — Recent Activity
      </p>
      <UsageBadge agentId={agentId} />
    </div>
    {activities.map((evt, i) => (
      <ActivityLine key={`${evt.timestamp}-${i}`} event={evt} />
    ))}
  </div>
)}
```

- [ ] **Step 2: Add `UsageBadge` component**

At the top of the file (below the existing `import` block), add:

```tsx
function UsageBadge({ agentId }: { agentId: string }) {
  const usage = useAgentStore(
    (s) => s.agents.find((a) => a.id === agentId)?.sessionUsage,
  );
  if (!usage) return null;

  const warning = usage.usedPercent >= 80;
  const colorClass = warning ? "text-warning" : "text-text-faint";
  const sidShort = usage.sessionId.slice(0, 8);

  return (
    <span className={`text-[10px] font-mono ${colorClass} shrink-0`}>
      sid:{sidShort} · {usage.usedPercent.toFixed(0)}%
    </span>
  );
}
```

If the theme doesn't define `text-warning`, use `text-amber-400` as a fallback (`grep -r "text-warning" webui-v2/src | head` will confirm).

- [ ] **Step 3: Verify TypeScript + eslint**

```bash
cd webui-v2 && npx tsc -b && npx eslint src/components/chat/agent-status-panel.tsx
```

Expected: clean.

- [ ] **Step 4: Manual visual verification**

Run the dev server:

```bash
cd webui-v2 && npm run dev
```

Open the app, connect a workspace with at least one agent, expand the agent row.

Test the four states:
1. **No usage yet** (fresh agent, before first turn completes): the popover header right side is empty.
2. **Low usage** (after first turn with `used_percent = 47`, possible by running a mock or a real claude): `sid:xxxxxxxx · 47%` in muted color.
3. **High usage** (`used_percent >= 80`): same text, warning (amber/yellow) color.
4. **Reset** (after `[[RESET]]`): badge disappears on the next agent update (either SSE usage event absent, or backend emits no snapshot).

For states (2) and (3), the mock provider (Task 2) can be used end-to-end if a stub runtime is running with `MockProvider::with_usage(...)`. Otherwise observe a real Claude agent.

- [ ] **Step 5: Commit**

```bash
git add webui-v2/src/components/chat/agent-status-panel.tsx
git commit -m "feat(webui): session_id + used_percent badge in agent popover

Top-right of the expanded 'Recent Activity' popover now shows
'sid:xxxxxxxx · NN%'. Warning color applies at ≥80%. Folded row is
unchanged. No badge renders when sessionUsage is absent."
```

---

## Phase 7 — Integration Tests

### Task 22: E2E usage reporting and exposure

**Files:**
- Create: `crates/gitim-runtime/tests/session_usage_e2e.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/gitim-runtime/tests/session_usage_e2e.rs`:

```rust
mod common;

use gitim_agent_provider::{MockProvider, ProviderUsage};
use gitim_runtime::state::{AgentState, UsageSource};
// ... any additional setup imports the common test harness requires

// NOTE: this is a sketch. The exact setup depends on common/mod.rs —
// follow the pattern used by existing tests/claude.rs which spins up a
// real daemon + agent loop with MockProvider.

#[tokio::test(flavor = "multi_thread")]
async fn usage_snapshot_surfaces_via_agent_state_and_http() {
    // 1. Spin up daemon + runtime with a workspace + one agent backed by
    //    MockProvider::with_usage(input_tokens=100_000, output_tokens=200).
    //
    // 2. Send a message to the agent channel so a turn fires.
    //
    // 3. Poll GET /workspaces/:slug/agents/:handler for up to 10s waiting
    //    for response JSON to contain "session_usage".
    //
    // 4. Assert:
    //    - session_usage.used_percent is 50.0 (100k / 200k)
    //    - session_usage.source is "provider_reported"
    //    - session_usage.session_id is non-empty
    //
    // 5. Load AgentState from .gitim/agent-state.json; assert session_usage
    //    field round-trips.
    //
    // 6. Assert estimated_tokens > 0 (estimator ran in parallel).
}
```

Fill in the harness using the same pattern as `crates/gitim-runtime/tests/claude.rs` (which already drives agent loops with mock providers). If `MockProvider::with_usage` isn't wired through the runtime's `create_provider` factory yet, extend the factory to accept an optional pre-configured provider for test overrides.

- [ ] **Step 2: Run the test**

```bash
cargo test -p gitim-runtime --test session_usage_e2e -- --nocapture
```

Expected: PASS within 30s (the default poll timeout).

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/tests/session_usage_e2e.rs
git commit -m "test(runtime): e2e session_usage reported and exposed

Full-stack smoke: MockProvider reports synthetic usage → runtime
computes snapshot → persists to agent-state.json → exposes via
GET /agents/:id. The estimator path is exercised in parallel
(asserted by estimated_tokens > 0 post-turn)."
```

---

### Task 23: E2E threshold injection

**Files:**
- Create: `crates/gitim-runtime/tests/threshold_injection_e2e.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/gitim-runtime/tests/threshold_injection_e2e.rs`:

```rust
mod common;

use gitim_agent_provider::{MockProvider, ProviderUsage};

#[tokio::test(flavor = "multi_thread")]
async fn notice_is_prepended_on_turn_after_crossing() {
    // Strategy: use a MockProvider that records every prompt it receives.
    //
    // 1. Turn 1: provider reports used_percent = 55 (below threshold).
    //    After turn, runtime computes snapshot (55%), does NOT set
    //    usage_notice_pending.
    //
    // 2. Turn 2: provider reports used_percent = 82 (crosses 80%).
    //    After turn, runtime sets usage_notice_pending = true.
    //
    // 3. Turn 3: Before provider.execute, runtime prepends the system
    //    notice. The mock provider captures the prompt; we assert it
    //    starts with "[系统通知]".
    //
    // 4. Turn 4: Assert the next prompt does NOT have the preamble
    //    (flag is cleared, one-shot semantics).
    //
    // The mock needs a builder that lets the test swap the reported
    // usage per turn — extend MockProvider accordingly if not present.
}
```

Extend `MockProvider` if needed with a `with_scripted_usage(Vec<ProviderUsage>)` method that consumes one entry per call.

- [ ] **Step 2: Run the test**

```bash
cargo test -p gitim-runtime --test threshold_injection_e2e -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/tests/threshold_injection_e2e.rs crates/gitim-agent-provider/src/mock.rs
git commit -m "test(runtime): e2e 80% crossing injects notice exactly once

Scripts four turns through MockProvider — 55%, 82%, 82%, 82%.
Asserts the preamble appears on turn 3 only. Covers the crossing
state machine end-to-end (detect → set flag → inject → clear)."
```

---

## Final Verification

### Task 24: Full-build + test sweep

- [ ] **Step 1: Cargo**

```bash
cargo build --all && cargo test
```

Expected: all green. Full duration likely ~2–3 min on a cold cache.

- [ ] **Step 2: WebUI**

```bash
cd webui-v2 && npx tsc -b && npx eslint src/
```

Expected: clean.

- [ ] **Step 3: Manual smoke**

Follow the manual-validation checklist from 01-design.md §6:
- Observe an agent for a few turns; confirm `turn_usage` log appears at `RUST_LOG=debug`.
- Force a high-usage scenario (e.g. have the agent process a very long document); wait for `threshold_crossed_80pct` info log.
- Verify the next turn's prompt in daemon logs contains `[系统通知]`.
- Verify the agent emits `[[RESET]]` (or, if it doesn't, file as a prompt-tuning issue — it's not an implementation bug).
- WebUI: expand the agent row, confirm badge appears and reaches warning color before the reset.

- [ ] **Step 4: Final commit (if anything trailing)**

If the sweep uncovered any fix, commit it under the relevant Task's scope. Otherwise, nothing to commit.

---

## Done

All 24 tasks complete. Merge path: `superpowers:finishing-a-development-branch`.
