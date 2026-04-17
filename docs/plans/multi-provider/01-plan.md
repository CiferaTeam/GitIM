# 多 Provider 支持（Claude + Codex）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 WebUI 在创建 Agent 时能选 Claude 或 Codex provider，并提供"Detect"按钮跑一次真实 `hello` 验证本地 CLI 可用性；底层 runtime 通过新路由 `GET /preflight/{provider}` 暴露检测能力，Agent 配置里 `provider` 字段强制必填，recover 时缺字段明确进入 `error` 状态并推送 SSE。

**Architecture:** preflight 逻辑以并列函数形式加到 `gitim-runtime::preflight` 模块（不扩展 Provider trait），HTTP 路由用 path param `/preflight/{provider}` 调度到 `preflight_claude` / `preflight_codex`，每个函数在临时 cwd 下用最小上下文跑 CLI（Claude `--setting-sources "" --tools "" --system-prompt`、Codex `exec --json`），60s 超时，验证输出含 `GITIM_OK`。前端 `AddAgentDialog` 加 provider 选择 + Detect 按钮，成功前 Add 按钮硬阻塞；模型清单前端硬编码。`AgentInfo` 新增 `error_message` 字段承载 recover 失败说明，前端 `agent-card` 错误态扩展展示。

**Tech Stack:** Rust (tokio / axum / tempfile / serde) · React 19 / TypeScript / Radix UI / Zustand · Playwright E2E · TDD inline `#[cfg(test)]` for preflight unit tests · real-CLI gated E2E for provider 集成

**约定：**
- 本 plan 遵循用户偏好 `plan_no_code`：只写分工、文件、验收，不写代码
- TDD 节奏：先红（写失败测试）→ 绿（实现）→ commit。每任务可独立 commit
- 工作目录：`/Users/lewisliu/ateam/GitIM/.worktrees/multi-provider`
- 分支：`feature/multi-provider`
- 实施期间所有 cargo / pnpm / git 操作都在上面 worktree 目录下执行

---

## Decisions Summary（来自 grill-me + plan-eng-review）

| # | 决策 | 结论 |
|---|------|------|
| Q1 | Tier-1 provider | `claude` + `codex`（其他 provider 后端存留、UI 不露） |
| Q2 | Preflight API | `GET /preflight/{provider}` path param，跑真实 hello |
| Q3 | Preflight CLI 形式 | Prompt=`Reply with exactly: GITIM_OK`，便宜模型（claude-haiku-4-5 / gpt-5.4-mini），60s timeout |
| Q3 | Preflight 响应 shape | `{available, provider, version?, model_used, duration_ms, output_preview, error?, error_kind?}` |
| Q4a | Detect 按钮粒度 | 单按钮，跟随当前 provider 选择 |
| Q4b | 阻塞 Add | **硬阻塞** — Detect 成功前 Add disabled |
| Q4c | 检测时机 | 纯手动点击触发 |
| Q4d | Detect 状态寿命 | Dialog 关闭即丢 |
| Q5a | 模型清单位置 | 全前端硬编码（新文件 `webui-v2/src/lib/providers.ts`） |
| Q5b | 模型清单 | Claude: `claude-sonnet-4-6` / `claude-opus-4-6` / `claude-haiku-4-5`；Codex: `gpt-5.4` / `gpt-5.3-codex`；默认空 |
| Q5c | Preflight 便宜模型 | Claude=`claude-haiku-4-5`；Codex=`gpt-5.4-mini` |
| Q6a | 默认 provider | 无默认（Dialog 打开 provider 选择器为空） |
| Q6b | executable_path UI | v1 不露，仅走 PATH |
| Q6c | error_kind 分类 | 3 类：`not_installed` / `timeout` / `other` |
| Q6d | 向后兼容 | **不兼容** — `provider` 字段强制必填；旧数据无字段 → error 状态 |
| Q7 | 配置作用域 | 本地 per-clone（`.gitim/me.json` git 忽略），不走 git 同步 |
| Q8 | Recover 缺 provider | 登记为 `status: "error"` + `error_message` 携带修复提示 + SSE 推送 |
| Arch#1 | `/preflight/claude` 兼容 | 删除老路由（无前端 consumer） |
| Arch#2 | 成本节流 | 仅 UI 按钮防抖；不加后端缓存 |
| Arch#6 | Preflight 实现位置 | `gitim-runtime/src/preflight.rs` 里并列函数（不扩展 Provider trait） |
| Arch#7 | Error 消息承载 | `AgentInfo.error_message: Option<String>` + SSE event_type="error" |

### Claude preflight recipe（验证过，$0.0019/次）
`claude --print --model claude-haiku-4-5 --output-format json --setting-sources "" --tools "" --system-prompt "Reply with exactly what the user asks." "Reply with exactly: GITIM_OK"`，cwd 设为 tempdir。

