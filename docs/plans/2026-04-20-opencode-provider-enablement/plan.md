# OpenCode Provider Enablement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 放开 opencode provider 的端到端支持：修复坏掉的 CLI 调用（不存在的 `--prompt` flag）、允许 opencode 不指定 model、runtime 放行、前端可选择。同时把所有 provider 按目录结构小重构一遍。

**Architecture:**
- **System prompt 注入路径**：用 `OPENCODE_CONFIG_CONTENT` 环境变量内联一个名为 `gitim` 的 agent（携带我们的 system prompt），CLI 端用 `--agent gitim` 选中。不污染用户 workspace 文件系统，不修改 `~/.config/opencode/`。
- **Model 默认**：`ExecOptions.model = None` 时不传 `--model`，让 opencode 用用户 `opencode auth login` 时选的默认。前端对 opencode 不再强制选 model。
- **Provider 目录重构**：`src/<name>.rs` → `src/<name>/mod.rs`。保留公共路径（`gitim_agent_provider::opencode::parse_line` 等）不变，外部测试不 break。
- **Preflight**：沿用 claude/codex 的 real-hello 模式；opencode preflight 同样用 OPENCODE_CONFIG_CONTENT 注入一个只会 echo 的 preflight agent，跑用户默认 model。

**Tech Stack:** Rust (tokio, serde_json), React 19 + Vite, axum HTTP handlers

---

## File Structure

### Crate `gitim-agent-provider` 重构

```
crates/gitim-agent-provider/src/
  lib.rs                 # pub mod 列表不变
  claude/mod.rs          # was claude.rs
  codex/mod.rs           # was codex.rs
  gemini/mod.rs          # was gemini.rs
  hermes/mod.rs          # was hermes.rs
  mock/mod.rs            # was mock.rs
  openclaw/mod.rs        # was openclaw.rs
  opencode/mod.rs        # was opencode.rs，且内容重写（T2）
  stubs/mod.rs           # was stubs.rs
  error.rs, provider.rs, prompts.rs, types.rs, util.rs  # 不动
```

**为什么不套 `providers/` 父目录**：Rust 的 `mod foo;` 自动解析 `foo.rs` 或 `foo/mod.rs`。直接把单文件 → 同名目录 + mod.rs 即可，public path 完全保持。用户提到"每个内可能之后会独立定制细节"，有了目录就可以未来加 `opencode/prompt.rs`、`opencode/parser.rs` 等 sub-modules。

### 修改的跨 crate 文件

- `crates/gitim-agent-provider/src/opencode/mod.rs` — 重写 execute()
- `crates/gitim-runtime/src/http.rs` — provider whitelist + 错误消息 + preflight dispatch
- `crates/gitim-runtime/src/preflight.rs` — 新增 preflight_opencode + preflight_opencode_with
- `webui-v2/src/lib/providers.ts` — 加 "opencode" 到 ProviderId 和 PROVIDERS
- `webui-v2/src/components/management/add-agent-dialog.tsx` — 支持 model 可选 provider

---

## Task 1: Refactor — move each provider file into its own directory

纯粹的文件移动，零行为变化。先做掉是因为后续 T2 的所有改动都在 `opencode/mod.rs`，重构先行让后续 diff 干净。

**Files:**
- Move: `crates/gitim-agent-provider/src/claude.rs` → `crates/gitim-agent-provider/src/claude/mod.rs`
- Move: `crates/gitim-agent-provider/src/codex.rs` → `crates/gitim-agent-provider/src/codex/mod.rs`
- Move: `crates/gitim-agent-provider/src/gemini.rs` → `crates/gitim-agent-provider/src/gemini/mod.rs`
- Move: `crates/gitim-agent-provider/src/hermes.rs` → `crates/gitim-agent-provider/src/hermes/mod.rs`
- Move: `crates/gitim-agent-provider/src/mock.rs` → `crates/gitim-agent-provider/src/mock/mod.rs`
- Move: `crates/gitim-agent-provider/src/openclaw.rs` → `crates/gitim-agent-provider/src/openclaw/mod.rs`
- Move: `crates/gitim-agent-provider/src/opencode.rs` → `crates/gitim-agent-provider/src/opencode/mod.rs`
- Move: `crates/gitim-agent-provider/src/stubs.rs` → `crates/gitim-agent-provider/src/stubs/mod.rs`

