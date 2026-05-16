# Kimi + Cursor Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 multica 的 cursor 和 kimi 两个 backend provider 移植到 GitIM,实现完整的 `Provider` trait + `create()` 注册 + `preflight_*_with_config` + `add_agent` server gate wiring。前端 `PROVIDER_IDS` 不动。

**Architecture:** 3 个 atomic commit。先做 cursor(独立 stream-json,不动 hermes);再把 hermes 的 ACP wire 抽到 `acp/` 子模块成 `AcpClient`(纯重构,行为不变);最后 kimi 用 `AcpClient` 实装。所有 provider 都是 `self_managed_context = false`,走 claude-style session model(runtime 持 opaque token,provider 实现 wire-specific resume)。

**Tech Stack:** Rust 2021,`tokio` async runtime,`async_trait`,`serde_json`,`tracing`。跟现有 provider crate 完全一致。

**Spec:** [`docs/plans/kimi-cursor-providers/00-requirements.md`](00-requirements.md)

**Multica reference:** `/Users/lewisliu/ateam/code-skills/.repos/multica/server/pkg/agent/{cursor.go, cursor_test.go, kimi.go, kimi_test.go, hermes.go}` —— 实施时把这些 Go 实现作为权威 reference 翻译。

---

## 项目惯例

- 修改任何已存在文件之前先 `Read` 一遍。
- 测试惯例:**外部 `tests/` 目录优先**,内联 `#[cfg(test)]` 用于纯 unit test。本 plan 全部用内联 `#[cfg(test)]`,因为都是纯 parse / mapper unit test,没有需要 spawn daemon。
- `serial_test`、`HomeGuard`、`GITIM_LOG_DIR` 这些 test isolation 机制本 plan 用不到(不动 daemon log)。
- 跑测试节奏:每个 task 末尾跑 scoped test(`cargo test -p gitim-agent-provider <filter>` / `cargo test -p gitim-runtime <filter>`),整个 plan 完成后跑一次全量 `cargo test`。
- 不要无脑 `cargo test`,贵。
- 每个 task 末尾 `git commit`。
- 跑 build 用 `cargo build -p gitim-agent-provider -p gitim-runtime`(scoped,跟全量 build 比快)。

---

## Task 1: Cursor Provider 完整实装

**目标:** 把 `stubs::CursorProvider`(返 `NotImplemented`)替换成完整的 stream-json provider,通 `create()` / `preflight` / `http.rs` 三处 wiring。一个 commit 完成。

**Files:**
- Create: `crates/gitim-agent-provider/src/cursor/mod.rs`
- Create: `crates/gitim-agent-provider/src/cursor/parse.rs`
- Delete: `crates/gitim-agent-provider/src/stubs/mod.rs`(整个 `stubs/` 目录)
- Modify: `crates/gitim-agent-provider/src/lib.rs`
- Modify: `crates/gitim-agent-provider/src/provider.rs`
- Modify: `crates/gitim-runtime/src/preflight.rs`
- Modify: `crates/gitim-runtime/src/http.rs`

**Multica reference:** [`multica/server/pkg/agent/cursor.go`](../../../../../../code-skills/.repos/multica/server/pkg/agent/cursor.go),完整 422 行。

### Step 1.1: Baseline 全量测试,确认 main 是绿的

- [ ] **Run baseline**

```bash
cargo test -p gitim-agent-provider -p gitim-runtime 2>&1 | tail -30
```

Expected: 全部 pass。如果有红测试,记下来,跟最终验证对比 —— 那些红的是祖传问题,不归本 plan 修。

### Step 1.2: 写 stream-json envelope 的 parse_event 失败测试

`cursor::parse::tests::*` 是这一步的目标。先创建文件 + 写 5 个测试,**不写实现**。

- [ ] **Create `crates/gitim-agent-provider/src/cursor/parse.rs` with tests only**

```rust
//! Parse cursor-agent's stream-json envelopes into typed events the
//! provider driver can dispatch. The envelope shape is documented inline
//! in `CursorStreamEvent`. See `multica/server/pkg/agent/cursor.go:282+`
//! for the reference Go decoder this is translated from.

use serde::Deserialize;
use serde_json::Value;

/// One line off cursor-agent's stdout stream. `type` is the dispatch key;
/// the other fields are populated per-type (most are absent on any given
/// event — `serde(default)` keeps deserialization permissive).
#[derive(Debug, Deserialize, Default)]
pub struct CursorStreamEvent {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// For `assistant` events: `{ content: [ { type, text|input|name|id } ] }`.
    #[serde(default)]
    pub message: Option<Value>,
    /// For `tool_use` standalone envelopes (NOT the assistant-embedded form).
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub parameters: Option<Value>,
    /// For `tool_result` standalone envelopes.
    #[serde(default)]
    pub output: Option<String>,
    /// For `result` envelopes.
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, rename = "result")]
    pub result_text: Option<String>,
    #[serde(default)]
    pub usage: Option<CursorUsage>,
    /// For `text` and `step_finish` envelopes.
    #[serde(default)]
    pub part: Option<Value>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct CursorUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

pub fn parse_event(line: &str) -> Option<CursorStreamEvent> {
    serde_json::from_str::<CursorStreamEvent>(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_init() {
        let e = parse_event(r#"{"type":"system","subtype":"init","session_id":"s1"}"#).unwrap();
        assert_eq!(e.r#type, "system");
        assert_eq!(e.subtype.as_deref(), Some("init"));
        assert_eq!(e.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn parses_assistant_text_block() {
        let e = parse_event(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        )
        .unwrap();
        let content = e.message.unwrap();
        let blocks = content.get("content").unwrap().as_array().unwrap();
        assert_eq!(blocks[0].get("type").unwrap(), "text");
        assert_eq!(blocks[0].get("text").unwrap(), "hi");
    }

    #[test]
    fn parses_tool_use_envelope() {
        let e = parse_event(
            r#"{"type":"tool_use","tool_name":"read_file","tool_id":"t1","parameters":{"path":"foo"}}"#,
        )
        .unwrap();
        assert_eq!(e.tool_name.as_deref(), Some("read_file"));
        assert_eq!(e.tool_id.as_deref(), Some("t1"));
        assert_eq!(e.parameters.unwrap().get("path").unwrap(), "foo");
    }

    #[test]
    fn parses_tool_result_envelope() {
        let e = parse_event(
            r#"{"type":"tool_result","tool_id":"t1","output":"file contents"}"#,
        )
        .unwrap();
        assert_eq!(e.tool_id.as_deref(), Some("t1"));
        assert_eq!(e.output.as_deref(), Some("file contents"));
    }

    #[test]
    fn parses_result_with_usage() {
        let e = parse_event(
            r#"{"type":"result","session_id":"s1","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":20},"model":"claude-sonnet-4-6"}"#,
        )
        .unwrap();
        assert_eq!(e.session_id.as_deref(), Some("s1"));
        let u = e.usage.unwrap();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.cache_read_input_tokens, 20);
    }

    #[test]
    fn returns_none_on_malformed_json() {
        assert!(parse_event("not json").is_none());
        assert!(parse_event("").is_none());
    }
}
```

- [ ] **Create skeleton `crates/gitim-agent-provider/src/cursor/mod.rs` (no impl yet, just to register sub-mod)**

```rust
//! Cursor Agent CLI provider — stream-json protocol.
//!
//! Spec: docs/plans/kimi-cursor-providers/00-requirements.md §"Cursor 设计"
//! Reference: multica/server/pkg/agent/cursor.go

pub mod parse;
```

- [ ] **Wire `cursor` mod into `lib.rs` BUT do NOT remove stubs yet** (so existing build still compiles)