### Codex preflight recipe（验证过，~$0.002/次）
`codex exec --json --model gpt-5.4-mini "Reply with exactly: GITIM_OK"`，cwd 设为 tempdir，stdin null。

---

## 任务依赖图

```
T1 preflight.rs 骨架 ─┬─→ T2 claude preflight (TDD) ─┐
                      ├─→ T3 codex preflight (TDD)  ─┤
                      └─→ T4 error_kind 映射          ─┤
                                                       │
                                                       ▼
                                        T5 HTTP /preflight/{provider}（删老路由）
                                                       │
T6 POST /agents/add 强制 provider ────────────────────┤
T7 AgentInfo 加 error_message ──→ T8 recover error 分支 ──→ T9 SSE error 推送
                                                       │
T10 webui providers.ts ─→ T11 client.ts ─→ T12 Dialog provider 选择 ─→ T13 Detect 按钮 + Add 阻塞
                                                       │
T14 Agent 类型 + agent-card 展示 error_message（依赖 T7）
                                                       │
                                    ┌──────────────────┴──────────────────┐
                                    ▼                  ▼                  ▼
                      T15 E2E preflight        T16 E2E 双 provider     T17 E2E UI Detect
                      (依赖 T5)                (依赖 T5+T6)             (依赖 T13)
                                                       │
                                                       ▼
                                                T18 手动 QA + 检查清单
```

**并行度：**
- T1 前置所有 preflight 任务
- T2 / T3 / T4 可同时进行（不同函数）
- T6 / T7 独立于 preflight 模块（后端 HTTP 侧）
- T10 / T11 前端启动链
- T15 / T16 / T17 最后一层可并行（不同测试文件）

---

## Task 1: gitim-runtime preflight 模块骨架

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs`（重构顶部：新增 `PreflightResult` 结构体、`ErrorKind` 枚举；保留现有 `check_env` / `query_version` 不动）
- Modify: `crates/gitim-runtime/src/lib.rs`（如需要 re-export）

**变更描述：**
- 新增 `pub struct PreflightResult` 对应 HTTP 响应 shape 的 Rust 端结构，字段：`available: bool`、`provider: String`、`version: Option<String>`、`model_used: Option<String>`、`duration_ms: u64`、`output_preview: Option<String>`、`error: Option<String>`、`error_kind: Option<ErrorKind>`
- 新增 `pub enum ErrorKind { NotInstalled, Timeout, Other }`，`Serialize` 为 snake_case
- `PreflightResult` 实现构造助手 `fn success(...)` 与 `fn failure(kind, error, ...)` 方便后续函数短路
- 不动既有 `check_env` / `VersionMismatch` / `check_claude`（check_claude 在 T2 里替换语义）
- 在文件顶部加模块 doc comment 说明 "Preflight module: real-hello CLI verification. Used by /preflight/{provider} HTTP endpoint."

**验收标准：**
- `cargo build -p gitim-runtime` 通过
- `cargo test -p gitim-runtime` 不破坏（现有 preflight 相关测试可能为空 — 允许保持空）
- `PreflightResult` 的 JSON 序列化 snapshot 符合 Decisions Summary 里的 shape（可用一个 trivial serde 单元测试验证）

**Steps:**
- [ ] Step 1：在 preflight.rs 顶部加 `PreflightResult` + `ErrorKind`
- [ ] Step 2：加一个 `#[cfg(test)] mod tests` 验证 serde 输出
- [ ] Step 3：`cargo test -p gitim-runtime preflight`
- [ ] Step 4：commit `feat(runtime): add PreflightResult + ErrorKind skeleton`

---