`lib.rs` 无需改动（`pub mod claude;` 会自动解析到 `claude/mod.rs`）。

- [ ] **Step 1.1: 验证重构前测试全绿**

Run: `cargo test -p gitim-agent-provider --lib --tests 2>&1 | tail -20`
Expected: `test result: ok.` 所有测试通过。

- [ ] **Step 1.2: 为每个 provider 创建目录并移动文件**

```bash
cd crates/gitim-agent-provider/src
for name in claude codex gemini hermes mock openclaw opencode stubs; do
  mkdir -p "$name"
  git mv "${name}.rs" "${name}/mod.rs"
done
```

Run: `ls crates/gitim-agent-provider/src/`
Expected:
```
claude/  codex/  error.rs  gemini/  hermes/  lib.rs  mock/  openclaw/  opencode/  prompts.rs  provider.rs  stubs/  types.rs  util.rs
```

- [ ] **Step 1.3: 验证重构后测试全绿**

Run: `cargo test -p gitim-agent-provider --lib --tests 2>&1 | tail -20`
Expected: 全部通过。特别关注 `opencode_parse_test.rs` / `factory_test.rs` 等外部 tests 仍能解析 `gitim_agent_provider::opencode::parse_line` 路径。

如果 opencode/mod.rs 里 `mod tests` 内联测试引用了 `super::` 里的项，路径仍然有效（tests 是 mod.rs 的子 mod）。

- [ ] **Step 1.4: 跨 crate 编译检查**

Run: `cargo check --workspace 2>&1 | tail -20`
Expected: 无 error。

- [ ] **Step 1.5: Commit**

```bash
git add -A crates/gitim-agent-provider/src
git commit -m "refactor(provider): move each provider into its own directory

Preserves public API paths (gitim_agent_provider::opencode::parse_line etc.)
so external tests don't break. Each provider now has its own dir for future
sub-files (prompt.rs, parser.rs, config.rs).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Fix opencode provider — inject system prompt via OPENCODE_CONFIG_CONTENT

删掉根本不存在的 `--prompt` flag。把 system prompt 装进一个临时 `gitim` agent，通过 `OPENCODE_CONFIG_CONTENT` 注入。model 为 None 时不传 `--model`。

**Files:**
- Modify: `crates/gitim-agent-provider/src/opencode/mod.rs`
- Test: `crates/gitim-agent-provider/tests/opencode_args_test.rs` (new)

### 背景（engineer 必读）

- `opencode run --help` 输出证实：**没有 `--prompt` 参数**。当前代码传 `--prompt <sys>` 会被 yargs 当 unknown argument 报错。
- opencode 注入 system prompt 的唯一 CLI 路径：`--agent <name>`。agent 定义来自
  - `.opencode/agent/<name>.md`（frontmatter + body），或
  - config JSON 的 `agent.<name>.prompt` 字段
- `OPENCODE_CONFIG_CONTENT` 环境变量接受**完整 config JSON 字符串**（见 opencode 源码 `config.ts:577`），作为最后一层 merge。所以可以内联定义 agent。
- agent config 里 `mode: "primary"` 让它可在 `opencode run` 顶层使用。

### 预期命令形态

有 system prompt 时：
```
OPENCODE_CONFIG_CONTENT={"agent":{"gitim":{"prompt":"<sys>","mode":"primary"}}}
OPENCODE_PERMISSION={"*":"allow"}
opencode run --format json --dangerously-skip-permissions --agent gitim [--model <m>] [--session <id>] -- "<user msg>"
```

没有 system prompt 时：
```
OPENCODE_PERMISSION={"*":"allow"}
opencode run --format json --dangerously-skip-permissions [--model <m>] [--session <id>] -- "<user msg>"
```

注意点：
- `--dangerously-skip-permissions` 加上，否则 CLI 默认会 auto-reject 所有 permission ask（external dir 访问、.env 读取等）。原来的实现靠 `OPENCODE_PERMISSION={"*":"allow"}` 压平权限，两者叠加是安全冗余——permission merge 后已经 allow，但 CLI 的 auto-reject 逻辑（run.ts:549）基于 `--dangerously-skip-permissions` 而非 permission 结果，所以必须加。
- `--` 分隔符确保 message 不被当 flag 解析（message 如果以 `-` 开头会出问题）。
- message 以 positional 数组传，yargs 会用空格 join——多个 positional 效果等价于一个。为避免 shell quoting 误解，我们用单个 positional 传整条消息。

### Step 细节

- [ ] **Step 2.1: 写命令构造器单元测试（failing）**

为了能测试命令参数，把构造 args 和 env 的逻辑抽成纯函数。新建 `crates/gitim-agent-provider/tests/opencode_args_test.rs`:

```rust
use gitim_agent_provider::opencode::build_invocation;
use gitim_agent_provider::ExecOptions;
use std::time::Duration;