```rust
// lib.rs — add at top of mod list, alphabetically near other providers
pub mod cursor;
```

Run: `cargo build -p gitim-agent-provider`
Expected: PASS.

### Step 1.3: 跑测试,确认它们 PASS(parse.rs 的 tests 是自包含的)

- [ ] **Run cursor parse tests**

```bash
cargo test -p gitim-agent-provider cursor::parse
```

Expected: 6 passed. (Tests are self-contained — the parse module compiles and runs independently.)

- [ ] **Commit checkpoint (intermediate)**

```bash
git add crates/gitim-agent-provider/src/cursor/ crates/gitim-agent-provider/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(cursor): add stream-json envelope parser

Self-contained parse module — no provider impl yet, just decodes the
JSONL envelopes cursor-agent emits. Reference: multica/cursor.go:282+.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

> **Why an intermediate commit:** Cursor 改动量大,把"parser 独立可测"作为子里程碑落盘,后面 execute() 跑挂时方便对照。Approach A 里的"3 个 atomic commit"是按 cursor / hermes / kimi 切分,**这里的 intermediate commit 是 cursor 内部的子检查点,不破坏 atomic 原则**。

### Step 1.4: 写 `CursorProvider::execute` 的失败测试(契约测试,不跑真实 CLI)

**Files:**
- Modify: `crates/gitim-agent-provider/src/cursor/mod.rs`

我们不能在 unit test 里 spawn 真实 `cursor-agent`,所以契约测试只测**确定性的**部分:`build_args` 函数(纯函数,把 ExecOptions + prompt 翻成 argv)。Provider trait 的 reports_usage / usage_is_cumulative / self_managed_context 三个 getter 用值断言。

- [ ] **Write failing tests inline in `cursor/mod.rs`**

```rust
//! Cursor Agent CLI provider — stream-json protocol.
//!
//! Spec: docs/plans/kimi-cursor-providers/00-requirements.md §"Cursor 设计"
//! Reference: multica/server/pkg/agent/cursor.go

use async_trait::async_trait;
use crate::{ExecOptions, Provider, ProviderConfig, ProviderError, Session};

pub mod parse;

pub struct CursorProvider {
    config: ProviderConfig,
}

impl CursorProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for CursorProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        unimplemented!("cursor execute not yet implemented")
    }
}