## Task 2: gitim-runtime preflight::preflight_claude（TDD）

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs`

**变更描述：**
- 新增 `pub async fn preflight_claude() -> PreflightResult`
- 语义：
  1. 在系统 tempdir 下创建唯一子目录作为 cwd（用 `tempfile::TempDir` — 若 Cargo.toml 未引入 `tempfile` 需要加 dev-dep 或 dep）
  2. 用 `tokio::process::Command` 拼参数跑 Claude：`--print` / `--model claude-haiku-4-5` / `--output-format json` / `--setting-sources ""` / `--tools ""` / `--system-prompt "Reply with exactly what the user asks."` / prompt=`"Reply with exactly: GITIM_OK"`，stdin null，stderr piped
  3. `tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())` 包装
  4. 退出码非 0 且 stderr 含 "Not logged in" 或 "authentication" / "ENOENT" → `available=false`，`error_kind=Other`，`error` 携带 stderr trim
  5. stdout 解析为 JSON（可能是单对象或数组；参照 `claude --output-format json` 实测行为），提取 `result` 字段
  6. 若 `result` 字符串包含 `GITIM_OK` → `available=true`，填 `output_preview`（截断 200 字符）、`duration_ms`、`model_used="claude-haiku-4-5"`
  7. 否则 → `available=false`，`error_kind=Other`，`error="response did not contain GITIM_OK"`
  8. 二进制不存在（`child.spawn()` 立即失败且 IO kind 是 `NotFound`）→ `error_kind=NotInstalled`
  9. Timeout → `error_kind=Timeout`

**测试（先写红）：**
- 测 `preflight_claude` 通过将 `PATH` 临时指向空目录 + 设置 `CLAUDE_EXEC_OVERRIDE`（或注入 binary name）模拟"claude 不存在"→ `error_kind=NotInstalled`。为测试可注入性，把函数拆成 `pub async fn preflight_claude_with(bin: &str) -> PreflightResult`，再包一个默认 `preflight_claude()` 调用 `"claude"`。公开 `_with` 版本为 `pub(crate)` 以便测试
- 测 `preflight_claude_with("/bin/false")` → available=false，error_kind=Other（退出码非 0）
- 测 `preflight_claude_with("/bin/true")` → 输出为空，解析失败 → error_kind=Other，error 说明 "empty output" 或 "parse failed"
- 测超时：用 `/bin/sh -c "sleep 120"` 作为 bin（但这会让测试跑 60s — 改用显式 `timeout=Duration::from_millis(200)` 的测试专用变体 `preflight_claude_with_timeout`）
- **集成测试（`#[ignore]`）：** 跑真实 claude，assert available=true 且 output_preview 含 `GITIM_OK`。放在 `crates/gitim-runtime/tests/preflight_claude.rs` 外部测试文件

**验收标准：**
- `cargo test -p gitim-runtime preflight_claude -- --nocapture` 单元测试全绿（不含 ignore）
- `cargo test -p gitim-runtime --test preflight_claude -- --ignored --nocapture` 手动跑通（需要本地已登录 Claude）
- 每个错误分支都有对应测试
- `tempfile` 依赖如需新增，只加到 `gitim-runtime` 的 `[dependencies]`

**Steps:**
- [ ] Step 1：在 `crates/gitim-runtime/tests/preflight_claude.rs` 写 5 个单元测试（NotInstalled / exit-nonzero / empty-output / timeout / success-ignored），全部先失败
- [ ] Step 2：`cargo test -p gitim-runtime preflight_claude` → 确认 5 个失败
- [ ] Step 3：在 preflight.rs 实现 `preflight_claude_with` + `preflight_claude`
- [ ] Step 4：若需要 `tempfile` 加 Cargo.toml
- [ ] Step 5：`cargo test -p gitim-runtime preflight_claude`（不含 ignored）全绿
- [ ] Step 6：`cargo test -p gitim-runtime --test preflight_claude -- --ignored` 手动验证真实跑通
- [ ] Step 7：commit `feat(runtime): add preflight_claude with real-hello verification`

---

## Task 3: gitim-runtime preflight::preflight_codex（TDD）

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs`
- Create: `crates/gitim-runtime/tests/preflight_codex.rs`

**变更描述：**
- 新增 `pub async fn preflight_codex()` 与 `pub(crate) async fn preflight_codex_with(bin: &str)`
- 语义差异（相对 claude）：
  1. 命令：`codex exec --json --model gpt-5.4-mini` + prompt=`"Reply with exactly: GITIM_OK"`
  2. 解析 stdout：逐行 JSONL，找到 `"type":"item.completed"` 且 `"item.type":"agent_message"` 的行，读 `text` 字段
  3. 寻找 `"type":"turn.completed"` 作为 "完成" 信号
  4. stderr 可能含 `ERROR codex_core_skills::loader: ...`（symlink 警告，和我们无关），不要据此判定失败
  5. 没收到 `turn.completed` → `error_kind=Other`，error="codex stream ended without turn.completed"
  6. 成功则 model_used="gpt-5.4-mini"

**测试结构：** 镜像 T2 的 5 个 case。集成测试文件 `crates/gitim-runtime/tests/preflight_codex.rs`，`#[ignore]` 标注真实跑。

**验收标准：**
- 单元测试 4 个分支全绿
- 集成测试 `--ignored` 手动跑通
- 不破坏 preflight_claude 测试

**Steps:**
- [ ] Step 1：写 5 个失败测试
- [ ] Step 2：实现 preflight_codex_with + preflight_codex
- [ ] Step 3：`cargo test -p gitim-runtime preflight_codex`
- [ ] Step 4：`cargo test -p gitim-runtime --test preflight_codex -- --ignored`
- [ ] Step 5：commit `feat(runtime): add preflight_codex with real-hello verification`

---

