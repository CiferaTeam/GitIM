# Provisioning Preflight — 实施计划 (分工版)

> 配套 `00-requirements.md` 阅读。所有 Architecture decisions §1-§9 + Premises P1-P8 已在 requirements 锁定。
> **本文档只写分工 + 步骤 + 验证标准，不嵌实际代码**（user 偏好）。代码细节由实施 agent 在每个 task 自己决定，但必须符合本文 acceptance criteria。

**Goal**: 给 `POST /workspaces/{slug}/agents/add` 加 server-side preflight gate —— preflight 失败时**没有任何 durable agent artifact** 被创建。

**Architecture**: 现有 `agents_add` handler 在 `handler_conflict` 检查之后、`provision_agent` 调用之前插一道 preflight。preflight 用 request body 的 env / model / llm_provider / llm_model 调对应 `preflight_X_with_*` 函数。失败 → return ErrorBody（含分类 `error_code` + `preflight_detail`）。WebUI 删 client-side detect。CLI plumbing 同步扩展保留 preflight_detail 通过 stderr 展示给 agent。

**Tech Stack**: Rust workspace stable channel; reqwest (existing); axum (existing); tokio (existing). 无新 crate dep。

---

## Phase A — Preflight 函数扩展 (3 tasks)

### Task 1 — 引入 `PreflightOverrides` typed struct + claude/codex 扩展

**Files**:
- Modify: `crates/gitim-runtime/src/preflight.rs`
- Modify: `crates/gitim-runtime/tests/preflight_claude.rs` (existing)
- Modify: `crates/gitim-runtime/tests/preflight_codex.rs` (existing)

**Acceptance**:
- [ ] Step 1.1: 在 `preflight.rs` 顶部加 `PreflightOverrides` struct（含 `env_override: Option<HashMap<String, String>>`, `model_override: Option<String>`）+ `Default` derive
- [ ] Step 1.2: 新增 `pub async fn preflight_claude_with_config(bin: &str, timeout: Duration, overrides: PreflightOverrides) -> PreflightResult` —— `cmd.envs(env_override)` 注入 + 若 `model_override.is_some()` 则替换 `CLAUDE_PREFLIGHT_MODEL` 注入到 `--model` 参数
- [ ] Step 1.3: 把现有 `preflight_claude_with(bin, timeout)` 改为 thin wrapper：内部调 `preflight_claude_with_config(bin, timeout, PreflightOverrides::default())`
- [ ] Step 1.4: 同模板：`preflight_codex_with_config(...)` + `preflight_codex_with(...)` delegating
- [ ] Step 1.5: 单元测试：每个 `_with_config` 路径加一个 happy-path（用 fake binary 在 PATH 注入，verify env vars + model arg 到达 fake 进程的 argv / env）+ 一个 default-path（`PreflightOverrides::default()`，verify 行为与旧函数一致）
- [ ] Step 1.6: `cargo build -p gitim-runtime` clean (no warnings)
- [ ] Step 1.7: `cargo test -p gitim-runtime --lib preflight` 全绿 + 现有 integration test `preflight_claude` / `preflight_codex` 不破
- [ ] Step 1.8: Commit — `feat(runtime/preflight): PreflightOverrides struct + claude/codex agent-aware variants`

### Task 2 — opencode / pi env_override 扩展 (no model_override)

**Files**:
- Modify: `crates/gitim-runtime/src/preflight.rs`
- Modify: `crates/gitim-runtime/tests/preflight_opencode.rs` (existing)
- Modify: `crates/gitim-runtime/tests/preflight_pi.rs` (existing)

**Acceptance**:
- [ ] Step 2.1: `preflight_opencode_with_config(bin, timeout, overrides)` —— 只用 `env_override`，**忽略** `model_override`（在 doc comment 显式说明：opencode CLI 无 `--model` flag，model 配在 `opencode auth login` 阶段）
- [ ] Step 2.2: 同模板：`preflight_pi_with_config(...)` —— 同样忽略 model_override，doc 说明 pi 把 provider/model 分开存
- [ ] Step 2.3: 老 `_with` 函数改为 delegating wrapper
- [ ] Step 2.4: 单元测试：env_override 注入到子进程 env；model_override 即使传也不影响 argv（doc + 行为一致）
- [ ] Step 2.5: 旧 integration test 不破
- [ ] Step 2.6: Commit — `feat(runtime/preflight): opencode + pi env_override (no model arg)`

### Task 3 — hermes `env_override` + chat-mode dispatch policy

