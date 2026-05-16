# Design: Port kimi + cursor providers from multica

## 背景

当前 `gitim-agent-provider` 后端有 7 个真实可用 provider:
`claude / codex / gemini / hermes / openclaw / opencode / pi`,加上一个 `mock` 测试 provider 和一个 `cursor` 占位 stub(`stubs::CursorProvider`,`execute()` 直接返 `NotImplemented`)。

参考实现 `~/ateam/code-skills/.repos/multica/server/pkg/agent/`(Go 端 happy-cli pattern)有 11 个 provider,比我们多 **copilot / cursor / kimi / kiro** 四个。

本次只搬两个:**kimi + cursor**。前端 `PROVIDER_IDS` 暂时不动 —— 后端 + CLI + preflight 通了就够,webui dropdown 后续再加。

## 决策

| 决策点 | 选择 | 备注 |
|---|---|---|
| 产出形态 | 可执行实现 + 编译过 | tests 覆盖核心 happy path,边角 case 后续补 |
| Kimi ACP 代码组织 | **抽 hermes ACP client 出来共享** | 新增 `acp/` 子模块,hermes 和 kimi 同用 |
| Cursor 实现 | 独立 stream-json 解析 | 跟 hermes/kimi 不重叠,完整新写 |
| Wiring 范围 | Provider trait + `create()` match + preflight | CLI `add-agent --provider kimi/cursor` 能 server-side gate 过 |
| 前端 | 不动 | `PROVIDER_IDS` 不加 kimi/cursor,webui dropdown 不暴露 |
| 执行顺序 | **Approach A — 分 3 个 atomic commit** | (1) cursor (2) hermes 重构 (3) kimi |

## 范围

### In-scope

- `crates/gitim-agent-provider/src/acp/` 新增 ACP transport 子模块
- `crates/gitim-agent-provider/src/hermes/mod.rs` 重构成调 `acp::AcpClient`,行为不变,现有 tests 仍绿
- `crates/gitim-agent-provider/src/cursor/mod.rs` 新增,替换 `stubs::CursorProvider`
- `crates/gitim-agent-provider/src/kimi/mod.rs` 新增
- `crates/gitim-agent-provider/src/lib.rs` 模块声明
- `crates/gitim-agent-provider/src/provider.rs` `create()` match 接通
- `crates/gitim-agent-provider/src/stubs/mod.rs` 删除(只剩一个 cursor stub,被实装替代后没剩内容)
- `crates/gitim-runtime/src/preflight.rs` 加 `preflight_cursor_with_config` 和 `preflight_kimi_with_config`
- `crates/gitim-runtime/src/http.rs` add_agent server-side preflight gate 加 cursor/kimi 分支

### Out-of-scope(v1)

- Copilot / Kiro 两个 provider —— 后续单独立 plan
- Frontend `PROVIDER_IDS` 暴露 —— 等后端跑通再加
- Cursor / Kimi 的高级流程:steering / mid-turn cancel 信号语义、stop_reason 的特殊处理(沿用默认)
- Cursor 的 `step_finish` 阶段事件细粒度上报到 SSE —— v1 只在 ExecResult 终值里用
- Kimi 的 ACP `session/cancel` 显式响应 —— 沿用 hermes 现有 cancel 行为

## 架构

### 文件改动地图

```
crates/gitim-agent-provider/src/
  acp/                          NEW.
    mod.rs                      AcpClient struct + initialize/session/prompt/dispatch
    parse.rs                    parse_notification, parse_acp_usage, detect_api_failure (从 hermes 迁出)
  cursor/                       NEW.
    mod.rs                      CursorProvider 实装
  kimi/                         NEW.
    mod.rs                      KimiProvider, 复用 acp::AcpClient
  hermes/mod.rs                 CHANGED. drive_session 改成调 acp::AcpClient
  stubs/mod.rs                  DELETED.
  provider.rs                   CHANGED. create() match 加 kimi, cursor 切到 crate::cursor
  lib.rs                        CHANGED. 声明 acp, cursor, kimi, 去掉 stubs

crates/gitim-runtime/src/
  preflight.rs                  CHANGED. + preflight_cursor_with_config, + preflight_kimi_with_config
  http.rs                       CHANGED. add_agent server-side gate dispatch
```