## Task 4: 错误分类工具（`map_io_error_to_kind`）

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs`

**变更描述：**
- 抽一个 `fn map_spawn_error(err: &std::io::Error) -> ErrorKind`：`NotFound` → `NotInstalled`，其他 → `Other`
- 抽一个 `fn build_failure_result(provider: &str, kind: ErrorKind, error: String, duration: Duration) -> PreflightResult`
- `preflight_claude` / `preflight_codex` 改用这两个 helper，消除重复

**验收标准：**
- 单元测试 `test_map_spawn_error_not_found` / `test_map_spawn_error_other` 绿
- T2/T3 测试仍全绿
- `cargo clippy -p gitim-runtime` 无新增 warning

**Steps:**
- [ ] Step 1：写 2 个 helper 的 unit test
- [ ] Step 2：实现 helper
- [ ] Step 3：重构 preflight_claude / preflight_codex 用 helper
- [ ] Step 4：`cargo test -p gitim-runtime preflight`
- [ ] Step 5：commit `refactor(runtime): extract preflight error mapping helpers`

---

## Task 5: HTTP 路由 `/preflight/{provider}`（替换老路由）

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

**变更描述：**
- 删除 `async fn preflight_claude()`（line 980-991）及其路由注册 `.route("/preflight/claude", ...)`（line 1025）
- 新增 `async fn preflight_handler(axum::extract::Path(provider): axum::extract::Path<String>) -> impl IntoResponse`
  - match provider：
    - `"claude"` → 调 `crate::preflight::preflight_claude().await`
    - `"codex"` → 调 `crate::preflight::preflight_codex().await`
    - 其他 → return `(StatusCode::BAD_REQUEST, Json({"ok": false, "error": "unknown provider"}))`
  - 成功分支返回 `(StatusCode::OK, Json(result))`
- 路由注册改为 `.route("/preflight/{provider}", get(preflight_handler))`
- 返回 shape 要和 `PreflightResult` 的 serde 一致（前端按这个 contract 消费）

**测试：**
- 修改 `crates/gitim-runtime/tests/runtime_http.rs`（若不存在则新建）加 3 个测试：
  1. `GET /preflight/unknown` → 400 + `error: "unknown provider"`
  2. `GET /preflight/claude` → 返回对象包含 `provider: "claude"` 字段（不断言 available，因为无真实 claude 环境会失败）
  3. `GET /preflight/codex` → 返回对象包含 `provider: "codex"` 字段

**验收标准：**
- `cargo test -p gitim-runtime` 全绿
- `cargo build -p gitim-runtime` 通过
- 手动 `curl` 验证：`curl http://127.0.0.1:<port>/preflight/claude` 返回合法 JSON
- 原 `/preflight/claude` 路由不再响应（返回 404）

**Steps:**
- [ ] Step 1：写 3 个失败的 HTTP 测试
- [ ] Step 2：删除 preflight_claude 老 handler + 路由
- [ ] Step 3：实现 preflight_handler + 注册路径参数路由
- [ ] Step 4：`cargo test -p gitim-runtime runtime_http`
- [ ] Step 5：commit `feat(runtime): replace /preflight/claude with /preflight/{provider}`

---

## Task 6: POST /agents/add 强制 provider 字段

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`AgentAddRequest` struct line 500-512 + `agents_add` handler line 514）

**变更描述：**
- `AgentAddRequest.provider` 从 `Option<String>` 改为 `String`（移除 `#[serde(default)]`）
- serde 反序列化缺字段时返回 422，但 axum 默认把 Json 反序列化错误转成 400 — 足够
- 在 handler 头部验证 provider 值合法：只接受 `"claude"` 或 `"codex"`；未知 provider 返回 400 + `"error": "unsupported provider: <name>"`
- （注意：后端还支持 gemini / hermes / etc，但本 PR UI 只露 claude/codex，HTTP 也只放行这俩。未来放更多 provider 时再放宽校验）
- `AgentInfo.provider` 字段类型保持 `Option<String>`（向后 serde 兼容 me.json 旧数据），只是在新建 agent 的路径上不会塞 None
- me.json 写入时把 provider 写进去（现有 code 已做，line 572-574）

**测试（新建或扩展 `crates/gitim-runtime/tests/runtime_http.rs`）：**
1. `POST /agents/add` without `provider` → 400
2. `POST /agents/add` with `provider: "gemini"` → 400 + "unsupported"
3. `POST /agents/add` with `provider: "claude"` + minimal body → 200（需要先 `POST /workspace` + `POST /git/init`）

**验收标准：**
- 3 个测试绿
- 旧测试（如有 mock provider 的）同步更新，确保带 provider 字段
- `grep -rn "provider.*None" crates/gitim-runtime/src/http.rs` 确认 agents_add 路径不再 fallback

**Steps:**
- [ ] Step 1：写 3 个失败测试
- [ ] Step 2：改 AgentAddRequest + 加白名单校验
- [ ] Step 3：排查修复既有 agents_add 相关测试
- [ ] Step 4：`cargo test -p gitim-runtime`
- [ ] Step 5：commit `feat(runtime): require provider field on POST /agents/add`

---