**Files**:
- Modify: `crates/gitim-runtime/src/preflight.rs` (`preflight_hermes_with` 加 `env_override`)
- Modify: `crates/gitim-runtime/tests/preflight_hermes.rs` (existing)
- Add new helper to read default hermes profile config（参考 hermes_profile.rs:46 default_profile_ready 模式）

**Acceptance**:
- [ ] Step 3.1: 给 `preflight_hermes_with(...)` signature 加 `env_override: Option<HashMap<String, String>>` 末位参数；所有 internal call sites（`preflight_hermes_acp` / `preflight_hermes_chat`）传 env_override 到 `tokio::process::Command::envs(...)`
- [ ] Step 3.2: 现有 callers (`/preflight/{provider}` handler etc.) 传 `None`，保持 generic 行为
- [ ] Step 3.3: 新增 `pub fn read_default_profile_llm() -> Option<(String, String)>` 在 `preflight.rs` 或 `hermes_profile.rs` —— 读 `$HERMES_HOME/config.yaml` 解析 `model.default` + `model.provider`，缺失返 None。Use `serde_yaml` (already workspace dep)。Test with stub config.yaml
- [ ] Step 3.4: 单元测试：env_override 注入 hermes 子进程；read_default_profile_llm 三种 case（无 config / config 有 model / config 无 model）
- [ ] Step 3.5: Commit — `feat(runtime/preflight): hermes env_override + read_default_profile_llm helper`

---

## Phase B — 入口 helper + classify + ErrorBody 扩展 (2 tasks)

### Task 4 — `preflight_for_add_request` 入口 + error_code 分类

**Files**:
- Modify: `crates/gitim-runtime/src/preflight.rs` (新 helper + classify fn)

**Acceptance**:
- [ ] Step 4.1: `pub async fn preflight_for_add_request(provider: &str, env: Option<&HashMap<String, String>>, model: Option<&str>, llm_provider: Option<&str>, llm_model: Option<&str>) -> PreflightResult`
- [ ] Step 4.2: 实现 dispatch logic:
  - `mock` → 立刻 `PreflightResult::success("mock", ...)`，不 shell out
  - `claude` → `preflight_claude_with_config(default_bin, default_timeout, PreflightOverrides { env, model })`
  - `codex` → 同 claude 模板
  - `opencode` / `pi` → `_with_config(bin, timeout, PreflightOverrides { env, model: None })`（model 已 by design 不验）
  - `hermes` → 见 §1 of requirements：
    - 双值 → `preflight_hermes_with(..., llm_provider, llm_model, env_override: env)`
    - 双缺 + default profile 有 LLM → 用 resolved llm 调 chat
    - 双缺 + default profile 无 LLM → 返 `PreflightResult::failure(error_kind: Other, error_code: "hermes_default_profile_no_llm")`
    - 只缺一个 → 返 `PreflightResult::failure(error_code: "missing_llm_provider")` (复用现有 code)
  - unknown provider → 返 failure with `unknown_provider`
- [ ] Step 4.3: 外层 `tokio::time::timeout(PROVIDER_PREFLIGHT_TIMEOUT, ...)` 包裹（const 设 90s 给 hermes/chat 余地，但比 LONG_REQUEST_TIMEOUT 紧）；timeout → `PreflightResult::failure(error_kind: Timeout)`
- [ ] Step 4.4: `pub fn classify_preflight_error_code(pf: &PreflightResult) -> &'static str` —— 根据 PreflightResult 内部 error_code / error_kind 映射到 top-level：
  - `hermes_default_profile_no_llm` → `"hermes_default_profile_no_llm"`
  - `missing_llm_provider` → `"missing_llm_provider"`
  - `unknown_provider` → `"unknown_provider"`
  - 默认 → `"provision_preflight_failed"`
- [ ] Step 4.5: 单元测试：每种 dispatch 分支（含 mock short-circuit、hermes 三种 LLM 路径、未知 provider、timeout flavor）
- [ ] Step 4.6: Commit — `feat(runtime/preflight): preflight_for_add_request entry + error_code classify`

### Task 5 — `ErrorBody` 扩展 + 构造器

**Files**:
- Modify: `crates/gitim-runtime/src/http.rs`