### `acp::AcpClient` 设计

```rust
pub struct AcpClient {
    provider_name: &'static str,         // "hermes" | "kimi" — logging / sniffer 前缀
    stdin: ChildStdin,
    pending: Mutex<HashMap<i64, oneshot::Sender<JsonRpcResult>>>,
    pending_tools: Mutex<HashMap<String, PendingToolCall>>,
    usage: Mutex<ProviderUsage>,         // session-accumulated
    hooks: AcpHooks,
}

pub struct AcpHooks {
    pub tool_name_mapper: fn(&str) -> String,
    pub accept_notification: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl AcpClient {
    pub async fn initialize(&self) -> Result<()>;
    pub async fn new_session(&self, cwd: &str) -> Result<String>;
    pub async fn resume_session(&self, cwd: &str, requested_id: &str) -> Result<(String, bool)>;
    pub async fn set_session_model(&self, session_id: &str, model_id: &str) -> Result<()>;
    pub async fn prompt(&self, session_id: &str, payload: &str) -> Result<PromptOutcome>;
    pub async fn handle_line(&self, line: &str, event_tx: &mpsc::Sender<Event>);
}

// Pure parsing (无状态, 供 unit test 直接调).
pub fn parse_notification(params: &Value) -> Option<ParsedNotification>;
pub fn parse_acp_usage(v: &Value) -> Option<ProviderUsage>;
pub fn detect_api_failure(output: &str) -> Option<String>;
```

**Hermes 怎么用**:
- `hermes/mod.rs::execute` 仍然负责 spawn `hermes acp` 子进程、设 `HERMES_YOLO_MODE=1`、kill_on_drop。
- 把 stdin/stdout pipe 交给 `AcpClient::new(provider_name = "hermes", hooks = hermes_hooks())`。
- driver loop:`initialize → new_session/resume → prompt → 收 promptDone`。

**Kimi 怎么用**:
- `kimi/mod.rs::execute` spawn `kimi acp`,**不**设 `HERMES_YOLO_MODE`。
- driver loop 比 hermes 多一步:`opts.model` 非空 → 调 `set_session_model(model)` —— 失败必须 fail task(不静默 fallback)。
- `hooks.tool_name_mapper = kimi_tool_name_from_title`(capitalised "Read file: …" 解析)。

**取舍**:
- `tool_name_mapper` 用函数指针 `fn(&str) -> String`,无状态,简单
- `accept_notification` 用 `Option<Arc<dyn Fn>>`,v1 hermes/kimi 都不用,留口子给未来的 "current-turn-only" 场景(kiro)
- 错误嗅探(`newACPProviderErrorSniffer` in multica)v1 沿用 hermes 现有 `detect_api_failure`,放 `acp/parse.rs`

### Cursor 设计

- bin = `cursor-agent`,args = `--output-format stream-json -p <prompt>`,可选 `--resume <session_id>` / `--model <model>`
- 事件 → GitIM Event 映射:

| Multica cursor 事件 | GitIM Event |
|---|---|
| `system{subtype=init}` | `Status{status:"running"}` |
| `system{subtype=error}` | `Error{content}` |
| `assistant{message:{content:[text]}}` | `Text{content}` |
| `assistant{message:{content:[thinking]}}` | `Thinking{content}` |
| `assistant{message:{content:[tool_use]}}` | `ToolUse{tool, call_id, input}` |
| `user{message:{content:[tool_result]}}` | `ToolResult{call_id, output}` |
| `step_finish{usage}` | accumulate to step_usage (内部状态) |
| `result{session_id, usage?, error?}` | session_token = session_id;`result.usage` 优先,fallback step_usage;status 由 error/`subtype` 决定 |