## Task 7: AgentInfo 扩展 error_message 字段

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`AgentInfo` struct line 42-60）

**变更描述：**
- 加字段 `pub error_message: Option<String>`，带 `#[serde(skip_serializing_if = "Option::is_none")]`
- 默认 None
- 所有 `AgentInfo { ... }` 字面量构造位置（agents_add line 588 + recover_from_config line 956）加 `error_message: None`
- 不改 `AgentInfo` 的 PartialEq / 其他 impl（没看到有）

**验收标准：**
- `cargo build -p gitim-runtime` 通过
- `cargo test -p gitim-runtime` 全绿
- JSON 序列化默认不出现 `error_message` 字段（None + skip_serializing_if 生效）

**Steps:**
- [ ] Step 1：加字段 + 所有构造点补默认值
- [ ] Step 2：`cargo build -p gitim-runtime`
- [ ] Step 3：`cargo test -p gitim-runtime`
- [ ] Step 4：commit `feat(runtime): AgentInfo.error_message for recovery diagnostics`

---

## Task 8: recover_from_config 缺 provider → error 状态

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`recover_from_config` line 858-978）

**变更描述：**
- 在 recover 循环里读 `me.json` 后，判断 `me["provider"].as_str()` 是否存在
- 若存在 → 当前行为（正常起 loop）
- 若缺失 → 把该 agent 插入 state 但：
  - `status = "error"`
  - `error_message = Some(format!("Missing provider in {me_path}. Add \"provider\": \"claude\" or \"provider\": \"codex\" and restart the runtime."))`
  - 不调 `start_agent_loop`（跳过 auto-start）
- 若值为未知 provider（非 claude/codex）→ 同样登记为 error + error_message 说明合法值
- log warn 而非 error（error 给用户看，warn 给运维）

**测试：**
- 新建或扩展 `crates/gitim-runtime/tests/recover.rs`：
  1. `test_recover_missing_provider_marks_error`：构造一个 workspace，放一个 agent dir 含 me.json 但缺 provider 字段；调 recover_from_config；assert agent 出现在 state 里，status="error"，error_message 包含 "Missing provider"，loop_handle 为 None
  2. `test_recover_unknown_provider_marks_error`：provider="gemini"；同上，error_message 提示合法值
  3. `test_recover_valid_provider_starts_loop`：provider="claude"，确认正常走（但注意这会触发 agent_loop.init，可能需要更多脚手架 — 如果复杂度高，把"正常路径"断言简化为 status 不等于 "error"）

**验收标准：**
- 3 个测试绿（或前两个绿 + 第三个由现有 startup e2e 覆盖）
- recover 日志里能看到 warn

**Steps:**
- [ ] Step 1：写 recover 的 2-3 个失败测试
- [ ] Step 2：改 recover_from_config 分支
- [ ] Step 3：`cargo test -p gitim-runtime recover`
- [ ] Step 4：commit `feat(runtime): recovery marks agents with missing provider as error state`

---

## Task 9: SSE 推送启动 error 事件

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`recover_from_config` 内）

**变更描述：**
- 在 T8 的 error 分支里插入 state 后，发一条 `AgentActivityEvent { agent_id, event_type: "error", detail: error_message_text, timestamp }` 到 `activity_tx`
- 不影响正常路径
- 注意：recover_from_config 内是否可直接拿到 activity_tx — 当前函数签名是 `async fn recover_from_config(state: SharedRuntimeState)`，可以从 state 里拿 `state.activity_tx`（已有 broadcast sender）

**测试：**
- `test_recover_missing_provider_broadcasts_error`：订阅 activity_tx.subscribe()，触发 recover，assert 收到一条 event_type="error" 的 event

**验收标准：**
- 测试绿
- 手动 QA：准备一个含损坏 me.json 的 workspace，启动 runtime，订阅 `/agents/events` SSE，看到 error event

**Steps:**
- [ ] Step 1：写测试
- [ ] Step 2：在 T8 分支里调 activity_tx.send
- [ ] Step 3：`cargo test -p gitim-runtime recover_missing_provider_broadcasts`
- [ ] Step 4：commit `feat(runtime): broadcast SSE error event on recovery failure`

---

## Task 10: webui-v2 providers 常量与类型

**Files:**
- Create: `webui-v2/src/lib/providers.ts`

**变更描述：**
- 定义 `ProviderId = "claude" | "codex"` 类型
- 定义 `PROVIDERS: Record<ProviderId, { label: string; models: { id: string; label: string }[] }>`，值参考 Decisions Q5b
- 导出 `PROVIDER_IDS: ProviderId[]`（用于 UI 遍历）
- 定义 `PreflightResult` TypeScript 接口匹配后端 `PreflightResult` 的 JSON shape
- 定义 `PreflightErrorKind = "not_installed" | "timeout" | "other"`