**Acceptance**:
- [ ] Step 5.1: `ErrorBody` struct 加 `#[serde(skip_serializing_if = "Option::is_none")] preflight_detail: Option<PreflightResult>` 字段
- [ ] Step 5.2: 新增 `impl ErrorBody { pub fn with_preflight(message: impl Into<String>, code: &str, detail: PreflightResult) -> Self }`；现有 `with_code` / `new` 构造器签名保留，内部初始化 `preflight_detail: None`
- [ ] Step 5.3: 单元测试：序列化 `with_preflight` 出来的 JSON 包含 `preflight_detail` 嵌套对象；序列化 `with_code` 出来的 JSON **不含** preflight_detail 字段（per `skip_serializing_if`）
- [ ] Step 5.4: 现有任何调 `ErrorBody::with_code` 的地方 unchanged
- [ ] Step 5.5: Commit — `feat(runtime/http): ErrorBody preflight_detail + with_preflight constructor`

---

## Phase C — agents_add 接入 (1 task)

### Task 6 — `agents_add` 接入 preflight gate

**Files**:
- Modify: `crates/gitim-runtime/src/http.rs::agents_add`
- Add: `crates/gitim-runtime/tests/provision_preflight.rs` (新 integration test 文件)

**Acceptance**:
- [ ] Step 6.1: 在 `agents_add` 内 `handler_conflict` check 通过后、`provision_agent` 调用前，调 `preflight_for_add_request(...)` with request body fields
- [ ] Step 6.2: 失败时 `let code = classify_preflight_error_code(&pf); return Json(ErrorBody::with_preflight(...))` —— 用 HTTP 200 + `ok: false` 走现有 runtime contract（per CLAUDE.md，多处 failure body 用 200 + error_code）
- [ ] Step 6.3: 成功时继续走现有 provision_agent / me.json / hermes profile / state.insert / spawn_agent_loop（不动这一段）
- [ ] Step 6.4: Integration tests in `tests/provision_preflight.rs`：
  - `mock_provider_short_circuits_to_success`：provider=mock + 任何 config → preflight pass → state.workspaces 有 agent
  - `claude_with_valid_env_passes`：fake claude binary 在 PATH (echoes "GITIM_OK") → preflight pass
  - `claude_with_failing_binary_returns_preflight_failed`：fake claude binary exit 1 → preflight fail → ErrorBody w/ code `provision_preflight_failed` → state.workspaces 无 entry, no agent_dir 在 disk
  - `hermes_dual_llm_specified_dispatches_to_chat_with_overrides`：mock hermes 验 argv 含 --provider X --model Y
  - `hermes_no_llm_with_default_profile_having_llm`：stub default profile config.yaml → preflight 走 chat-mode w/ resolved llm
  - `hermes_no_llm_default_profile_missing_llm_returns_hermes_default_profile_no_llm`：stub default profile config.yaml 无 model → ErrorBody w/ `error_code: "hermes_default_profile_no_llm"`
  - `top_level_timeout_with_slow_fake_binary`：fake binary sleep 9999 → preflight 超时 (PROVIDER_PREFLIGHT_TIMEOUT) → ErrorBody w/ timeout-flavored preflight_detail
  - `post_preflight_failure_unchanged_path`：preflight pass + hermes profile clone 故意失败 → 仍走现有 `hermes_profile_create_failed` cleanup chain（**不**改 existing behavior）
- [ ] Step 6.5: 复用 `tests/common/mod.rs` daemon 隔离基础设施
- [ ] Step 6.6: `cargo test -p gitim-runtime --test provision_preflight` 全绿
- [ ] Step 6.7: 现有 `cargo test -p gitim-runtime` 全部测试不破（确认 no regression）
- [ ] Step 6.8: Commit — `feat(runtime/http): agents_add preflight gate before provision_agent`

---

## Phase D — CLI plumbing 扩展 (1 task)

### Task 7 — `cli/http.rs` + `cli/dto.rs` + `bin/runtime.rs` print path

**Files**:
- Modify: `crates/gitim-runtime/src/cli/http.rs::CliError`
- Modify: `crates/gitim-runtime/src/cli/http.rs::process_response_inner`
- Modify: `crates/gitim-runtime/src/cli/dto.rs::ErrorResponse`
- Modify: `crates/gitim-runtime/src/bin/runtime.rs` (error print envelope)