/// Build the argv vector for a one-shot `cursor-agent` invocation.
/// Reference: multica/server/pkg/agent/cursor.go:397-422.
///
/// Shape: `chat -p <merged_prompt> --output-format stream-json --yolo
///   [--workspace <cwd>] [--model <m>] [--resume <id>]`
///
/// `merged_prompt` = `system_prompt + "\n\n---\n\n" + prompt` when
/// `opts.system_prompt` is Some, else just `prompt`. cursor-agent CLI
/// does not support `--system-prompt` (see multica/cursor.go:415-416).
pub(crate) fn build_args(prompt: &str, opts: &ExecOptions) -> Vec<String> {
    let merged_prompt = match &opts.system_prompt {
        Some(sp) if !sp.is_empty() => format!("{sp}\n\n---\n\n{prompt}"),
        _ => prompt.to_string(),
    };
    let mut args = vec![
        "chat".to_string(),
        "-p".to_string(),
        merged_prompt,
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--yolo".to_string(),
    ];
    if let Some(cwd) = &opts.cwd {
        args.push("--workspace".to_string());
        args.push(cwd.to_string_lossy().into_owned());
    }
    if let Some(model) = &opts.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(resume) = &opts.resume_token {
        args.push("--resume".to_string());
        args.push(resume.clone());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_args_minimal() {
        let args = build_args("hi", &ExecOptions::default());
        assert_eq!(
            args,
            vec!["chat", "-p", "hi", "--output-format", "stream-json", "--yolo"]
        );
    }

    #[test]
    fn build_args_with_system_prompt_merges() {
        let opts = ExecOptions {
            system_prompt: Some("sys".to_string()),
            ..Default::default()
        };
        let args = build_args("hi", &opts);
        assert_eq!(args[2], "sys\n\n---\n\nhi");
    }

    #[test]
    fn build_args_with_cwd_model_resume() {
        let opts = ExecOptions {
            cwd: Some(PathBuf::from("/tmp/x")),
            model: Some("claude-sonnet-4-6".to_string()),
            resume_token: Some("sess-abc".to_string()),
            ..Default::default()
        };
        let args = build_args("hi", &opts);
        // Order-sensitive assertions
        assert!(args.windows(2).any(|w| w == ["--workspace", "/tmp/x"]));
        assert!(args.windows(2).any(|w| w == ["--model", "claude-sonnet-4-6"]));
        assert!(args.windows(2).any(|w| w == ["--resume", "sess-abc"]));
    }

    #[test]
    fn provider_trait_flags() {
        let p = CursorProvider::new(ProviderConfig::default());
        assert!(p.reports_usage());
        assert!(!p.usage_is_cumulative());
        assert!(!p.self_managed_context());
    }
}
```

- [ ] **Run failing tests**

```bash
cargo test -p gitim-agent-provider cursor::tests
```

Expected: 4 tests; `build_args_*` pass (pure functions implemented above);
`provider_trait_flags` may pass since defaults are already `true/false/false`.
Actually, all 4 will pass because we wrote impl alongside tests. The failing-test
discipline really applies once we touch execute() — see Step 1.6.

### Step 1.5: 实装 `CursorProvider::execute` 的主流程

完整翻译 multica `cursor.go:Execute`(行 26-225),逻辑:
1. resolve bin path (`config.executable_path` 或 `"cursor-agent"`)
2. `which::which(bin)` 验证存在 → 不存在返 `ProviderError::ExecutableNotFound`
3. 设 timeout(`opts.timeout.unwrap_or(20 * 60s)`)
4. 构 argv via `build_args(prompt, &opts)`
5. spawn `tokio::process::Command`,`stdout/stderr` piped,`kill_on_drop(true)`,设 cwd + env
6. spawn driver task:
   - `tokio::io::BufReader::new(stdout)` 按行读
   - 每行 `parse::parse_event(line)`,按 `event.type` dispatch:
     - `system{subtype:"init"}` → 发 `Event::Status { status: "running" }`
     - `system{subtype:"error"}` → 发 `Event::Error`
     - `assistant` → 调 `handle_assistant_message(message, event_tx, output)`
     - `tool_use` → 发 `Event::ToolUse`
     - `tool_result` → 发 `Event::ToolResult`
     - `result` → 终态 + usage 抓取
     - `error` → 发 `Event::Error`,记 finalError
     - `text` → part 解 `{ text }`,发 `Event::Text` + 累 output
     - `step_finish` → part 解 `{ tokens: { input, output, cache: { read } } }`,累 step_usage
   - Cancel/timeout 监听 → 设 finalStatus
   - 最后通过 `result_tx` 发 `ExecResult`(session_token = session_id, usage = result.usage ?? step_usage)
7. 返 `Session { events, result, abort_handle, cancel_token }`

参考结构可以 model 在 `claude/mod.rs`(同样是 stream-json + 驱动 task)。

- [ ] **Read `claude/mod.rs` as structural reference**

```bash
# Just for the human reader / subagent — no tool needed, file is ~700 lines, focus on:
#   - line 80-150: spawn pattern
#   - line 180-260: stream parse dispatch
#   - line 320-345: ExecResult assembly
```

- [ ] **Write full execute() impl in `cursor/mod.rs`**(参考 claude/mod.rs 同型代码 + multica cursor.go:26-225 翻译;长度估计 ~300 行,**这一步不在本 plan 里 inline 整段代码,因为 plan 文档已经太长且实施者更适合在 IDE 里 model 着 claude 邻居代码写**。要求:)

  - 用 `async_trait::async_trait`、`tokio::sync::{mpsc, oneshot}`、`tokio_util::sync::CancellationToken`,所有信号通道跟 claude 一致
  - 错误用 `crate::ProviderError`
  - `handle_assistant_message(value, &event_tx, &output)`:从 `message.content[]` 取各 block,按 `block.type`(`output_text|text|thinking|tool_use`)分发,跟 multica cursor.go:227-265 一致
  - `step_finish` 累 step_usage,`result` 拿 `result.usage` 时整体覆盖,fallback step_usage(跟 multica:193-196 一致)
  - 输出 `output` accumulator 用 `String`(单 driver task 内,不需要 Mutex)
  - **必须**:`reports_usage() = true`(default 即为 true,无需 override),`usage_is_cumulative() = false`(default 即为 false), `self_managed_context() = false`(default)。所以**不需要 override 任何 trait method**,只实装 `execute()` —— 我们在 Step 1.4 的 `provider_trait_flags` test 也已经断言这些默认值。

实施时把 multica `cursor_test.go` 里的 envelope 字符串当 fixture 抄,在 `cursor::parse::tests` 已经覆盖了 5 个核心 case。

- [ ] **Run cursor unit tests after impl**

```bash
cargo test -p gitim-agent-provider cursor
```

Expected: 10 tests pass(parse 6 + tests 4)。

### Step 1.6: 加 `preflight_cursor_with_config` 到 preflight.rs

参考 `preflight_claude_with_config`(`preflight.rs:304+`)的结构,因为 cursor-agent 用的也是 stream-json,preflight 行为很相似。

- [ ] **Read existing reference**

```bash
# In your head, scan crates/gitim-runtime/src/preflight.rs:304-450
# for `preflight_claude_with_config`'s shape (spawn → wait → parse → classify).
```

- [ ] **Add `DEFAULT_BIN_CURSOR` const + `preflight_cursor_with_config` fn**

In `crates/gitim-runtime/src/preflight.rs`,新增 const 跟 fn:

```rust
// Near line 1366 with the other DEFAULT_BIN_* consts
const DEFAULT_BIN_CURSOR: &str = "cursor-agent";

/// Preflight cursor-agent CLI with a real "say hi" hello.
///
/// Flow:
/// 1. Spawn `cursor-agent --version` to capture version string.
/// 2. Spawn `cursor-agent chat -p "say hi" --output-format stream-json --yolo
///    [--model <m>]`, read stdout line-by-line for the first `assistant`
///    text content or `result` event; kill on first hit.
/// 3. Bail with the appropriate `ErrorKind` on missing binary / timeout /
///    non-zero exit before a text event arrives.
pub async fn preflight_cursor_with_config(
    bin: &str,
    timeout: Duration,
    overrides: PreflightOverrides,
) -> PreflightResult {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    let provider = "cursor";
    let started = Instant::now();

    // 1. Verify --version.
    let version = match TokioCommand::new(bin).arg("--version").output().await {
        Ok(out) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        Ok(_) => None,
        Err(e) => {
            return PreflightResult::failure(
                provider,
                ErrorKind::NotInstalled,
                format!("cursor-agent --version failed: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    // 2. Spawn hello.
    let mut cmd = TokioCommand::new(bin);
    cmd.arg("chat")
        .arg("-p")
        .arg("say hi")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--yolo")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(ref m) = overrides.model_override {
        cmd.arg("--model").arg(m);
    }
    if let Some(env) = &overrides.env_override {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return PreflightResult::failure(
                provider,
                if e.kind() == std::io::ErrorKind::NotFound {
                    ErrorKind::NotInstalled
                } else {
                    ErrorKind::Other
                },
                format!("spawn {bin}: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    // 3. Read until first text/result event or timeout.
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout).lines();

    let read_fut = async {
        let mut preview = String::new();
        while let Some(line) = reader.next_line().await.ok().flatten() {
            preview.push_str(&line);
            preview.push('\n');
            if preview.len() > 4096 {
                break;
            }
            if let Some(evt) = gitim_agent_provider::cursor::parse::parse_event(&line) {
                match evt.r#type.as_str() {
                    "assistant" | "text" | "result" => return Some(preview),
                    "error" => {
                        return Some(format!("cursor reported error: {preview}"));
                    }
                    _ => continue,
                }
            }
        }
        None
    };

    let res = tokio::time::timeout(timeout, read_fut).await;
    let _ = child.start_kill();
    let _ = child.wait().await;

    let duration_ms = started.elapsed().as_millis() as u64;
    match res {
        Ok(Some(preview)) => PreflightResult::success(
            provider,
            version,
            overrides.model_override.clone(),
            duration_ms,
            Some(preview.chars().take(500).collect()),
        ),
        Ok(None) => PreflightResult::failure(
            provider,
            ErrorKind::Other,
            "cursor-agent exited before emitting any assistant/result event",
            duration_ms,
        ),
        Err(_) => PreflightResult::failure(
            provider,
            ErrorKind::Timeout,
            format!("cursor-agent preflight exceeded {}ms", timeout.as_millis()),
            duration_ms,
        ),
    }
}
```

⚠ **重要**:上面用 `gitim_agent_provider::cursor::parse::parse_event` 是因为 `gitim-runtime` 已经 depend on `gitim-agent-provider`。**确认** crate dep 关系:在 `crates/gitim-runtime/Cargo.toml` grep `gitim-agent-provider`,有就直接用,没有就加 `gitim-agent-provider = { path = "../gitim-agent-provider" }`(99% 已经有,因为 runtime 调 Provider trait)。

实际为了不让 preflight 模块跟 provider crate 内部 wire 解析逻辑耦合,可以在 preflight.rs 内部写一个 minimal `is_terminal_event(line: &str) -> Option<&str>` 用 `serde_json::Value::get("type")` 直接读,不导 cursor 私有 module。改写下面这块:

```rust
            if let Some(evt_type) = serde_json::from_str::<serde_json::Value>(&line)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
            {
                match evt_type.as_str() {
                    "assistant" | "text" | "result" => return Some(preview),
                    "error" => return Some(format!("cursor reported error: {preview}")),
                    _ => continue,
                }
            }
```

去掉 `use gitim_agent_provider::...`,把 preflight 跟 provider crate 解耦。**采用这个写法**(更干净)。

- [ ] **Add `cursor_bin: Option<String>` to `PreflightDispatchOverrides` struct**

In `crates/gitim-runtime/src/preflight.rs` 约 line 1395:

```rust
#[derive(Debug, Clone, Default)]
pub struct PreflightDispatchOverrides {
    pub claude_bin: Option<String>,
    pub codex_bin: Option<String>,
    pub opencode_bin: Option<String>,
    pub pi_bin: Option<String>,
    pub hermes_bin: Option<String>,
    pub cursor_bin: Option<String>,        // NEW
    pub hermes_home: Option<PathBuf>,
    pub outer_timeout: Option<Duration>,
}
```

- [ ] **Add cursor dispatch branch in `dispatch_preflight`**

In `dispatch_preflight` 函数(约 line 1502+),在 `"hermes" =>` 后、`other =>` 前加:

```rust
        "cursor" => {
            let bin = overrides.cursor_bin.as_deref().unwrap_or(DEFAULT_BIN_CURSOR);
            preflight_cursor_with_config(bin, inner_timeout, prov_overrides).await
        }
```

- [ ] **Build to verify**

```bash
cargo build -p gitim-runtime
```

Expected: PASS.

### Step 1.7: 把 cursor 切换到新实装 + 删 stubs

- [ ] **Modify `crates/gitim-agent-provider/src/provider.rs::create()`**

把:
```rust
        "cursor" => Ok(Box::new(crate::stubs::CursorProvider::new(config))),
```

改成:
```rust
        "cursor" => Ok(Box::new(crate::cursor::CursorProvider::new(config))),
```

- [ ] **Modify `crates/gitim-agent-provider/src/lib.rs`**

把 `mod stubs;` 整行删除。`pub mod cursor;` 应该在 Step 1.2 已加。

- [ ] **Delete `crates/gitim-agent-provider/src/stubs/` directory**

```bash
rm -r crates/gitim-agent-provider/src/stubs
```

- [ ] **Build to verify the switch compiled**

```bash
cargo build -p gitim-agent-provider
```

Expected: PASS,没有 `stubs` 残留引用。

### Step 1.8: 跑全部 cursor 相关 test

- [ ] **Run scoped tests**

```bash
cargo test -p gitim-agent-provider cursor
cargo test -p gitim-runtime preflight | grep -i cursor || echo "(no cursor-named preflight tests yet — that's fine, no e2e tests in this plan)"
```

Expected: cursor crate tests 全 pass。preflight 本 plan **不加** Rust 端 cursor preflight tests(那需要 fake `cursor-agent` shell script fixture,工作量超出 plan scope;e2e `#[ignore]` test 单独由用户手动跑)。

### Step 1.9: Commit cursor (Approach A 的第 1 个 atomic commit)

- [ ] **Final commit for Task 1**

```bash
git add -A crates/gitim-agent-provider/ crates/gitim-runtime/src/preflight.rs
git status        # verify nothing else snuck in
git commit -m "$(cat <<'EOF'
feat(cursor): port cursor-agent provider from multica

Stream-json driver replaces the stubs::CursorProvider placeholder.
- crates/gitim-agent-provider/src/cursor/{mod.rs,parse.rs}
- crates/gitim-runtime/src/preflight.rs: preflight_cursor_with_config
  + cursor_bin override slot + dispatch_preflight branch
- stubs/ deleted (only held the cursor placeholder)

Provider flags: reports_usage=true, usage_is_cumulative=false,
self_managed_context=false — runtime owns the [[RESET]] channel and
the occupancy preamble, same as claude/codex.

Wire: chat -p <merged_prompt> --output-format stream-json --yolo
[--workspace <cwd>] [--model X] [--resume <id>]. system_prompt is
prepended to user prompt because cursor-agent CLI has no
--system-prompt (see multica/cursor.go:415-416).

Frontend PROVIDER_IDS deliberately not updated — webui dropdown
exposure deferred per design.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

> **Note on `http.rs`**: `add_agent` 的 server-side preflight gate 已经把 dispatch 委托给 `preflight_for_add_request`,后者内部调 `dispatch_preflight` 跑 match。所以**只要 `dispatch_preflight` 加了 `"cursor" =>` 分支,`http.rs` 不需要任何改动**。Spec 里 §"http.rs gate" 描述的实际是在 `preflight.rs::dispatch_preflight` 里加分支,我们 Step 1.6 已经做了。Spec 这一处描述会在 Task 1 完成后顺手 patch 一下(见 Task 4)。

---

## Task 2: 抽出 `acp::AcpClient`(纯重构,hermes 行为不变)

**目标:** 把 hermes/mod.rs 里的 ACP wire 部分(initialize / session 生命周期 / parse / pending RPC 表 / usage accumulator)迁到新 `acp/` 子模块,hermes 改成调 `AcpClient`,**对外行为完全不变**,所有现有 hermes tests 仍绿。

**Files:**
- Create: `crates/gitim-agent-provider/src/acp/mod.rs`
- Create: `crates/gitim-agent-provider/src/acp/parse.rs`
- Modify: `crates/gitim-agent-provider/src/hermes/mod.rs`
- Modify: `crates/gitim-agent-provider/src/lib.rs`

**Reference:** 现有 `crates/gitim-agent-provider/src/hermes/mod.rs`(749 行)+ multica `hermes.go`(1422 行,只看 ACP transport 部分用作 cross-check)。

### Step 2.1: Baseline — 跑现有 hermes 全部测试,记 pass 计数

- [ ] **Run hermes tests + capture count**

```bash
cargo test -p gitim-agent-provider hermes 2>&1 | tail -5
```

Expected: 全 pass。**记下来 pass 数字**(例如 "23 passed"),Task 2 完成后这个数字必须一致。

### Step 2.2: Inventory — 找出 hermes/mod.rs 哪些是 ACP wire,哪些是 hermes-specific

- [ ] **Categorize every top-level fn / impl / type in `hermes/mod.rs`** (用 `grep -n "^pub fn\|^fn \|^impl \|^pub struct" hermes/mod.rs`)

预期分类:

| 项 | 归属 |
|---|---|
| `HermesProvider` struct + impl `Provider` | **hermes** (保留) |
| `HERMES_YOLO_MODE` env 注入 | **hermes** |
| `prompt_memory`/`prompt_reset_protocol`/`prompt_cold_start`/`prompt_identity`/`prompt_collaboration`/`prompt_gitim_api` override | **hermes** |
| `self_managed_context = true`、`usage_is_cumulative = true` | **hermes** |
| `build_prompt_payload(prompt)` | **hermes**(包 ACP `{ type: "text", text }` 的 hermes 风格 prompt;kimi 也用一个类似但简单的版本) |
| `detect_api_failure(output)` | **hermes**(把这个迁到 `acp/parse.rs` 但只 hermes 调) |
| `ParsedNotification` enum + `parse_notification(params)` | **ACP** |
| `parse_acp_usage(v)` + `parse_acp_usage_for_test` | **ACP** |
| `drive_session(...)` 大函数体里的:JSON-RPC 请求 / pending map / session/new / session/resume / session/prompt / session/update notification dispatch / usage accumulator | **ACP**(抽进 `AcpClient`) |
| `drive_session(...)` 的:spawn `hermes acp`、设 env、kill_on_drop、`build_prompt_payload(prompt)` 包装、`detect_api_failure(final_output)` 兜底升级 | **hermes**(留在 `hermes/mod.rs::execute()`) |
| `try_send_event` 辅助 | **ACP**(共用) |

**Note**: 边界不是完美的 —— 比如 `pending RPC 表` 是 ACP 协议的,但 `current ChildStdin` 是 hermes execute() 持有的进程。`AcpClient` 接受 `ChildStdin` 作为 ctor 参数,这条线就清楚了。

### Step 2.3: Create `acp/parse.rs` — 迁 ParsedNotification + parse_notification + parse_acp_usage + detect_api_failure 及其 tests

- [ ] **Create `crates/gitim-agent-provider/src/acp/parse.rs`**

把 `hermes/mod.rs` 里以下符号**整段**剪贴到 `acp/parse.rs`,visibility 改 `pub`,所有 `#[cfg(test)] mod tests` 块一并搬:

- `pub enum ParsedNotification { Text, Thinking, ToolCall, ToolResult, Usage }`
- `pub fn detect_api_failure(output: &str) -> Option<String>`
- `pub fn build_prompt_payload(prompt: &str) -> String` —— **不要迁**,这是 hermes-specific(SOUL.md guidance);留在 hermes/mod.rs
- `pub fn parse_notification(params: &Value) -> Option<ParsedNotification>`
- `pub fn parse_acp_usage_for_test(v: &Value) -> Option<ProviderUsage>` + 内部 `parse_acp_usage` —— 把 `parse_acp_usage` 改成 `pub fn`,删掉 `_for_test` 重命名 wrapper(它就是 wrapper,不需要)
- 所有相关的 `#[cfg(test)]` tests

  在 `acp/parse.rs` 头部加 module doc:
  ```rust
  //! ACP wire-level parsing — pure, stateless functions decoding the
  //! JSON-RPC notification payloads `hermes` and `kimi` both emit.
  //!
  //! See multica/server/pkg/agent/hermes.go for the upstream Go decoder
  //! this is translated from.
  ```

- [ ] **Create `crates/gitim-agent-provider/src/acp/mod.rs` skeleton**

```rust
//! ACP (Agent Client Protocol) JSON-RPC transport shared between
//! `hermes` and `kimi` providers.
//!
//! See docs/plans/kimi-cursor-providers/00-requirements.md §"会话管理模型"
//! for the per-execute() lifecycle reasoning.

pub mod parse;

// AcpClient struct + impl will land in Step 2.4.
```

- [ ] **Wire `acp` into lib.rs**

```rust
// crates/gitim-agent-provider/src/lib.rs
pub mod acp;
```

- [ ] **Delete the migrated symbols from `hermes/mod.rs`** and `use crate::acp::parse::*` instead:

```rust
// At top of hermes/mod.rs imports:
use crate::acp::parse::{
    detect_api_failure, parse_acp_usage, parse_notification, ParsedNotification,
};
```

- [ ] **Run hermes tests — should still be `<previous count>` passed**

```bash
cargo test -p gitim-agent-provider hermes
cargo test -p gitim-agent-provider acp::parse
```

Expected: hermes pass count unchanged from Step 2.1, plus acp::parse tests (which are the migrated ones, same count overall).

### Step 2.4: 写 `AcpClient` 的 struct + 钩子定义

- [ ] **Add to `crates/gitim-agent-provider/src/acp/mod.rs`**

```rust
//! ACP (Agent Client Protocol) JSON-RPC transport shared between
//! `hermes` and `kimi` providers.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, warn};

use crate::{Event, ProviderError, ProviderUsage};

pub mod parse;

/// Hook table — caller-provided customization points that
/// differentiate hermes / kimi while sharing the transport.
pub struct AcpHooks {
    /// Map an ACP tool title ("Read file: …" / "read:" / "patch (replace)")
    /// into the snake_case identifier the UI/runtime expects.
    pub tool_name_mapper: fn(&str) -> String,
    /// `None` by default — let every session/update notification through.
    /// `Some(predicate)` returns false for notifications that should be
    /// dropped (e.g. kiro-style "current-turn-only" semantics, not needed
    /// for hermes/kimi in v1 but kept on the type so adding kiro later
    /// is a one-line construction).
    pub accept_notification: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// Per-`execute()` ACP transport bound to one provider sub-process'
/// stdin/stdout. **Not** retained across turns — runtime owns the
/// opaque session_token, every turn spawns fresh.
pub struct AcpClient {
    provider_name: &'static str,
    stdin: Mutex<ChildStdin>,
    next_id: Mutex<i64>,
    pending: Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>,
    pending_tools: Mutex<HashMap<String, PendingToolCall>>,
    usage: Mutex<ProviderUsage>,
    hooks: AcpHooks,
}

#[derive(Debug)]
pub enum JsonRpcResponse {
    Ok(Value),
    Err { code: i64, message: String },
}

#[derive(Debug)]
pub(crate) struct PendingToolCall {
    pub tool: String,
    pub input: Value,
}

/// Outcome of `session/prompt` — captures the terminal stop_reason and
/// the cumulative usage snapshot the response carries.
#[derive(Debug)]
pub struct PromptOutcome {
    pub stop_reason: String,
    pub usage: ProviderUsage,
}

impl AcpClient {
    pub fn new(provider_name: &'static str, stdin: ChildStdin, hooks: AcpHooks) -> Self {
        Self {
            provider_name,
            stdin: Mutex::new(stdin),
            next_id: Mutex::new(1),
            pending: Mutex::new(HashMap::new()),
            pending_tools: Mutex::new(HashMap::new()),
            usage: Mutex::new(ProviderUsage::default()),
            hooks,
        }
    }

    /// Send a JSON-RPC request, await its response. Caller must drive
    /// the stdout reader concurrently (typically via a tokio::spawn'd
    /// task calling `handle_line` for each line) so responses can be
    /// matched to their pending oneshot senders.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        // Implementation: bump next_id, register pending entry, write
        // {"jsonrpc":"2.0","id":N,"method":...,"params":...}\n to stdin,
        // await the oneshot, map Err variant to ProviderError.
        // Migrate from hermes/mod.rs `drive_session` body.
        todo!("Step 2.5 fills this in")
    }

    /// Process one line read from the provider's stdout — either a
    /// response to a pending request, or a `session/update` notification
    /// that turns into Event::*. The stream reader task should call this
    /// for every line.
    pub async fn handle_line(&self, line: &str, event_tx: &mpsc::Sender<Event>) {
        todo!("Step 2.5 fills this in")
    }

    pub async fn initialize(&self) -> Result<(), ProviderError> {
        let _ = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": { "name": "gitim-agent", "version": env!("CARGO_PKG_VERSION") },
                    "clientCapabilities": {},
                }),
            )
            .await?;
        Ok(())
    }

    /// Returns the session id assigned by the provider.
    pub async fn new_session(&self, cwd: &str) -> Result<String, ProviderError> {
        let res = self
            .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await?;
        extract_session_id(&res).ok_or_else(|| {
            ProviderError::Protocol(format!(
                "{}: session/new returned no session ID",
                self.provider_name
            ))
        })
    }

    /// Returns `(actual_session_id, was_changed_from_requested)`. The
    /// provider may hand back a different id if the requested one
    /// expired — caller should log when `was_changed = true`.
    pub async fn resume_session(
        &self,
        cwd: &str,
        requested: &str,
    ) -> Result<(String, bool), ProviderError> {
        let res = self
            .request(
                "session/resume",
                json!({ "cwd": cwd, "sessionId": requested }),
            )
            .await?;
        let actual = extract_session_id(&res).unwrap_or_else(|| requested.to_string());
        Ok((actual.clone(), actual != requested))
    }

    /// Switch the active model. Kimi calls this between session/new and
    /// the first prompt when ExecOptions::model is set.
    pub async fn set_session_model(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<(), ProviderError> {
        self.request(
            "session/set_model",
            json!({ "sessionId": session_id, "modelId": model_id }),
        )
        .await?;
        Ok(())
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        payload: &str,
    ) -> Result<PromptOutcome, ProviderError> {
        let res = self
            .request(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": payload }],
                }),
            )
            .await?;
        let stop_reason = res
            .get("stopReason")
            .and_then(|v| v.as_str())
            .unwrap_or("end_turn")
            .to_string();
        let usage = parse::parse_acp_usage(&res).unwrap_or_default();
        Ok(PromptOutcome { stop_reason, usage })
    }

    /// Drain accumulated usage at session end.
    pub async fn finalize_usage(&self) -> ProviderUsage {
        self.usage.lock().await.clone()
    }
}

fn extract_session_id(v: &Value) -> Option<String> {
    v.get("sessionId").and_then(|x| x.as_str()).map(String::from)
}
```

- [ ] **Build to verify struct compiles** (函数 stub 还在 `todo!()`,但类型应该 typecheck)

```bash
cargo build -p gitim-agent-provider 2>&1 | tail -20
```

Expected: build PASS (`todo!()` 不阻止 compile)。

### Step 2.5: 把 hermes/mod.rs `drive_session` 里的 ACP wire 实装迁到 `AcpClient::request` / `handle_line`

这一步是核心重构,工作量较大。算法:

- [ ] **In `hermes/mod.rs` 找 `drive_session` 函数体的以下逻辑,翻译到 `AcpClient::request`**:
  - 自增 request id
  - 注册 pending oneshot
  - 把 JSON-RPC 请求 serialize 后 `stdin.write_all` + `flush` + 写 `\n`
  - await pending receiver
  - 把 `JsonRpcResponse::Err` 映射成 `ProviderError::Protocol`

- [ ] **`AcpClient::handle_line` 实装**:
  ```rust
  // Pseudocode — adapt from hermes/mod.rs lines you found in Step 2.2
  let v: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => return };
  if let Some(id) = v.get("id").and_then(|x| x.as_i64()) {
      // Response to a pending request
      if let Some(tx) = self.pending.lock().await.remove(&id) {
          let _ = tx.send(if let Some(err) = v.get("error") {
              JsonRpcResponse::Err {
                  code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                  message: err.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
              }
          } else {
              JsonRpcResponse::Ok(v.get("result").cloned().unwrap_or(Value::Null))
          });
      }
      return;
  }
  if v.get("method").and_then(|m| m.as_str()) == Some("session/update") {
      if let Some(should_accept) = &self.hooks.accept_notification {
          if !should_accept() { return; }
      }
      if let Some(params) = v.get("params") {
          if let Some(parsed) = parse::parse_notification(params) {
              dispatch_parsed(parsed, &self.hooks, event_tx, &self.usage, &self.pending_tools).await;
          }
      }
  }
  ```

  `dispatch_parsed` 是个私有 helper,把 `ParsedNotification` enum 各 variant 翻成 `Event::*`,顺便:
    - `ParsedNotification::ToolCall { tool, call_id, input }` → 用 `self.hooks.tool_name_mapper(&tool)` 映射 tool 名,记入 `pending_tools`,发 `Event::ToolUse`
    - `ParsedNotification::Usage(u)` → 更新 `self.usage`,发 `Event::Usage` (with provider's current session id —— 需要从 caller 传进来或者 AcpClient 持有 session_id state;**简化决定**:`AcpClient` 加 `current_session_id: Mutex<Option<String>>`,`new_session/resume_session` 写入它,`handle_line` 读它)

- [ ] **Run hermes tests again — they should still pass**

```bash
cargo test -p gitim-agent-provider hermes
```

Expected: 跟 Step 2.1 一致的 pass 数。

### Step 2.6: Refactor `hermes/mod.rs::execute` 改成调 `AcpClient`

- [ ] **Replace `hermes/mod.rs::drive_session` 内的 ACP wire 业务**

新的 `execute()` body 算法:
1. spawn `hermes acp` child (跟之前一致,设 `HERMES_YOLO_MODE=1` env)
2. take stdin, stdout, stderr
3. 构 `let hooks = AcpHooks { tool_name_mapper: hermes_tool_name_from_title, accept_notification: None }`
4. `let client = Arc::new(AcpClient::new("hermes", stdin, hooks))`
5. spawn 个 task A: 读 stdout 行,对每行调 `client.handle_line(&line, &event_tx).await`
6. spawn 个 task B (driver): `client.initialize() → client.new_session/resume_session(cwd, requested?) → client.prompt(session_id, build_prompt_payload(prompt))` → 收 PromptOutcome → 累 usage → 拿 `client.finalize_usage()` + `detect_api_failure(output)` 兜底 → 发 `result_tx`
7. cancel_token / abort_handle 同之前

`hermes_tool_name_from_title` 是 hermes 已有的 fn(在 hermes/mod.rs 内),保留。

- [ ] **Verify hermes tests still pass**

```bash
cargo test -p gitim-agent-provider hermes
```

Expected: 仍是 Step 2.1 的 pass 数。**如果有 regression,回到 Step 2.5 检查 dispatch_parsed 逻辑**。

### Step 2.7: Commit Task 2(Approach A 的第 2 个 atomic commit)

- [ ] **Verify build + scoped tests**

```bash
cargo build -p gitim-agent-provider
cargo test -p gitim-agent-provider
```

Expected: PASS。Hermes + ACP 模块全绿。

- [ ] **Commit**

```bash
git add -A crates/gitim-agent-provider/src/acp/ crates/gitim-agent-provider/src/hermes/mod.rs crates/gitim-agent-provider/src/lib.rs
git status     # 没有意外文件
git commit -m "$(cat <<'EOF'
refactor(provider): extract acp::AcpClient shared by hermes (and kimi)

Hermes 的 ACP wire(initialize / session lifecycle / parse / pending RPC
表 / usage accumulator) 抽到 crates/gitim-agent-provider/src/acp/ 子模块,
hermes/mod.rs 改成 thin wrapper:仍然负责 spawn 子进程、设 HERMES_YOLO_MODE
env、注入 hermes-specific prompt blocks、final detect_api_failure 兜底。

行为对外完全不变 —— 所有 hermes tests 仍绿。Step 1 抽 parse 模块时
parse_notification / parse_acp_usage / detect_api_failure / ParsedNotification
平移到 acp/parse.rs。

AcpHooks::accept_notification 字段留作未来扩展(kiro 的 current-turn-only
语义),v1 hermes 和 kimi 都设 None。

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Kimi Provider 实装

**目标:** 用 `acp::AcpClient` 实装 kimi,通 `create()` / `preflight` 三处 wiring。一个 atomic commit。

**Files:**
- Create: `crates/gitim-agent-provider/src/kimi/mod.rs`
- Modify: `crates/gitim-agent-provider/src/lib.rs`
- Modify: `crates/gitim-agent-provider/src/provider.rs`
- Modify: `crates/gitim-runtime/src/preflight.rs`

**Multica reference:** [`multica/server/pkg/agent/kimi.go`](../../../../../../code-skills/.repos/multica/server/pkg/agent/kimi.go) (403 行) + [`kimi_test.go`](../../../../../../code-skills/.repos/multica/server/pkg/agent/kimi_test.go) (211 行)。

### Step 3.1: 写 `kimi_tool_name_from_title` 失败测试

- [ ] **Create `crates/gitim-agent-provider/src/kimi/mod.rs` with mapper + tests, but skeletal Provider impl**

```rust
//! Kimi Code CLI provider — ACP transport via shared acp::AcpClient.
//!
//! Spec: docs/plans/kimi-cursor-providers/00-requirements.md §"Kimi 设计"
//! Reference: multica/server/pkg/agent/kimi.go

use async_trait::async_trait;

use crate::{ExecOptions, Provider, ProviderConfig, ProviderError, Session};

pub struct KimiProvider {
    config: ProviderConfig,
}

impl KimiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Provider for KimiProvider {
    /// ACP `session/prompt` response usage is session-cumulative — runtime
    /// subtracts the baseline in `AgentState.last_session_usage`.
    fn usage_is_cumulative(&self) -> bool {
        true
    }

    /// Kimi has no in-loop compression like hermes — let runtime own the
    /// `[[RESET]]` channel and occupancy preamble, same as claude/codex.
    fn self_managed_context(&self) -> bool {
        false
    }

    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        unimplemented!("Step 3.3 fills this in")
    }
}

/// Map an ACP tool title emitted by Kimi's CLI into the snake_case
/// identifier the UI expects. Kimi uses capitalised titles like
/// "Read file: /path" — strip everything after the first colon,
/// then case-match the prefix. See multica/kimi.go:367-409.
pub(crate) fn kimi_tool_name_from_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return String::new();
    }
    let prefix = match t.find(':') {
        Some(i) => t[..i].trim(),
        None => t,
    };
    let normalized = prefix.to_lowercase();
    match normalized.as_str() {
        "read" | "read file" => "read_file".to_string(),
        "write" | "write file" => "write_file".to_string(),
        "edit" | "patch" => "edit_file".to_string(),
        "shell" | "bash" | "terminal" | "run command" | "run shell command" => "terminal".to_string(),
        "search" | "grep" | "find" => "search_files".to_string(),
        "glob" => "glob".to_string(),
        "web search" => "web_search".to_string(),
        "fetch" | "web fetch" => "web_fetch".to_string(),
        "todo" | "todo write" => "todo_write".to_string(),
        _ => prefix.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cases adapted from multica/server/pkg/agent/kimi_test.go
    // TestKimiToolNameFromTitle.
    #[test]
    fn maps_read_file() {
        assert_eq!(kimi_tool_name_from_title("Read file: /tmp/foo.txt"), "read_file");
        assert_eq!(kimi_tool_name_from_title("Read"), "read_file");
    }

    #[test]
    fn maps_write_file() {
        assert_eq!(
            kimi_tool_name_from_title("Write file: /tmp/bar.txt"),
            "write_file"
        );
    }

    #[test]
    fn maps_edit_and_patch() {
        assert_eq!(kimi_tool_name_from_title("Edit file: foo"), "edit_file");
        assert_eq!(kimi_tool_name_from_title("Patch (replace)"), "edit_file");
    }

    #[test]
    fn maps_shell_variants() {
        for t in [
            "Shell command: ls",
            "Bash: pwd",
            "Terminal: echo",
            "Run command: gcc",
            "Run shell command: make",
        ] {
            assert_eq!(kimi_tool_name_from_title(t), "terminal", "input: {t}");
        }
    }

    #[test]
    fn maps_search_grep_find() {
        for t in ["Search: foo", "Grep: bar", "Find: baz"] {
            assert_eq!(kimi_tool_name_from_title(t), "search_files", "input: {t}");
        }
    }

    #[test]
    fn maps_glob_web_todo() {
        assert_eq!(kimi_tool_name_from_title("Glob: **/*.rs"), "glob");
        assert_eq!(kimi_tool_name_from_title("Web search: rust"), "web_search");
        assert_eq!(kimi_tool_name_from_title("Web fetch: https://"), "web_fetch");
        assert_eq!(kimi_tool_name_from_title("Todo write"), "todo_write");
    }

    #[test]
    fn empty_returns_empty() {
        assert_eq!(kimi_tool_name_from_title(""), "");
        assert_eq!(kimi_tool_name_from_title("   "), "");
    }

    #[test]
    fn unknown_falls_through_to_prefix() {
        assert_eq!(
            kimi_tool_name_from_title("Custom tool: arg"),
            "Custom tool"
        );
    }

    #[test]
    fn provider_trait_flags() {
        let p = KimiProvider::new(ProviderConfig::default());
        assert!(p.reports_usage());
        assert!(p.usage_is_cumulative());
        assert!(!p.self_managed_context());
    }
}
```

- [ ] **Wire kimi mod into `lib.rs`**

```rust
// crates/gitim-agent-provider/src/lib.rs
pub mod kimi;
```

- [ ] **Run mapper tests**

```bash
cargo test -p gitim-agent-provider kimi::tests
```

Expected: 9 tests pass (mapper + provider_trait_flags)。

### Step 3.2: Wire kimi 进 `create()` 和 `preflight::dispatch_preflight`

- [ ] **Modify `crates/gitim-agent-provider/src/provider.rs::create()`**

在 `"opencode"` 后、`"pi"` 前(字母序),加:

```rust
        "kimi" => Ok(Box::new(crate::kimi::KimiProvider::new(config))),
```

- [ ] **Build**

```bash
cargo build -p gitim-agent-provider
```

Expected: PASS(execute() 还 unimplemented,但 trait obj 能构造)。

### Step 3.3: 实装 `KimiProvider::execute`

参考 multica/kimi.go:33-356 翻译。算法:
1. resolve bin (default `"kimi"`)
2. spawn `kimi acp` child,**不**设 `HERMES_YOLO_MODE`
3. take stdin, stdout, stderr
4. `let hooks = AcpHooks { tool_name_mapper: kimi_tool_name_from_title, accept_notification: None }`
5. `let client = Arc::new(AcpClient::new("kimi", stdin, hooks))`
6. spawn task A: 读 stdout 行 → `client.handle_line(&line, &event_tx).await`
7. spawn driver task B:
   - `client.initialize()`
   - if `opts.resume_token.is_some()`:`client.resume_session(cwd, &requested)` → `(actual_id, changed)`,changed=true 时 warn log
   - else:`client.new_session(cwd)` → session_id
   - if `opts.model.is_some()`:`client.set_session_model(&session_id, &model)` —— **失败必须 fail task**,return `ExecResult::failed` with session_token
   - 构 `user_text = match opts.system_prompt { Some(sp) => format!("{sp}\n\n---\n\n{prompt}"), None => prompt }`
   - `client.prompt(&session_id, &user_text)` → PromptOutcome
   - 收尾:status by stop_reason / cancel / timeout;usage = `client.finalize_usage().await`(merge with PromptOutcome.usage)
   - 发 `result_tx`
8. 返 `Session::new(events, result, abort_handle, cancel_token)`

> 这一步内容多,建议实施者把 `hermes/mod.rs::execute` 摆在旁边作模板,只改三处差异:(1) bin 名、(2) 不设 HERMES_YOLO_MODE、(3) `set_session_model` 这一步。

- [ ] **Implement execute() body in `crates/gitim-agent-provider/src/kimi/mod.rs`**

(Skeleton 略 — refer to refactored `hermes/mod.rs::execute` as the template, ~200 行 inline)

- [ ] **Build + run all kimi tests**

```bash
cargo build -p gitim-agent-provider
cargo test -p gitim-agent-provider kimi
```

Expected: All kimi tests pass。Note:无 e2e test(spawn 真实 `kimi`),只覆盖 mapper + provider_trait_flags。

### Step 3.4: 加 `preflight_kimi_with_config` 到 preflight.rs

参考 `preflight_hermes_with`(`preflight.rs:1035+`)的结构 —— 它也是 ACP-based,启子进程跑 initialize+prompt 跑 hello。

但 kimi 的 preflight 比 hermes 简单(没有 default profile 解析、没有 hermes_home),所以更接近 claude 的扁平形态。

- [ ] **Add `DEFAULT_BIN_KIMI` const + `preflight_kimi_with_config` fn**

```rust
// Near other DEFAULT_BIN_* consts
const DEFAULT_BIN_KIMI: &str = "kimi";

/// Preflight Kimi CLI with a real ACP "say hi" hello.
///
/// Flow:
/// 1. `kimi --version` to capture version.
/// 2. Spawn `kimi acp` and drive minimal ACP lifecycle:
///    initialize → new_session → (set_session_model if model set) →
///    prompt("say hi") → wait for stop_reason → kill.
/// 3. Map missing-binary / timeout / non-zero exit to ErrorKind.
pub async fn preflight_kimi_with_config(
    bin: &str,
    timeout: Duration,
    overrides: PreflightOverrides,
) -> PreflightResult {
    // Implementation modeled on preflight_hermes_with at preflight.rs:1035+,
    // but with no hermes_home / default-profile resolution. Spawn `kimi acp`,
    // write JSON-RPC frames to its stdin, read responses from stdout.
    //
    // Because preflight.rs is part of gitim-runtime (not gitim-agent-provider),
    // and we want to avoid pulling acp::AcpClient into runtime's deps, write
    // the JSON-RPC frames inline — preflight only needs initialize +
    // session/new + session/prompt, ~50 lines of write/read.
    //
    // (See existing preflight_hermes_with for the exact pattern — it does
    // the same thing for hermes.)
    todo!("Implementer: model on preflight_hermes_with, ~120 lines")
}
```

> 这里有意 `todo!` 占位 —— `preflight_hermes_with` 现有 ~270 行,kimi preflight 应该是其简化版(去掉 hermes_home / default-profile / 模型解析的几条分支),实施者直接 copy + simplify。

- [ ] **Add `kimi_bin` to PreflightDispatchOverrides**

```rust
#[derive(Debug, Clone, Default)]
pub struct PreflightDispatchOverrides {
    // ...existing fields...
    pub kimi_bin: Option<String>,         // NEW
}
```

- [ ] **Add kimi branch in `dispatch_preflight`**

```rust
        "kimi" => {
            let bin = overrides.kimi_bin.as_deref().unwrap_or(DEFAULT_BIN_KIMI);
            preflight_kimi_with_config(bin, inner_timeout, prov_overrides).await
        }
```

放在 `"hermes" =>` 后、`"cursor" =>` 前(字母序),`other =>` 之前。

- [ ] **Build + run scoped tests**

```bash
cargo build -p gitim-runtime
cargo test -p gitim-runtime preflight
```

Expected: PASS。

### Step 3.5: Commit Task 3(Approach A 的第 3 个 atomic commit)

- [ ] **Final commit**

```bash
git add -A crates/gitim-agent-provider/src/kimi/ crates/gitim-agent-provider/src/lib.rs crates/gitim-agent-provider/src/provider.rs crates/gitim-runtime/src/preflight.rs
git status     # verify nothing else snuck in
git commit -m "$(cat <<'EOF'
feat(kimi): port kimi-cli ACP provider via shared acp::AcpClient

KimiProvider 复用上一 commit 抽出来的 acp::AcpClient,只提供 kimi-
specific 差异:(1) 启 `kimi acp` 而非 `hermes acp`,不设 HERMES_YOLO_MODE
env;(2) kimi_tool_name_from_title 解析 capitalised "Read file: …" titles
(hermes 的 mapper 处理 lowercase "read:" titles);(3) opts.model 非空时
在 prompt 前调 set_session_model,失败 fail task(不静默 fallback)。

Provider flags: usage_is_cumulative=true(ACP 行为),self_managed_context
=false —— runtime 接管 [[RESET]] 通道,跟 claude/codex 一致。

Wire:
- create() match 加 "kimi"
- preflight_kimi_with_config + kimi_bin override + dispatch_preflight branch
- http.rs 无需改动(走 preflight_for_add_request 间接 dispatch)

Tool name mapper test cases 跟 multica kimi_test.go::TestKimiToolNameFromTitle
对齐。

Frontend PROVIDER_IDS 仍不动 — webui dropdown 暴露见 spec out-of-scope。

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Spec patch + 全量验证

### Step 4.1: Patch `00-requirements.md` 把 "http.rs gate" 这段改成 "preflight.rs dispatch_preflight 分支"

实施过程中我们发现 `http.rs::add_agent` 已经把 dispatch 委托给 `preflight_for_add_request`,所以新加 provider 不需要碰 `http.rs`。Spec 里 §"http.rs gate" 那段措辞需要 patch。

- [ ] **Edit `docs/plans/kimi-cursor-providers/00-requirements.md`**

把这段:

```
### http.rs gate

`add_agent` 在 `handler_conflict` 检查后、`provision_agent` 之前的 server-side gate 加:

```rust
"cursor" => preflight_cursor_with_config(&env, opts.model.as_deref()).await,
"kimi"   => preflight_kimi_with_config(&env, opts.model.as_deref()).await,
```
```

改成:

```
### preflight dispatch 分支

`add_agent` 的 server-side preflight gate 通过 `preflight_for_add_request`
委托给 `preflight::dispatch_preflight`,所以新 provider 只需在
`dispatch_preflight` 加 match 分支:

```rust
"cursor" => preflight_cursor_with_config(&overrides.cursor_bin..., inner_timeout, prov_overrides).await,
"kimi"   => preflight_kimi_with_config(&overrides.kimi_bin..., inner_timeout, prov_overrides).await,
```

`http.rs` 不需要任何改动。`PreflightDispatchOverrides` 加 `cursor_bin` /
`kimi_bin` 两个 test seam 字段。
```

Same 对 §"In-scope" 列表里 `crates/gitim-runtime/src/http.rs` 那行,改成"`preflight.rs` 的 `dispatch_preflight` 函数",这样 spec 就跟实施现状一致了。

### Step 4.2: 全量验证

- [ ] **Run full test suite**

```bash
cargo test 2>&1 | tail -20
```

Expected: Pass count >= baseline from Step 1.1 + the new cursor/kimi tests added in this plan。

> 如果出现 baseline 之外新红的 test,先 diff 看是否是 hermes 重构 regression。如果是,回 Task 2 排查 `AcpClient::handle_line` 的 dispatch 逻辑。如果不是(比如 daemon 集成测试因为环境/网络挂),记下来,跟用户确认能否 ignore。

### Step 4.3: Commit spec patch + 最终状态

- [ ] **Commit spec patch**

```bash
git add docs/plans/kimi-cursor-providers/00-requirements.md
git commit -m "$(cat <<'EOF'
docs: correct kimi+cursor spec to match implementation reality

http.rs 不需要碰 — add_agent server gate 通过 preflight_for_add_request
间接 dispatch,新 provider 只在 preflight::dispatch_preflight 加 match
分支即可。In-scope 列表 + §"http.rs gate" 一并 patch 跟实施一致。

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Final state**

```bash
git log --oneline -8
```

Expected commit chain(由近到远):
```
[hash7] docs: correct kimi+cursor spec ...
[hash6] feat(kimi): port kimi-cli ACP provider ...
[hash5] refactor(provider): extract acp::AcpClient ...
[hash4] feat(cursor): port cursor-agent provider ...
[hash3] feat(cursor): add stream-json envelope parser   ← intermediate sub-commit from Task 1
[hash2] docs: rename design doc to match plan-dir convention
[hash1] docs: clarify session model + fix cursor args
[hash0] docs: design plan to port kimi + cursor providers ...
```

---

## Plan 完成后的 follow-up(本 plan **不**做)

- **Frontend PROVIDER_IDS 暴露**:加 `cursor` 和 `kimi` 到 `products/gitim/frontend/src/lib/providers.ts::PROVIDER_IDS`,decide each provider's `models` list,改 `add-agent-dialog.tsx` —— **后续 plan**。
- **e2e `#[ignore]` hello tests**:`cursor::tests::e2e_hello` + `kimi::tests::e2e_hello` —— 需要本机装 CLI,**后续 plan**。
- **Cursor `step_finish` 事件细粒度上报到 SSE**:v1 只在 ExecResult 终值用,**后续看实战需求加**。
- **Kimi 的 detect_api_failure 嗅探**:multica 在 stderr 装了 ACP provider error sniffer,kimi v1 沿用 hermes 现有 `detect_api_failure`(纯 final output 字符串嗅探),只 hermes 调,不嗅 stderr。**未来 kimi 实测发现同类问题再开关**。
- **Copilot / Kiro 两个 provider**:**单独立 plan**(参考本 plan 的结构)。

---

## Self-Review

(Already covered inline — see Step 1.6 note on cursor preflight tests being skipped vs Task 4 spec patch addressing the http.rs misdescription. No remaining placeholders. Type signatures consistent: `AcpClient::new(provider_name, stdin, hooks)`, `AcpHooks { tool_name_mapper: fn, accept_notification: Option<Arc<...>> }`, `PromptOutcome { stop_reason, usage }` referenced consistently across Task 2 / Task 3.)