**验收标准：**
- `pnpm -C webui-v2 exec tsc --noEmit`（或项目现有 type check 命令）通过
- 其他文件 import 这些常量不出 TS 错误

**Steps:**
- [ ] Step 1：创建 providers.ts
- [ ] Step 2：运行 type check
- [ ] Step 3：commit `feat(webui): add provider/model constants and types`

---

## Task 11: webui-v2 client.ts 接入 preflight + addAgent 签名

**Files:**
- Modify: `webui-v2/src/lib/client.ts`
- Modify: `webui-v2/src/lib/types.ts`（Agent 类型）

**变更描述：**
- 新增 `export async function preflightProvider(provider: ProviderId): Promise<ApiResponse<PreflightResult>>`，`GET ${baseUrl()}/preflight/${provider}`。返回值的 `ok` 字段基于 HTTP status（200 → ok=true）
- `addAgent(name, systemPrompt, model?, env?)` 签名升级为 `addAgent(name, provider: ProviderId, systemPrompt, model?, env?)`；body 加 `provider` 字段；必填（不再 optional）
- 调用方 AddAgentDialog 会在 T12/T13 同步更新
- `Agent` 类型加 `provider?: ProviderId`（从 recover 读回）、`errorMessage?: string`
- `mapBackendAgent`（同文件内若存在）加 `provider: backend.provider`、`errorMessage: backend.error_message`

**验收标准：**
- `tsc --noEmit` 通过
- mock client（client.ts 里的 `mockClient.addAgent`）签名同步更新或兜底处理（传入的 provider 值被忽略）

**Steps:**
- [ ] Step 1：改 Agent 类型加字段
- [ ] Step 2：client.ts 加 preflightProvider + 改 addAgent 签名
- [ ] Step 3：修复 mapBackendAgent
- [ ] Step 4：`tsc --noEmit`
- [ ] Step 5：commit `feat(webui): add preflightProvider and provider in addAgent signature`

---

## Task 12: AddAgentDialog provider 选择器 + 模型联动

**Files:**
- Modify: `webui-v2/src/components/management/add-agent-dialog.tsx`

**变更描述：**
- 新增 state：`provider: ProviderId | ""`（默认空，无默认 Q6a）、`model: string`（默认空 Q5b）
- 在表单顶部"Name"之前加一个 `Provider` 选择器（用已有 Radix `<select>` 或复用既有 ui/select 组件，和现有 model select 一致风格）
- options：遍历 `PROVIDER_IDS` 渲染 `{PROVIDERS[id].label}`；首选项 value="" label="— Select provider —"
- provider 变化时 `setModel("")` 清空（切换 provider 后 model 列表变了，旧值无效）
- 现有 model select 改为根据 `provider` 动态展示 `PROVIDERS[provider].models`；provider 为空时 model 选择器 disabled
- 表单 Submit 条件同时包含：`name.trim() && !validationError && provider && (detect pass — 见 T13) && !submitting`
- handleSubmit 传 `provider` 给 `client.addAgent`

**验收标准：**
- `tsc --noEmit` 通过
- 手动 QA：
  - Dialog 初始打开，provider 选择器 "— Select —"，model 选择器 disabled，Add 按钮 disabled
  - 选 Claude 后 model 下拉出现 3 个 Claude model
  - 换成 Codex 后 model 下拉切成 2 个 Codex model，之前选的 claude 模型被清空

**Steps:**
- [ ] Step 1：加 provider state + 选择器 UI + 清空 model 的 effect
- [ ] Step 2：改 model 选择器为动态
- [ ] Step 3：更新 submit disabled 条件（此时 detect 还没接，先只接 provider 非空）
- [ ] Step 4：`tsc --noEmit`
- [ ] Step 5：commit `feat(webui): provider selector and model linkage in AddAgentDialog`

---

## Task 13: AddAgentDialog Detect 按钮 + Add 硬阻塞

**Files:**
- Modify: `webui-v2/src/components/management/add-agent-dialog.tsx`

**变更描述：**
- 新增 state：
  - `detecting: boolean`（检测中，按钮 loading）
  - `detectResult: PreflightResult | null`（null=未检测；available=true/false 分别展示绿/红状态）
- 在 provider 选择器下方加一个 `Detect` 按钮：
  - disabled 当 provider=="" 或 detecting==true
  - onClick：`setDetecting(true); setDetectResult(null); const res = await preflightProvider(provider); setDetectResult(res.data); setDetecting(false)`
  - 旁边展示状态：
    - detecting → Loader icon + "Detecting..."
    - detectResult.available==true → 绿勾 + "OK — {duration_ms} ms"
    - detectResult.available==false → 红叉 + `detectResult.error` (或根据 error_kind 的用户文案：not_installed → "CLI not found — install claude/codex and retry"；timeout → "Timed out"；other → 展示原始 error)