**Acceptance**:
- [ ] Step 7.1: `CliError::ResponseErrorCode` enum variant 加 `preflight_detail: Option<gitim_runtime::preflight::PreflightResult>` 字段
- [ ] Step 7.2: `cli::dto::ErrorResponse` struct 加 `preflight_detail: Option<gitim_runtime::preflight::PreflightResult>` (skip_serializing_if Option::is_none)
- [ ] Step 7.3: `process_response_inner` 在 `error_code` 分类时 deserialize body 含的 `preflight_detail` 字段一起塞进 CliError；body 无该字段时为 `None`
- [ ] Step 7.4: 主 binary `bin/runtime.rs` 的 error envelope print path：当 `preflight_detail` 是 `Some(pf)` 时 print to stderr：
  - `error_kind:` enum 翻译成 user-friendly 一句（not_installed / timeout / other）
  - `output_preview:` 截断到 ~200 字符
  - `version:` if Some
  - `model_used:` if Some
  - 其它 stderr line 保持简洁
- [ ] Step 7.5: 测试：用 fake server 返 ErrorBody with preflight_detail → CLI 跑 add-agent → stderr 含 `output_preview` substring
- [ ] Step 7.6: 现有 cli tests 全部不破
- [ ] Step 7.7: Commit — `feat(runtime/cli): preserve preflight_detail through error envelope`

---

## Phase E — WebUI 改 + docs (3 tasks)

### Task 8 — WebUI 删 client-side detect

**Files**:
- Modify: `products/gitim/frontend/src/lib/types.ts` (`ApiResponse` 扩展)
- Modify: `products/gitim/frontend/src/lib/client.ts::addAgent` (typed return)
- Modify: `products/gitim/frontend/src/components/management/add-agent-dialog.tsx`

**Acceptance**:
- [ ] Step 8.1: `ApiResponse<T>` 加 optional `preflight_detail?: PreflightResult`（PreflightResult 已在 types.ts 现有 import 上）
- [ ] Step 8.2: `client.ts::addAgent` 改 return type to typed `ApiResponse<{ id: string }>` 而非 raw JSON
- [ ] Step 8.3: `add-agent-dialog.tsx`:
  - 删 `detecting` / `detectResult` / `detectSeq` state vars
  - 删 `handleDetect` 函数 + 调用
  - 删 `detectErrorMessage` 函数
  - 删 "Detect" 按钮 + 其结果显示 block
  - 删 `!detectResult?.available` Submit guard
  - 删 provider onChange 触发 detect 失效的逻辑
  - 删 `PreflightResult` import 中不再用的字段（保留 PreflightResult 给错误展示用）
  - Loading state 复用现有 `submitting`
  - 错误展示：response 含 `preflight_detail` 时（不论 error_code）展示 `error` + `error_kind` 翻译 + `output_preview` collapsible
- [ ] Step 8.4: Submit 按钮文案改成"Add agent"（去掉先 detect 的暗示）
- [ ] Step 8.5: Manual smoke：在 dev server 上加 hermes agent + 故意配错 LLM model → 确认 add 走完流程 + UI 显示 preflight_detail.output_preview
- [ ] Step 8.6: TypeScript `npm run typecheck` (or whatever 现有 frontend type check command) clean
- [ ] Step 8.7: Commit — `feat(frontend): drop client-side detect, surface preflight_detail in errors`

### Task 9 — `docs/specs/runtime-cli.md` 更新

**Files**:
- Modify: `docs/specs/runtime-cli.md`

**Acceptance**:
- [ ] Step 9.1: 更新 / 删除现有 "Provisioning preflight (future work)" 章节 —— 这次 land 了，改写成"现状"段说明 server-side preflight 已落地
- [ ] Step 9.2: add-agent section 加 error_code 表条目：
  - `provision_preflight_failed` —— preflight 失败（含 auth / model not found / network 等）；exit 2
  - `hermes_default_profile_no_llm` —— hermes 不指定 llm 且 default profile 无 LLM 配置；exit 2
- [ ] Step 9.3: add-agent section 加 known limitations：
  - opencode / pi preflight 不验 model 名（CLI 无 `--model` flag）
  - claude/codex preflight 烧 agent 配置 model 一个 hello token
  - hermes 不指定 llm 时 preflight 验 default profile 的 LLM —— 即"agent 将会继承的"
- [ ] Step 9.4: 加 "What preflight catches / doesn't catch" 简短列表（per requirements §8）
- [ ] Step 9.5: Commit — `docs(specs): document provisioning-preflight now landed`

### Task 10 — `CLAUDE.md` Current Orientation 更新

**Files**:
- Modify: `CLAUDE.md`

**Acceptance**:
- [ ] Step 10.1: "Where we are" 末尾追加一段记录 provisioning-preflight 落地：
  - server-side preflight gate before provision_agent
  - reuses preflight_X functions with agent-aware overrides
  - hermes backward-compat: 无 llm 时 validate default profile's LLM
  - WebUI 删 client-side detect
  - new error codes: provision_preflight_failed / hermes_default_profile_no_llm