- Provider trait override:
  - `reports_usage() = true`
  - `usage_is_cumulative() = false` —— `step_finish` 是 per-step delta;`result.usage` 是 session-cumulative,但 cursor 一次 `execute()` 只有一个 prompt turn,所以 cumulative = turn,runtime baseline 不会错算
  - `self_managed_context() = false` —— 让 runtime 接管 [[RESET]]
- prompt_* 全部默认(file-memory + reset 设定跟 claude 一致)

### Kimi 设计

- bin = `kimi`,args = `acp` + 用户 custom_args
- env:不设 `HERMES_YOLO_MODE`,只透传 `ProviderConfig.env`
- ACP 流:
  1. `initialize` (protocolVersion=1, clientInfo, clientCapabilities)
  2. `new_session` 或 `resume_session`
  3. `opts.model` 非空 → `set_session_model(session_id, model_id)`,失败 fail task
  4. `prompt(session_id, [{type:text, text:userText}])` —— 如果 `opts.system_prompt` 非空,拼成 `system_prompt + "\n\n---\n\n" + prompt` 当 userText
  5. 等 promptDone,收尾时 cancel runCtx → drain stdin/stderr → 报 ExecResult
- Provider trait override:
  - `reports_usage() = true`
  - `usage_is_cumulative() = true` —— ACP `session/prompt` 响应的 usage 是 session-cumulative,跟 hermes 一致;runtime 走 baseline subtraction
  - `self_managed_context() = false` —— kimi 不像 hermes 有 in-loop compression
- prompt_* 全部默认(不像 hermes override `prompt_memory` / `prompt_reset_protocol`,kimi 没有 SOUL.md / MEMORY.md 那类自管理)

### Tool name mapper(kimi)

照搬 multica `kimiToolNameFromTitle` 的 case 表:

```rust
fn kimi_tool_name_from_title(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() { return String::new(); }
    let t = match t.find(':') { Some(i) => t[..i].trim(), None => t };
    match t.to_lowercase().as_str() {
        "read" | "read file" => "read_file",
        "write" | "write file" => "write_file",
        "edit" | "patch" => "edit_file",
        "shell" | "bash" | "terminal" | "run command" | "run shell command" => "terminal",
        "search" | "grep" | "find" => "search_files",
        "glob" => "glob",
        "web search" => "web_search",
        "fetch" | "web fetch" => "web_fetch",
        "todo" | "todo write" => "todo_write",
        _ => t,
    }.to_string()
}
```

## Preflight

按 `feedback_preflight_real_hello` —— preflight 必须跑真实 hello,不只 `--version`。

### `preflight_cursor_with_config`

1. `which cursor-agent`(executable path 可由 `env["GITIM_CURSOR_EXEC"]` 或默认 `cursor-agent`)
2. `cursor-agent --version` 抓 version string
3. 启短 hello:`cursor-agent -p "say hi" --output-format stream-json [--model X if 给了]`,90s 超时
4. 第一个 `assistant` text content 或 `result` event 到达 → PASS。kill 进程。
5. 失败 → `error_kind` 映射 `not_installed` / `timeout` / `other`,带 stderr / stdout 前 N 字符当 `output_preview`

### `preflight_kimi_with_config`

1. `which kimi`
2. `kimi --version` 抓 version
3. 启动 `kimi acp` 子进程,acp client 走 `initialize → new_session → (if opts.model: set_session_model) → prompt("say hi")`
4. 第一个 text content notification 到达 → PASS。kill。
5. error_kind 同上

### http.rs gate

`add_agent` 在 `handler_conflict` 检查后、`provision_agent` 之前的 server-side gate 加:

```rust
"cursor" => preflight_cursor_with_config(&env, opts.model.as_deref()).await,
"kimi"   => preflight_kimi_with_config(&env, opts.model.as_deref()).await,
```

## 测试

### Unit(`cargo test` 默认跑)

- `acp::parse::tests::*` —— 把现有 `hermes/mod.rs` 里 `parse_notification` / `parse_acp_usage` 的测试迁出来,放 `acp/parse.rs` 内联 mod tests
- `cursor::tests::parse_event_*` —— 几个 stream-json envelope 形状(system init / assistant text+tool_use / user tool_result / step_finish / result)
- `kimi::tests::tool_name_from_title_*` —— 跟 multica `kimi_test.go` 的 case 表一致
- hermes 现有所有 tests 必须仍绿(重构验证)