- provider 变化时 `setDetectResult(null)`（切 provider 后旧的 detect 作废）
- Add 按钮 disabled 条件加入 `!detectResult?.available`
- 按 Q4d，Dialog 关闭时（onOpenChange(false)）重置所有 state（detecting、detectResult、provider、model 等）

**验收标准：**
- `tsc --noEmit` 通过
- 手动 QA：
  - 未选 provider → Detect disabled
  - 选 Claude → Detect enabled，点击 → loading（短暂） → 若本地已登录 Claude：绿勾，Add 按钮点亮
  - 若本地无 Claude CLI：红叉 + "CLI not found"，Add 仍 disabled
  - 切到 Codex → 状态重置，Add 再次 disabled
  - 关闭 Dialog 再打开 → 所有状态初始

**Steps:**
- [ ] Step 1：加 detecting / detectResult state + Detect 按钮 UI
- [ ] Step 2：接 preflightProvider 调用，处理 error_kind 文案
- [ ] Step 3：provider 变化时 reset detectResult
- [ ] Step 4：Dialog close 时 reset 全部 state
- [ ] Step 5：Add disabled 条件加 detectResult.available
- [ ] Step 6：`tsc --noEmit`
- [ ] Step 7：commit `feat(webui): Detect button with hard-block Add until preflight passes`

---

## Task 14: agent-card 展示 error_message

**Files:**
- Modify: `webui-v2/src/components/management/agent-card.tsx`
- 可能涉及 `webui-v2/src/components/chat/agent-status-panel.tsx`（若该面板也展示状态）

**变更描述：**
- 在 `status === "error"` 分支下，除了现有 "Error" badge，多渲染一行 `{agent.errorMessage ?? "unknown error"}`，字体较小（`text-xs text-muted-foreground`），支持换行
- 确保 Agent 类型已在 T11 加字段 `errorMessage?: string`
- agent-status-panel.tsx 同步调整（若已有 error 分支就顺带补上）

**验收标准：**
- `tsc --noEmit` 通过
- 手动 QA：构造一个 provider 缺失的 agent（参考 Task 8 场景），界面卡片显示 Error badge + 下方展示具体修复提示文本

**Steps:**
- [ ] Step 1：改 agent-card.tsx 加 error_message 展示
- [ ] Step 2：`tsc --noEmit`
- [ ] Step 3：commit `feat(webui): show error_message on agent-card when status=error`

---

## Task 15: E2E — preflight 路由测试

**Files:**
- Create: `e2e/tests/preflight.spec.ts`

**变更描述：**
- 复用 `startEnv()` 启动 runtime + vite（实际不需要 vite，但 startEnv 已包含；若想纯 HTTP 可用更轻的 helper — 暂复用现有）
- 测试用例：
  1. `GET /preflight/unknown` → HTTP 400，body 含 `error: "unknown provider"`
  2. `GET /preflight/claude` → HTTP 200，body 含 `provider: "claude"` + `available` 字段（bool）+ `duration_ms` 字段；不断言 available 的值（本地可能没登录）
  3. `GET /preflight/codex` → 同上但 provider=codex
- 所有测试不带 `E2E_REAL_PROVIDERS` 门控：它们只验证路由 contract，不依赖真实 LLM 响应。即使 CLI 没装，路由也要返回 200 + `available: false, error_kind: "not_installed"`

**验收标准：**
- `pnpm -C e2e exec playwright test preflight` 全绿
- 测试里不做 `test.skip()` 门控（本测试不烧钱）

**Steps:**
- [ ] Step 1：写测试文件
- [ ] Step 2：`pnpm -C e2e exec playwright test preflight`
- [ ] Step 3：commit `test(e2e): preflight route contract`

---

## Task 16: E2E — 双 provider 真实消息往返

**Files:**
- Create: `e2e/tests/agent-interaction-real.spec.ts`

**变更描述：**
- 顶层 `test.describe(...).skip(!process.env.E2E_REAL_PROVIDERS, "set E2E_REAL_PROVIDERS=1 to enable")`
- `beforeAll`：`buildRuntime()` + `startEnv()`
- 创建 Claude agent：`POST /agents/add` 带 `provider="claude"`, `model="claude-haiku-4-5"`, `handler="claude-bot"`, `display_name="Claude Bot"`
- 创建 Codex agent：同上 `provider="codex"`, `model="gpt-5.4-mini"`, `handler="codex-bot"`
- `POST /agents/start` 两个
- 初始化 poll cursor
- 给 "general" channel 发 2 条消息：
  - `@claude-bot reply with exactly: CLAUDE_HELLO`
  - `@codex-bot reply with exactly: CODEX_HELLO`
- 循环 `POST /im/poll`，until 两个回复都看到（body 含 CLAUDE_HELLO 和 CODEX_HELLO）或 150s 超时
- 测试超时单独设为 180_000（override 默认 60s — 参考 `playwright.config.ts` 用法）
- 成本控制：只跑 1 turn，prompt 非常短
- `afterAll`：stop agents + stopEnv