- [ ] Step 10.2: "Where we're going" 末尾 add: opencode/pi model-arg 支持（如未来需要）
- [ ] Step 10.3: Crate 地图 `gitim-runtime` 行的"关键模块" 加 `preflight` 标注 agent-aware variants
- [ ] Step 10.4: Commit — `docs(claude): orient on provisioning-preflight landing`

---

## Phase F — Regression + final smoke (1 task)

### Task 11 — Full regression + manual e2e

**Files**: 无新增

**Acceptance**:
- [ ] Step 11.1: `cargo test -p gitim-runtime` 全跑过（baseline 294 lib + integration 全部，per CLAUDE.md "末尾全量"）
- [ ] Step 11.2: Cross-crate spot check：`cargo test -p gitim-core -p gitim-daemon -p gitim-sync` 无 regression
- [ ] Step 11.3: WebUI dev server + 手工 e2e：
  - 加 claude agent + 故意拼错 model → preflight fail + UI 显示 detail
  - 加 hermes agent + 双 llm 值 + 正确 → preflight pass + agent 起来
  - 加 hermes agent + 不指定 llm + default profile 有 LLM → preflight pass
  - 加 mock agent → 立刻 pass
- [ ] Step 11.4: CLI e2e：`gitim-runtime add-agent --provider claude --model bogus-model-xyz ...` → exit 2 + stderr 含 preflight_detail.output_preview
- [ ] Step 11.5: 不 commit；汇报状态；进 sop-dev-mode Phase 6 (code review × 2)

---

## 依赖关系图

```
T1 (PreflightOverrides + claude/codex)
   ↓
T2 (opencode/pi env_override)        ← 跟 T1 平行也可，但同文件 sequential 更稳
   ↓
T3 (hermes env_override + read_default_profile_llm)
   ↓
T4 (preflight_for_add_request entry + classify)
   ↓
T5 (ErrorBody extension)             ← T4/T5 顺序无关，T5 可与 T4 平行
   ↓
T6 (agents_add 接入)                  ← 依赖 T4 + T5
   ↓
T7 (CLI plumbing)                     ← 依赖 T5（ErrorBody 形状）
   ↓                                    + T6 (确认服务端真返 detail)
T8 (WebUI)                            ← 同 T7，依赖 T5/T6
   ↓
T9 (docs/specs)                       ← 实施完成后写文档
   ↓
T10 (CLAUDE.md)
   ↓
T11 (regression + e2e)
```

T1-T3 同 `preflight.rs` 文件，sequential 实施。T4/T5 可并行但同模块。T7/T8 可并行（前后端独立）。

---

## 不在本 plan 范围（已在 00-requirements.md 写明）

- 修改 `/preflight/{provider}` HTTP endpoint（generic preflight 不动）
- 修改 agent_loop
- opencode / pi 的 `--model` arg 支持（writing-plans 调研得知不支持，asymmetry 文档化）
- Provisioning 之后的失败路径 (hermes_profile_create_failed / apply_model_config) 仍留 remote orphan —— 现状不变
- Preflight 结果持久化 / 缓存
- 异步 preflight + status poll endpoint
- 新 provider 接入

---

## Self-review checklist (writing-plans 自检)

- [x] **Spec coverage**: P1-P8 + §1-§9 全部落到 task。Hermes B 决策 → T4 dispatch；ErrorBody 扩展 → T5；CLI plumbing → T7；WebUI → T8；§8 文档 → T9
- [x] **No placeholder**: 每 step 有 exact 文件路径 + 验证 expected output；中文叙述代替代码块（per user 偏好）
- [x] **Type consistency**: `PreflightOverrides` / `preflight_for_add_request` / `classify_preflight_error_code` 名字跨 task 一致
- [x] **TDD 节奏**: 每个 task 内部 test 第一，validation 后 commit
- [x] **Bite-sized**: 每 step 可独立 reason
- [x] **依赖排序**: phase 边界清晰，T1-T3 同文件 sequential，跨 phase 依赖明确

---

## Execution handoff

**Plan 完成并 commit。两种执行方式选一：**

1. **Subagent-Driven（推荐，跟 runtime-cli 一致）**：每 task fresh subagent 实施，task 间 spec + quality two-stage review，过了 next。
2. **Inline Execution**：在当前 session 顺序打完，checkpoint 在 phase 边界。

或者你想再 push back / 调整某些 task scope 后再开工。