### `#[ignore]`(手动跑,需要本机装 CLI)

- `cursor::tests::e2e_hello` —— 起真实 cursor-agent
- `kimi::tests::e2e_hello` —— 起真实 kimi

## 执行顺序(Approach A,3 个 atomic commit)

### Commit 1 — Cursor

- 新增 `crates/gitim-agent-provider/src/cursor/mod.rs`
- 删除 `crates/gitim-agent-provider/src/stubs/`(只有一个 cursor stub)
- `lib.rs`:`mod cursor;`,去掉 `mod stubs;`
- `provider.rs::create()`:`"cursor" => Ok(Box::new(crate::cursor::CursorProvider::new(config)))`
- `preflight.rs` + `http.rs` 加 cursor 分支
- 内联 tests
- **验证**:`cargo test -p gitim-agent-provider` + `cargo test -p gitim-runtime --test preflight`

### Commit 2 — Hermes 重构(纯重构)

- 新增 `crates/gitim-agent-provider/src/acp/{mod.rs, parse.rs}`
- 把 hermes/mod.rs 的 ACP wire 部分(initialize / session 生命周期 / parse / pending RPC 表 / usage accumulator)迁到 `acp/`
- hermes/mod.rs 改成调 `acp::AcpClient`,行为对外不变
- hermes 现有 tests 迁的 parse tests 也跟着移到 `acp/parse.rs` 模块
- `lib.rs`:`mod acp;`
- **验证**:hermes 全套 tests 仍绿,行为无 regression

### Commit 3 — Kimi

- 新增 `crates/gitim-agent-provider/src/kimi/mod.rs`
- 用 `acp::AcpClient`,接通 `kimi acp` 启动 / tool-name mapper / `set_session_model`
- `lib.rs`:`mod kimi;`
- `provider.rs::create()`:`"kimi" => ...`
- `preflight.rs` + `http.rs` 加 kimi 分支
- 内联 tests
- **验证**:`cargo test -p gitim-agent-provider -p gitim-runtime`

## Non-goals 显式记录

- **不暴露到 frontend dropdown** — `PROVIDER_IDS` 不加,webui add-agent dialog 不动。webui 用户暂时无法选 kimi/cursor。CLI 用户可以:`gitim-runtime add-agent --provider kimi --model <…>`。
- **不做 Copilot / Kiro** — 单独立 plan,本次范围外。
- **不重新设计 `Provider` trait 接口** — `reports_usage` / `usage_is_cumulative` / `self_managed_context` / 8 个 `prompt_*` 接口照用现状。
- **不改 `ExecOptions` / `ExecResult` / `ProviderUsage` 字段集** — Cursor 和 Kimi 的字段集都能在现有结构里表达。
- **不动 `ProviderConfig.executable_path` 解析** — `None` → 用默认 bin 名(`cursor-agent` / `kimi`),`Some` → 用指定 path。跟 hermes 一致。

## 风险 / 待定项

- Cursor stream-json 的 envelope 形状是看 `multica/server/pkg/agent/cursor.go` 推断的,真实运行时可能跟 multica 那边的版本有 drift —— 实施期 Commit 1 跑真实 cursor-agent 时如有 mismatch,再调字段。
- Kimi 的 `session/set_model` 失败语义:multica 这边是 fail task,我们沿用。如果 kimi 在某些 case 下 `set_model` 不必要也不会失败(比如已经在用相同 model),这是 kimi CLI 自己处理。
- `accept_notification` 钩子留作未来扩展 — v1 hermes 和 kimi 都不需要。
- `acp::AcpClient` 抽出后,hermes 自己的 `detect_api_failure` 失败嗅探也迁到 `acp/parse.rs`,但只有 hermes 调,kimi v1 暂不调(kimi 没有把上游 LLM HTTP 错误吞到 assistant text 这个观察)。如果 kimi 实测有同类问题,再开关给 kimi。