**验收标准：**
- 设 `E2E_REAL_PROVIDERS=1` 本地跑：`E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test agent-interaction-real`，两个 agent 都收到消息并回复
- 未设环境变量时：测试 skip（不烧钱）

**Steps:**
- [ ] Step 1：写测试文件
- [ ] Step 2：未设环境变量先跑一次确认 skip 生效
- [ ] Step 3：设置 `E2E_REAL_PROVIDERS=1` 手动跑一次验证通过
- [ ] Step 4：commit `test(e2e): real Claude + Codex agent message round-trip`

---

## Task 17: E2E UI — Detect 按钮端到端

**Files:**
- Modify: `e2e/tests/ui-agent-crud.spec.ts`（或另建 `e2e/tests/ui-agent-detect.spec.ts` 若担心文件太大）
- 同样门控 `E2E_REAL_PROVIDERS=1`

**变更描述：**
- 新增 test case `"user can detect provider and add claude + codex agents via UI"`：
  1. `page.goto(baseURL)`（vite dev server）
  2. 等启动界面完成（参考现有 startup.spec.ts 的 helper）
  3. 导航到 Agents/Management 页
  4. 点 "Add Agent" 打开 Dialog
  5. 填 Name
  6. 选 Provider=Claude（定位 Radix/HTML select）
  7. 确认 model 下拉出现
  8. 选 model=claude-haiku-4-5
  9. 点 Detect 按钮
  10. 等绿勾出现（timeout 30s）
  11. 断言 Add 按钮 enabled
  12. 点 Add → Dialog 关闭 → agent 出现在列表
  13. 重复同样流程建 codex-bot（provider=Codex, model=gpt-5.4-mini）
  14. 断言两个 agent 都 list 里且状态 idle/running

**验收标准：**
- `E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test ui-agent-crud` 绿
- 未设时 skip

**Steps:**
- [ ] Step 1：研究 AddAgentDialog 各字段的最终 HTML/accessibility name，决定 Playwright locator 策略
- [ ] Step 2：写测试 case
- [ ] Step 3：手动跑验证
- [ ] Step 4：commit `test(e2e): UI Detect button + add claude and codex agents`

---

## Task 18: 手动 QA 检查清单 + 文档

**Files:**
- Create: `docs/plans/multi-provider/02-qa-checklist.md`
- Modify: `docs/releases/v0.4.2.md`（或当前 next release 文档；若不存在则 skip）

**变更描述：**
- QA 清单至少覆盖：
  1. 新建 Claude agent 正流程（Detect 绿 → Add → agent 列表 idle）
  2. 新建 Codex agent 正流程
  3. 切换 provider 后 Detect 状态重置
  4. 未 Detect 点 Add：应无效
  5. Detect 失败时 Add disabled + 错误文案展示
  6. Detect 后关闭 Dialog 再打开：状态重置
  7. me.json 手动删 provider → runtime 重启 → agent 列表显示红 Error + 错误提示
  8. agent-card 显示 error_message
  9. POST /agents/add 缺 provider 字段 → 400（curl 验证）
  10. GET /preflight/unknown → 400
- 发布说明同步新增条目

**验收标准：**
- 清单文件存在，10+ 条
- 本地人工过一遍，全部通过

**Steps:**
- [ ] Step 1：写清单文件
- [ ] Step 2：commit `docs(plan): multi-provider QA checklist`
- [ ] Step 3（可选）：更新 release notes

---

## 总体验收（Phase 结束条件）

- [ ] `cargo test` 全绿（不含 `--ignored`）
- [ ] `cargo clippy -p gitim-runtime -p gitim-agent-provider` 无新 warning
- [ ] `cargo test -p gitim-runtime --test preflight_claude -- --ignored`（需要真实 claude 已登录） — 手动跑一次通过
- [ ] `cargo test -p gitim-runtime --test preflight_codex -- --ignored` — 手动跑一次通过
- [ ] `pnpm -C e2e exec playwright test preflight` 绿
- [ ] `E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test agent-interaction-real` 绿（手动跑）
- [ ] `E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test ui-agent-crud` 绿（手动跑）
- [ ] 手动 QA 清单 10 条全过
- [ ] `/preflight/claude`（老路径）返回 404
- [ ] 前端 `tsc --noEmit` 通过
- [ ] 前端 `pnpm -C webui-v2 lint` 不引入新 warning

## Out of Scope（明确不做）

- Gemini / Hermes / Openclaw / Opencode / Cursor 的 UI 露出
- `executable_path` UI 字段
- Deep preflight（带 CLAUDE.md 的 cwd）
- 前端单元测试基础设施
- 跨 clone 同步 provider 配置
- 旧 me.json 数据的自动迁移（provider 缺失直接 error，不自动补）
- 后端 preflight 结果缓存 / 节流