#[test]
fn no_system_prompt_no_agent_flag() {
    let opts = ExecOptions {
        system_prompt: None,
        model: None,
        resume_token: None,
        timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };
    let inv = build_invocation("hello", &opts);
    assert!(inv.args.contains(&"run".to_string()));
    assert!(inv.args.contains(&"--format".to_string()));
    assert!(inv.args.contains(&"json".to_string()));
    assert!(inv.args.contains(&"--dangerously-skip-permissions".to_string()));
    assert!(!inv.args.iter().any(|a| a == "--agent"));
    assert!(!inv.args.iter().any(|a| a == "--model"));
    assert!(!inv.env.contains_key("OPENCODE_CONFIG_CONTENT"));
    assert_eq!(inv.env.get("OPENCODE_PERMISSION").map(String::as_str), Some(r#"{"*":"allow"}"#));
    // message terminator + message as last positional
    let dash_pos = inv.args.iter().position(|a| a == "--").expect("-- present");
    assert_eq!(inv.args[dash_pos + 1], "hello");
}

#[test]
fn system_prompt_injects_gitim_agent_via_env() {
    let opts = ExecOptions {
        system_prompt: Some("you are gitim".to_string()),
        model: None,
        ..Default::default()
    };
    let inv = build_invocation("hello", &opts);
    let cfg = inv.env.get("OPENCODE_CONFIG_CONTENT").expect("config content set");
    let parsed: serde_json::Value = serde_json::from_str(cfg).unwrap();
    assert_eq!(parsed["agent"]["gitim"]["prompt"], "you are gitim");
    assert_eq!(parsed["agent"]["gitim"]["mode"], "primary");
    // --agent gitim present
    let idx = inv.args.iter().position(|a| a == "--agent").expect("--agent flag");
    assert_eq!(inv.args[idx + 1], "gitim");
}

#[test]
fn model_only_when_provided() {
    let with = ExecOptions {
        model: Some("anthropic/claude-sonnet-4-6".to_string()),
        ..Default::default()
    };
    let inv = build_invocation("x", &with);
    let idx = inv.args.iter().position(|a| a == "--model").expect("--model flag");
    assert_eq!(inv.args[idx + 1], "anthropic/claude-sonnet-4-6");
}

#[test]
fn resume_token_uses_session_flag() {
    let opts = ExecOptions {
        resume_token: Some("ses_abc123".to_string()),
        ..Default::default()
    };
    let inv = build_invocation("x", &opts);
    let idx = inv.args.iter().position(|a| a == "--session").expect("--session flag");
    assert_eq!(inv.args[idx + 1], "ses_abc123");
}

#[test]
fn prompt_flag_never_present() {
    // Regression: opencode run has no --prompt flag. Asserting absence.
    let opts = ExecOptions {
        system_prompt: Some("sys".to_string()),
        ..Default::default()
    };
    let inv = build_invocation("user msg", &opts);
    assert!(!inv.args.iter().any(|a| a == "--prompt"),
        "opencode run has no --prompt flag; system prompt must go through --agent + OPENCODE_CONFIG_CONTENT");
}
```

Run: `cargo test -p gitim-agent-provider --test opencode_args_test 2>&1 | tail -20`
Expected: FAIL with "unresolved import `gitim_agent_provider::opencode::build_invocation`"

- [ ] **Step 2.2: 实现 build_invocation 和重写 execute**

Edit `crates/gitim-agent-provider/src/opencode/mod.rs` — 替换现有 `execute()` 的实现，抽出可测纯函数 `build_invocation`。

在文件顶部保留 imports 不变的基础上，加：

```rust
use std::collections::HashMap;
use serde_json::json;
```

在文件末尾（parse_line 之前）插入：

```rust
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
    env.insert("OPENCODE_PERMISSION".to_string(), r#"{"*":"allow"}"#.to_string());

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
```

Replace the body of `impl Provider for OpencodeProvider::execute` to use `build_invocation`:

```rust
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

        // Apply provider-level env first so call-site can override.
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
        info!(pid, cwd = ?opts.cwd, model = ?opts.model, has_sys = opts.system_prompt.is_some(), "opencode started");

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
```

注意：原实现里 `cmd.env("OPENCODE_PERMISSION", ...)` 硬编码被移除，改为从 `inv.env` 统一应用；`self.config.env` 的 env 现在先应用，再被 `inv.env` 覆盖（如果有冲突）。这比原实现更可控——保证 OPENCODE_PERMISSION 和 OPENCODE_CONFIG_CONTENT 由 build_invocation 说了算。

- [ ] **Step 2.3: 运行单元测试验证通过**

Run: `cargo test -p gitim-agent-provider --test opencode_args_test 2>&1 | tail -20`
Expected: 5 tests passed.

- [ ] **Step 2.4: 确保 parse 测试仍然通过**

Run: `cargo test -p gitim-agent-provider --test opencode_parse_test 2>&1 | tail -20`
Expected: All passed.

- [ ] **Step 2.5: 整个 crate 测试**

Run: `cargo test -p gitim-agent-provider 2>&1 | tail -20`
Expected: All passed.

- [ ] **Step 2.6: Commit**

```bash
git add crates/gitim-agent-provider/src/opencode/mod.rs crates/gitim-agent-provider/tests/opencode_args_test.rs
git commit -m "fix(opencode): inject system prompt via OPENCODE_CONFIG_CONTENT

opencode run has no --prompt flag (only TUI does). The previous implementation
passed --prompt and was silently broken. System prompt is now injected as a
temporary 'gitim' agent via OPENCODE_CONFIG_CONTENT env + --agent gitim,
which is the only supported CLI path.

Also:
- Drop --model when opts.model is None, so user's opencode default is used
- Add --dangerously-skip-permissions (default is deny, which blocked tools)
- Use -- terminator so prompts starting with dash aren't parsed as flags
- Extract build_invocation() as a pure function for unit testing

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Runtime — whitelist + preflight for opencode

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:968-978` (provider whitelist), `:1647-1665` (unknown-provider error), `:1747-1756` (preflight dispatch)
- Modify: `crates/gitim-runtime/src/preflight.rs` (add preflight_opencode + preflight_opencode_with)

### Preflight 策略

跟 claude/codex 同样用 real-hello。opencode 特殊：我们不控制 provider/model。用户 `opencode auth login` 选的是什么 provider 就跑那个。我们能做的限制：
- 注入一个最短的 `gitim_preflight` agent（只说 "Reply with exactly what the user asks."）
- 让用户默认 model 跑一次，产出应包含 "GITIM_OK"
- tempdir cwd 隔离
- 60s 超时，`--dangerously-skip-permissions`，不需要任何工具

### Step 细节

- [ ] **Step 3.1: 写 preflight 失败分支测试（skeleton）**

Append to `crates/gitim-runtime/src/preflight.rs` 末尾的 `#[cfg(test)] mod tests` 里：

```rust
#[tokio::test]
async fn opencode_preflight_not_installed() {
    let result = super::preflight_opencode_with(
        "/nonexistent/opencode",
        std::time::Duration::from_secs(1),
    ).await;
    let v = serde_json::to_value(&result).unwrap();
    assert_eq!(v["provider"], serde_json::Value::String("opencode".into()));
    assert_eq!(v["available"], serde_json::Value::Bool(false));
    assert_eq!(v["error_kind"], serde_json::Value::String("not_installed".into()));
}

#[tokio::test]
async fn opencode_preflight_timeout() {
    // /bin/yes blocks forever; preflight must time out and classify correctly.
    let result = super::preflight_opencode_with(
        "/bin/yes",
        std::time::Duration::from_millis(200),
    ).await;
    let v = serde_json::to_value(&result).unwrap();
    assert_eq!(v["provider"], serde_json::Value::String("opencode".into()));
    assert_eq!(v["available"], serde_json::Value::Bool(false));
    assert_eq!(v["error_kind"], serde_json::Value::String("timeout".into()));
}
```

Run: `cargo test -p gitim-runtime --lib preflight::tests::opencode 2>&1 | tail -20`
Expected: FAIL with "cannot find function `preflight_opencode_with`".

- [ ] **Step 3.2: 实现 preflight_opencode_with 和 preflight_opencode**

参照 `preflight_claude_with` 的结构。在 `preflight.rs` 里紧跟 codex 相关函数之后插入：

```rust
/// Run a real-hello ping against the opencode CLI at `bin`.
///
/// Unlike claude/codex where we force a cheap model, opencode uses whatever
/// model the user authenticated with via `opencode auth login`. We cannot
/// predict that at preflight time, so we accept the variance. System prompt
/// is injected via OPENCODE_CONFIG_CONTENT as a minimal echo agent to keep
/// the request cheap and deterministic.
pub async fn preflight_opencode_with(bin: &str, timeout: Duration) -> PreflightResult {
    let started = Instant::now();

    let tmpdir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            return PreflightResult::failure(
                "opencode",
                ErrorKind::Other,
                format!("failed to create tempdir: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let config_content = serde_json::json!({
        "agent": {
            "gitim_preflight": {
                "prompt": "Reply with exactly what the user asks, nothing more.",
                "mode": "primary",
            }
        }
    }).to_string();

    let mut cmd = tokio::process::Command::new(bin);
    cmd.current_dir(tmpdir.path())
        .args([
            "run",
            "--format", "json",
            "--dangerously-skip-permissions",
            "--agent", "gitim_preflight",
            "--",
            "Reply with exactly: GITIM_OK",
        ])
        .env("OPENCODE_CONFIG_CONTENT", &config_content)
        .env("OPENCODE_PERMISSION", r#"{"*":"allow"}"#)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let kind = map_spawn_error(&e);
            let msg = if kind == ErrorKind::NotInstalled {
                format!("opencode CLI not found at `{bin}`: {e}")
            } else {
                format!("failed to spawn opencode: {e}")
            };
            return PreflightResult::failure(
                "opencode",
                kind,
                msg,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return PreflightResult::failure(
                "opencode",
                ErrorKind::Other,
                format!("opencode IO error: {e}"),
                started.elapsed().as_millis() as u64,
            );
        }
        Err(_) => {
            return PreflightResult::failure(
                "opencode",
                ErrorKind::Timeout,
                format!("opencode preflight exceeded {}ms", timeout.as_millis()),
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = truncate(stderr.trim(), STDERR_TRUNCATE);
        let msg = if trimmed.is_empty() {
            format!("opencode exited with status {}", output.status)
        } else {
            format!("opencode exited with status {}: {}", output.status, trimmed)
        };
        return PreflightResult::failure("opencode", ErrorKind::Other, msg, duration_ms);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = extract_opencode_text(&stdout);

    if text.contains("GITIM_OK") {
        PreflightResult::success(
            "opencode",
            None,
            None, // model used = whatever user authed; unknown here
            duration_ms,
            Some(truncate(&text, PREVIEW_TRUNCATE)),
        )
    } else {
        PreflightResult::failure(
            "opencode",
            ErrorKind::Other,
            "response did not contain GITIM_OK",
            duration_ms,
        )
    }
}

/// Run a real-hello preflight against the default `opencode` binary.
pub async fn preflight_opencode() -> PreflightResult {
    preflight_opencode_with("opencode", Duration::from_secs(60)).await
}

/// Concatenate all `text` part payloads from opencode's NDJSON stream.
fn extract_opencode_text(stdout: &str) -> String {
    let mut out = String::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let Ok(val): Result<serde_json::Value, _> = serde_json::from_str(line) else { continue };
        if val.get("type").and_then(|t| t.as_str()) != Some("text") { continue; }
        if let Some(text) = val.get("part").and_then(|p| p.get("text")).and_then(|t| t.as_str()) {
            out.push_str(text);
        }
    }
    out
}
```

Run: `cargo test -p gitim-runtime --lib preflight::tests::opencode 2>&1 | tail -20`
Expected: Both tests pass.

- [ ] **Step 3.3: 加 extract_opencode_text 单元测试**

在同一个 `mod tests` 里追加：

```rust
#[test]
fn extract_opencode_text_concatenates_text_parts() {
    let stdout = r#"
{"type":"step_start","sessionID":"s1","part":{}}
{"type":"text","sessionID":"s1","part":{"text":"GITIM_"}}
{"type":"text","sessionID":"s1","part":{"text":"OK"}}
{"type":"step_finish","sessionID":"s1","part":{}}
"#;
    assert_eq!(super::extract_opencode_text(stdout), "GITIM_OK");
}

#[test]
fn extract_opencode_text_ignores_non_text_lines() {
    let stdout = r#"
not json
{"type":"tool_use","part":{}}
{"type":"text","part":{"text":"hello"}}
"#;
    assert_eq!(super::extract_opencode_text(stdout), "hello");
}
```

Run: `cargo test -p gitim-runtime --lib preflight::tests::extract_opencode_text 2>&1 | tail -10`
Expected: 2 passed.

- [ ] **Step 3.4: 扩展 http.rs 的 provider whitelist**

Edit `crates/gitim-runtime/src/http.rs:969`:

```rust
// Before:
match req.provider.as_str() {
    "claude" | "codex" | "mock" => {}
// After:
match req.provider.as_str() {
    "claude" | "codex" | "opencode" | "mock" => {}
```

- [ ] **Step 3.5: 更新 unknown-provider error 消息**

Grep for the current error messages that hardcode "claude or codex"：

Run: `grep -n '"claude" or "codex"\|claude or codex\|p != "claude" && p != "codex"' crates/gitim-runtime/src/http.rs`
Expected: matches around line 1647-1665 and possibly 2220.

Edit those messages to include opencode in the supported set. 具体替换 http.rs:1649:

```rust
// Before:
"Missing \"provider\" in {}. Add \"provider\": \"claude\" or \"provider\": \"codex\" to the file and restart the runtime.",
// After:
"Missing \"provider\" in {}. Add \"provider\": \"claude\", \"codex\", or \"opencode\" to the file and restart the runtime.",
```

http.rs:1652-1653:

```rust
// Before:
Some(p) if p != "claude" && p != "codex" => Some(format!(
    "Unsupported provider \"{}\" in {}. Expected \"claude\" or \"codex\".",
// After:
Some(p) if p != "claude" && p != "codex" && p != "opencode" => Some(format!(
    "Unsupported provider \"{}\" in {}. Expected \"claude\", \"codex\", or \"opencode\".",
```

Run: `grep -rn "\"claude\" or \"codex\"\|claude or codex" crates/gitim-runtime/src/http.rs`
Expected: 无匹配（全部更新完）。

- [ ] **Step 3.6: 扩展 preflight dispatch**

Edit `crates/gitim-runtime/src/http.rs:1747`:

```rust
// Before:
match provider.as_str() {
    "claude" => {
        ...preflight_claude()...
    }
    "codex" => {
        ...preflight_codex()...
    }
    _ => ...
}
// After: add opencode arm mirroring the above
```

Run: `grep -n "preflight_claude()\|preflight_codex()" crates/gitim-runtime/src/http.rs` 找准上下文后加入：

```rust
        "opencode" => {
            let result = crate::preflight::preflight_opencode().await;
            (StatusCode::OK, Json(result)).into_response()
        }
```

紧跟 codex 之后、`_ =>` 之前。

- [ ] **Step 3.7: 更新另一处 provision error（http.rs:2220 附近）**

Run: `grep -n 'provider_str' crates/gitim-runtime/src/http.rs | head`
查看 2220 附近 match provider_str 的用法。如果那里也是只认 claude/codex 的 match，加上 opencode。具体看当前代码再决定——如果是调用 `gitim_agent_provider::create(provider, ...)`，provider 字符串直接透传即可，无需改动（create 已支持 opencode）。

- [ ] **Step 3.8: 整个 runtime 编译 + 测试**

Run: `cargo test -p gitim-runtime --lib 2>&1 | tail -20`
Expected: All passed.

Run: `cargo check --workspace 2>&1 | tail -10`
Expected: 无 error。

- [ ] **Step 3.9: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/preflight.rs
git commit -m "feat(runtime): allow opencode as provider end-to-end

- http.rs: add opencode to AgentAddRequest whitelist
- http.rs: update unknown-provider error message enum
- http.rs: add opencode case to /preflight/:provider dispatch
- preflight.rs: add preflight_opencode_with + preflight_opencode using
  real-hello against user's opencode-authed default model

Model is not forced at preflight (unlike claude/codex which pin a cheap
model). opencode picks whatever the user authenticated with; we accept
the cost variance because the CLI doesn't expose a way to list auth'd
providers without spawning.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Frontend — add opencode to provider list with optional model

**Files:**
- Modify: `webui-v2/src/lib/providers.ts`
- Modify: `webui-v2/src/components/management/add-agent-dialog.tsx`

### Step 细节

- [ ] **Step 4.1: 扩展 providers.ts**

Edit `webui-v2/src/lib/providers.ts`:

```typescript
// Detect pings a fixed cheap model in the runtime (claude-haiku-4-5 / gpt-5.4-mini),
// not the user's selected model — so a green check verifies CLI availability, not model availability.
export type ProviderId = "claude" | "codex" | "opencode";

export type PreflightErrorKind = "not_installed" | "timeout" | "other";

/**
 * Mirrors the `PreflightResult` struct emitted by gitim-runtime's preflight check.
 * Field names stay snake_case — this is the on-the-wire contract, not a style choice.
 */
export interface PreflightResult {
  available: boolean;
  provider: string;
  version: string | null;
  model_used: string | null;
  duration_ms: number;
  output_preview: string | null;
  error: string | null;
  error_kind: PreflightErrorKind | null;
}

export interface ProviderModel {
  id: string;
  label: string;
}

export interface ProviderInfo {
  label: string;
  models: ProviderModel[];
  /**
   * If true, Model selection is optional — the provider picks its own default
   * (e.g. opencode uses the user's `opencode auth login` default). Empty model
   * id is sent as undefined to the runtime.
   */
  modelOptional?: boolean;
}

export const PROVIDERS: Record<ProviderId, ProviderInfo> = {
  claude: {
    label: "Claude",
    models: [
      { id: "claude-sonnet-4-6", label: "Claude Sonnet 4.6" },
      { id: "claude-opus-4-7", label: "Claude Opus 4.7" },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5" },
    ],
  },
  codex: {
    label: "Codex",
    models: [
      { id: "gpt-5.4", label: "GPT-5.4" },
      { id: "gpt-5.3-codex", label: "GPT-5.3 Codex" },
    ],
  },
  opencode: {
    label: "OpenCode",
    models: [],
    modelOptional: true,
  },
};

export const PROVIDER_IDS: ProviderId[] = ["claude", "codex", "opencode"];
```

- [ ] **Step 4.2: 更新 add-agent-dialog.tsx 放开 model 可选**

Edit `webui-v2/src/components/management/add-agent-dialog.tsx`。三处改动：

**(a) 渲染 Model 选择区域——当 provider.modelOptional 时显示 "(opencode default)" 提示并隐藏 select**

找到 `<div className="space-y-1.5">` 里 `htmlFor="agent-model"` 的 label 区块（原在第 239-257 行附近），替换为：

```tsx
{provider && PROVIDERS[provider as ProviderId].modelOptional ? (
  <div className="space-y-1.5">
    <label className="text-sm font-medium">Model</label>
    <p className="text-xs text-muted-foreground">
      {PROVIDERS[provider as ProviderId].label} uses the default model from{" "}
      <code>opencode auth login</code>. No selection needed.
    </p>
  </div>
) : (
  <div className="space-y-1.5">
    <label className="text-sm font-medium" htmlFor="agent-model">
      Model
    </label>
    <select
      id="agent-model"
      value={model}
      onChange={(e) => setModel(e.target.value)}
      disabled={!provider}
      className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
    >
      <option value="">— Select model —</option>
      {availableModels.map((m) => (
        <option key={m.id} value={m.id}>
          {m.label}
        </option>
      ))}
    </select>
  </div>
)}
```

**(b) 提交校验：modelOptional 时不 require model**

在 `handleSubmit` 里第一条 if 条件（~第 102 行）：

```tsx
// Before:
if (
  !name.trim() ||
  validationError ||
  submitting ||
  !provider ||
  !model ||
  !detectResult?.available
)
  return;
// After:
const providerInfo = provider ? PROVIDERS[provider as ProviderId] : null;
const modelRequired = providerInfo ? !providerInfo.modelOptional : true;
if (
  !name.trim() ||
  validationError ||
  submitting ||
  !provider ||
  (modelRequired && !model) ||
  !detectResult?.available
)
  return;
```

**(c) 提交按钮 disabled 同步**

同 (b)，在 `<Button type="submit" disabled={...}>` 里用 `modelRequired && !model` 替换 `!model`。

**(d) switch provider 时清 model（原逻辑已对，可保留不动）**

- [ ] **Step 4.3: 人工检查 TypeScript 编译**

Run: `cd webui-v2 && pnpm tsc --noEmit 2>&1 | tail -20`
Expected: 无 error。

如果项目用 npm/bun 请相应替换命令：

Run: `cd webui-v2 && cat package.json | grep '"type-check\|typecheck"'`
然后用对应 script。

- [ ] **Step 4.4: Commit**

```bash
git add webui-v2/src/lib/providers.ts webui-v2/src/components/management/add-agent-dialog.tsx
git commit -m "feat(webui): add opencode provider to agent creation

- ProviderId now includes 'opencode'
- PROVIDERS.opencode declares modelOptional: true (empty models list)
- add-agent-dialog shows opencode as 'uses default model' instead of forcing
  a select, since opencode.ai picks the user's authed model
- Submit validation skips model requirement for modelOptional providers

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: End-to-end manual verification

**Files:**
- Create: `docs/plans/2026-04-20-opencode-provider-enablement/verification.md` (notes, not a test)

纯手工验证，记录步骤。无代码改动。

- [ ] **Step 5.1: 手动跑一次真实 opencode provision**

前提：`opencode auth login` 已经登过一个 provider。

```bash
# 1. 启动 gitim runtime
cargo run -p gitim-runtime -- serve &
RUNTIME_PID=$!
sleep 3

# 2. 通过 WebUI 创建 workspace（local 模式最简单），然后 add agent with provider=opencode
# 3. 观察 runtime 日志有没有 spawn opencode 进程 + 产生 NDJSON

# 4. 给 agent 发条消息，看 agent_loop 能不能正常处理

kill $RUNTIME_PID
```

把结果记录到 `verification.md`：ok / fail 的观察，stderr 摘录。

- [ ] **Step 5.2: 直接验证命令构造**

在 `$WORKTREE_PATH` 下：

```bash
OPENCODE_CONFIG_CONTENT='{"agent":{"gitim":{"prompt":"You are a grumpy cat.","mode":"primary"}}}' \
  opencode run --format json --dangerously-skip-permissions --agent gitim -- "Say meow in a grumpy tone." 2>&1 | tail -20
```

Expected: NDJSON 输出，assistant text 是只会回 meow 的 grumpy cat 语气，说明 prompt 注入生效。

- [ ] **Step 5.3: 验证不传 model 时 opencode 用自己的默认**

```bash
opencode run --format json --dangerously-skip-permissions -- "Reply with: hi" 2>&1 | head -50
```

Expected: 用户默认 model 回复 "hi"。

- [ ] **Step 5.4: Commit verification notes**

```bash
git add docs/plans/2026-04-20-opencode-provider-enablement/verification.md
git commit -m "docs(opencode): manual verification notes

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Checklist

- [x] **Spec coverage:**
  - [x] 修复 opencode.rs 的 --prompt → T2
  - [x] 通过 OPENCODE_CONFIG_CONTENT 注入 system prompt → T2
  - [x] 支持 model=None（用 opencode 默认）→ T2 + T4
  - [x] Provider 目录重构 → T1
  - [x] Frontend opencode 选项 → T4
  - [x] Runtime whitelist + preflight → T3
- [x] **No placeholders**: 每个 step 都有具体代码 / 命令 / 期望输出
- [x] **Type consistency**: `build_invocation` / `Invocation` / `extract_opencode_text` 名字贯穿全文
- [x] **File paths**: 绝对/相对路径均来自 `$WORKTREE_PATH`
- [x] **Test 独立性**: preflight timeout 测试用 `/bin/yes`（系统内置），不依赖 opencode 装机
