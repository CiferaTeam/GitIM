# Hermes Multi-LLM Provider Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 WebUI 在添加 hermes-typed agent 时选择具体 LLM provider × model;后端 introspect 用户已配 provider,live-fetch model 列表,创建 profile 后用 `hermes config set` 写入 model 子树。每个 agent 跑独立 LLM,真正实现 "3 个 hermes agent 用 3 个不同 LLM"。

**Architecture:** 新 `hermes_llm` 模块负责 provider/model 数据(registry static + introspect .env/custom_providers + live fetch /models)。`POST /agents/add` 当 `provider == "hermes"` 时要求 `llm_provider` + `llm_model`,ensure_profile 后顺序 shell out `hermes -p gitim-<h> config set model.{provider,default,base_url}` 三次,失败回滚 delete_profile + cleanup_agent_dir。前端 AddAgentDialog 在选了 hermes 后内嵌一段 LLM dropdown + 升级版 Detect 按钮(带 llm_provider/llm_model query param,在 default profile 上 override 验证)。

**Tech Stack:** Rust(tokio / reqwest / serde / tempfile / httpmock)· React 19 / TypeScript / Radix UI / Zustand · Playwright E2E · TDD inline `#[cfg(test)]` for unit / `tests/` 外部文件 for integration · `--ignored` + `E2E_REAL_PROVIDERS` 守门真实 LLM 调用

**约定:**
- 本 plan 遵循用户偏好 `plan_no_code`:只写分工、文件、验收,不写代码
- TDD 节奏:先红(写失败测试)→ 绿(实现)→ commit。每任务可独立 commit
- 工作目录:`/Users/lewisliu/ateam/GitIM/.claude/worktrees/laughing-curie-aac17c/`
- 分支:`claude/laughing-curie-aac17c`(已在用)
- 实施期间所有 cargo / pnpm / git 操作都在上面 worktree 目录下执行
- 测试节奏:任务开头 + 末尾跑全量,中间只跑 scoped 测试(参考 CLAUDE.md "跑测试的节奏")
- Spec source of truth:`docs/plans/hermes-multi-llm/01-design.md`(本 plan 是它的可执行展开)

---

## Decisions Summary(来自 brainstorming)

| # | 决策 | 结论 |
|---|------|------|
| Q1 | LLM provider 列表来源 | **后端 introspection** —— 读 `.env` + `config.yaml.custom_providers`,不读 `auth.json` |
| Q2 | OAuth provider 范围 | **不做** —— v1 仅 API-key |
| Q3 | 写新 profile model 配置 | **shell out** `hermes -p gitim-<h> config set model.{provider,default,base_url}` |
| Q4 | model 列表来源 | **live fetch** `<base_url>/models`,失败 200 + error 字段,前端 fallback Custom 输入 |
| Q5 | 已有 agent + edit | **strict new-only** —— 不做 retroactive,不做 PATCH |
| L1 | model label | v1 统一 `label = id`(不做 display_name 美化) |
| L2 | model fetch cache | 不缓存 |
| L3 | `/models` HTTP status | 永远 200,error 字段载错误 |
| L4 | endpoint namespace | `/hermes/llm/*` |
| L5 | me.json 字段 | `llm_provider` / `llm_model`(不复用 `model`)|
| L6 | config-set 失败回滚 | `delete_profile` + `cleanup_agent_dir` |
| L7 | Detect 升级 | query param `llm_provider`/`llm_model`,在 default profile 上 `--provider/--model` override |
| L8 | 前端布局 | AddAgentDialog 内嵌段(不拆 wizard 页) |
| L9 | dialog state 寿命 | dialog close 时 reset 全部 hermes-LLM state |

---

## File Structure

### 新建

| 路径 | 职责 |
|------|------|
| `crates/gitim-runtime/src/hermes_llm/mod.rs` | re-export 子模块,顶层 doc |
| `crates/gitim-runtime/src/hermes_llm/registry.rs` | `BUILTIN_PROVIDERS` 静态表 6 项 + `BuiltinProvider` 结构 |
| `crates/gitim-runtime/src/hermes_llm/introspect.rs` | `list_providers(hermes_home)` 读 .env + custom_providers,纯函数 |
| `crates/gitim-runtime/src/hermes_llm/models.rs` | `fetch_models(provider, hermes_home)` live fetch /models,5s timeout |
| `crates/gitim-runtime/tests/hermes_llm_introspect.rs` | introspect 集成测试,tempdir fixtures |
| `crates/gitim-runtime/tests/hermes_llm_fetch.rs` | fetch_models 集成测试,httpmock |
| `crates/gitim-runtime/tests/hermes_llm_http.rs` | HTTP endpoint 集成测试 |
| `crates/gitim-runtime/tests/hermes_llm_e2e.rs` | 完整 add_agent flow 端到端断言 |
| `webui-v2/src/lib/hermes-llm.ts` | TypeScript 类型 + 静态 builtin label 映射(不重复 base_url,后端为准)|
| `e2e/tests/ui-hermes-llm.spec.ts` | Playwright UI E2E,`E2E_REAL_PROVIDERS` 守门 |
| `docs/plans/hermes-multi-llm/03-qa-checklist.md` | 手动 QA 清单 |

### 修改

| 路径 | 改动概要 |
|------|----------|
| `crates/gitim-runtime/Cargo.toml` | 加 `httpmock` dev-dep(若未有);确认 `reqwest` / `serde_yaml` / `tempfile` 已在 |
| `crates/gitim-runtime/src/lib.rs` | `pub mod hermes_llm;` |
| `crates/gitim-core/src/me_json.rs` | `MeJson` 加 `llm_provider` + `llm_model` 字段 + `merged_with` 处理 |
| `crates/gitim-runtime/src/hermes_profile.rs` | 新增 `pub async fn apply_model_config(handler, llm_provider, llm_model, base_url: Option)` 3 次 shell out 序列 |
| `crates/gitim-runtime/src/preflight.rs` | `preflight_hermes_with` 加 `llm_provider: Option<&str>` + `llm_model: Option<&str>` 参数 |
| `crates/gitim-runtime/src/http.rs` | (1) `AgentAddRequest` 加字段; (2) `agents_add` provider==hermes 校验 + apply_model_config + 回滚; (3) 新两个 GET handler; (4) `preflight_handler` 接受 query param |
| `webui-v2/src/lib/client.ts` | 加 `listHermesLlmProviders` / `listHermesLlmModels` / 升级 `preflightHermes(llmProvider, llmModel)` / 升级 `addAgent` 接受新字段 |
| `webui-v2/src/lib/types.ts` | `Agent` 类型加 `llmProvider?` / `llmModel?` 字段 |
| `webui-v2/src/components/management/add-agent-dialog.tsx` | provider==hermes 时内嵌 LLM section,dropdown 联动 + state machine + dialog close reset |
| `CLAUDE.md` | Current Orientation 更新(加 hermes 多 LLM 选择已落地);Non-goals 写明 OAuth / edit / retroactive 留 v2 |

---

## 任务依赖图

```
T0 Phase 0 baseline ─┬─→ (绿灯,继续)
                      └─→ (红灯,改 Section 3 step 4 实现 → 重新进 brainstorming)

T1 hermes_llm 骨架 + registry ──→ T2 introspect ────┐
                              └─→ T3 fetch_models ────┤
                                                       ├──→ T6 GET /providers ──┐
                                                       └──→ T7 GET /models   ───┤
T4 MeJson 字段扩展 ──┐                                                            │
T5 apply_model_config ─┴─→ T8 add_agent 升级(provider=hermes 路径)──────────────┤
                                                                                  │
T9 preflight_hermes_with 升级 ──→ T10 preflight_handler query param ─────────────┤
                                                                                  │
                                              ┌───────────────────────────────────┤
                                              ▼                                   │
                                T11 webui-v2 types + hermes-llm.ts                │
                                              │                                   │
                                              ▼                                   │
                                T12 client.ts 接入                                │
                                              │                                   │
                                              ▼                                   │
                                T13 AddAgentDialog 内嵌 LLM 段                    │
                                              │                                   │
                                              ▼                                   │
                                ┌─────────────┴────────────┐                      │
                                ▼                          ▼                      │
                T14 backend e2e (cargo)         T15 ui e2e (Playwright)           │
                                              │                                   │
                                              ▼                                   │
                                T16 CLAUDE.md + qa-checklist ─────────────────────┘
```

**并行度:**
- T2 / T3 / T4 / T5 互不依赖,可并行
- T6 / T7 互不依赖,可并行
- T9 / T10 互不依赖,可并行(但 T10 测试依赖 T9 的签名)
- T14 / T15 最后一层可并行

---

## Task 0: Phase 0 baseline 验证

**目标:** 在写任何代码前,**实地 verify spec 的三个 load-bearing 假设**。任一红灯 → 暂停 + 跟用户确认是否回 brainstorming 改 design。

**Files:**
- 临时记录:不写到代码,把 verify 结果贴到本 task 步骤的 PR 描述里

**待验证清单:**

1. **`hermes config set` 支持点号 path** —— 跑 `hermes config set model.provider zai`,检查 `~/.hermes/config.yaml` 是否真的 nested 写到 `model: {provider: zai}` 子树。同样测 `model.default`、`model.base_url`。
2. **Spec 表里 6 个 builtin provider 的 `inference_base_url` + `api_key_env_vars`** —— 跟 `~/ateam/code-skills/.repos/hermes-agent/hermes_cli/auth.py` 真实 `PROVIDER_REGISTRY` 逐条对账(spec 已对账过一次,baseline 再 verify 是为 catch hermes 升级 drift)。
3. **每个 builtin provider 的 `/models` endpoint 真实路径** —— 用 `curl -H "Authorization: Bearer $KEY" <base_url>/models` 探测,记录哪些走 `/models`、哪些走 `/v1/models`、哪些不存在。这条决定 `BuiltinProvider` 是否需要 `models_path: Option<&str>` 字段。
4. **当前 main 全量测试 baseline** —— 跑 `cargo test --workspace`,记录已知红测试 / `--ignored` 列表,作为后续增量比对基线。
5. **本机 hermes binary 状态** —— 跑 `hermes --version` + `hermes profile list`,确认 binary 在 PATH + 至少有一个 profile(default 已配 minimax-cn,符合)。

**验收标准:**
- 每条 verify 都有 yes/no/部分 答案 + 证据(命令输出片段)
- 任一假设红灯,暂停后续 task,把状态汇报给用户
- baseline 全量测试结果记录到本 task 的临时段落,后续 task 完成时跟它比对

**Steps:**
- [ ] **Step 1:** 在 default profile 跑 `hermes config set model.provider zai && cat ~/.hermes/config.yaml`,确认 model.provider=zai 在 yaml 子树
- [ ] **Step 2:** 跑 `hermes config set model.base_url https://example.test && cat ~/.hermes/config.yaml`,确认 model.base_url 写入 + 注意是否覆盖了原 model.base_url 字段(如果是,后续 step 4 失败回滚 + 重试要把这个值还原 — 在 spec risks 加一行)
- [ ] **Step 3:** 跑 `hermes config set model.provider minimax-cn && hermes config set model.default MiniMax-M2.7-highspeed && hermes config set model.base_url https://api.minimaxi.com/anthropic` 还原默认
- [ ] **Step 4:** 对账 6 个 provider 的 `auth.py` 条目,记录 alias 列表 + base_url 一致性(spec 已对过,这步是 sanity check)
- [ ] **Step 5:** 用 `KIMI_API_KEY` + `MINIMAX_CN_API_KEY`(用户已有)跑 `curl -H "Authorization: Bearer $KEY" <base_url>/models`,记录响应 status + body 摘要;其他 4 个 provider 没 key 跑不出来,登记 "无 key 跳过,实现时凭 hermes 文档假设 OpenAI-compatible /models 路径"
- [ ] **Step 6:** `cargo test --workspace 2>&1 | tail -40` 抓最后 40 行(过/红 summary + 时长),贴到本 task
- [ ] **Step 7:** `hermes --version && hermes profile list` 输出贴到本 task
- [ ] **Step 8:** 把上述 verify 结果汇总成一个 markdown 段落,放到本 plan 顶部一个临时 "Baseline 2026-05-10" 区块。后续 task 全完成时删除这段
- [ ] **Step 9:** Commit `chore(plan): record hermes-multi-llm baseline verification` —— 本 commit 只动 02-plan.md(临时段落)

---

## Task 1: hermes_llm 模块骨架 + BUILTIN_PROVIDERS registry

**Files:**
- Create: `crates/gitim-runtime/src/hermes_llm/mod.rs`
- Create: `crates/gitim-runtime/src/hermes_llm/registry.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`(加 `pub mod hermes_llm;`)
- Test: `registry.rs` 内联 `#[cfg(test)] mod tests`

**变更描述:**
- `mod.rs`:re-export `registry::BuiltinProvider` 和 `BUILTIN_PROVIDERS`;加模块级 doc 说明 "Hermes-internal LLM provider × model selection layer"
- `registry.rs`:
  - `pub struct BuiltinProvider { pub id: &'static str, pub label: &'static str, pub env_vars: &'static [&'static str], pub base_url: &'static str, pub models_path: &'static str }` (`models_path` 默认 `"/models"`,Anthropic 等用 `"/v1/models"`,Phase 0 verify 决定)
  - `pub const BUILTIN_PROVIDERS: &[BuiltinProvider]` 6 项,顺序 alphabetic by id
  - 顶部 `// Mirrored from hermes_cli/auth.py:PROVIDER_REGISTRY @ <hermes-version-from-Phase0>. Resync on hermes minor bumps; CI does not enforce.` 注释

**测试(先红):**
1. `registry_has_six_builtin_providers` —— assert `BUILTIN_PROVIDERS.len() == 6`
2. `registry_ids_unique` —— assert id 不重复
3. `registry_no_empty_env_vars` —— assert 每个 provider 至少 1 个 env_var alias
4. `registry_anthropic_has_token_aliases` —— assert id="anthropic" 的 env_vars 含 `ANTHROPIC_API_KEY` 且 len >= 2
5. `registry_zai_has_glm_alias` —— assert id="zai" 的 env_vars 含 `GLM_API_KEY`

**验收标准:**
- `cargo build -p gitim-runtime` 通过
- `cargo test -p gitim-runtime hermes_llm::registry` 5 个测试全绿
- `cargo clippy -p gitim-runtime` 无新增 warning

**Steps:**
- [ ] **Step 1:** 在 `registry.rs` 内联 mod tests 写 5 个失败测试(模块还不存在 → 编译失败)
- [ ] **Step 2:** `cargo test -p gitim-runtime hermes_llm` 验证编译失败
- [ ] **Step 3:** 实现 `BuiltinProvider` struct + `BUILTIN_PROVIDERS` const(6 项,值从 spec Section "BUILTIN_PROVIDERS" 表 + Phase 0 verify 结果取)
- [ ] **Step 4:** 实现 `mod.rs` re-export
- [ ] **Step 5:** 在 `lib.rs` 加 `pub mod hermes_llm;`
- [ ] **Step 6:** `cargo test -p gitim-runtime hermes_llm::registry` 全绿
- [ ] **Step 7:** Commit `feat(runtime): hermes_llm module skeleton + BUILTIN_PROVIDERS table`

---

## Task 2: introspect — list_providers 实现

**Files:**
- Create: `crates/gitim-runtime/src/hermes_llm/introspect.rs`
- Modify: `crates/gitim-runtime/src/hermes_llm/mod.rs`(re-export `list_providers` + `LlmProvider`)
- Test: `crates/gitim-runtime/tests/hermes_llm_introspect.rs`(集成测试,tempdir fixtures)

**变更描述:**
- `LlmProvider { id: String, label: String, kind: ProviderKind, base_url: Option<String> }` + `enum ProviderKind { ApiKey, Custom }`,均 derive `Serialize` + `Deserialize` + `Clone` + `PartialEq` + `Debug`
- `pub fn list_providers(hermes_home: &Path) -> Vec<LlmProvider>` —— 不返回 Result(失败 → 空列表 + log warn,符合 spec "all return 200 with empty or partial list")
- 实现:
  1. 读 `<hermes_home>/.env`(用 `dotenvy::from_path_iter` 或自己解析 `KEY=VALUE` 行,跳过注释 + 空行)
  2. 对 `BUILTIN_PROVIDERS` 每个 entry,检查它的 `env_vars` 任一在 .env 里出现且值非空 → push 一条 `{ id, label, kind: ApiKey, base_url: Some(<from registry>) }`
  3. 读 `<hermes_home>/config.yaml`,parse YAML 取 `custom_providers: List[{name, base_url, ...}]`,每条 push `{ id: format!("custom:{name}"), label: format!("{name} (custom)"), kind: Custom, base_url: Some(<from entry>) }`
  4. 顺序:builtin alphabetic,custom 列表保持 yaml 顺序排在最后
- 失败模式(对应 spec 表):.env 缺/读不了 → 跳过 source 1;config.yaml 缺/parse 失败 → 跳过 source 2 + log warn;hermes_home 整个不存在 → 返回空 vec

**测试(`tests/hermes_llm_introspect.rs`):**
1. `empty_hermes_home_returns_empty_list` —— hermes_home = 不存在的 tempdir 子路径
2. `env_with_key_lists_provider` —— 写 `KIMI_API_KEY=foo` 到 tempdir/.env,断言 list 含 id=kimi-coding
3. `env_with_alias_lists_provider` —— 写 `ZAI_API_KEY=foo`(zai 的 alias),断言 id=zai 出现
4. `empty_value_treated_as_unconfigured` —— 写 `KIMI_API_KEY=`(空值),断言 list 不含 kimi-coding
5. `config_yaml_custom_providers_listed` —— 写 yaml 含 `custom_providers: [{name: my-glm, base_url: https://x}]`,断言 list 含 id=`custom:my-glm`,kind=Custom
6. `config_yaml_parse_error_skipped` —— 写非法 yaml,断言 list 仍返回 builtin 部分(不 panic)
7. `builtin_and_custom_with_same_name_both_listed` —— .env 有 KIMI_API_KEY 同时 config.yaml.custom_providers 有 name=kimi-coding,断言两条都在 list(id 不同)
8. `ordering_builtin_alphabetic_then_custom` —— 多 provider + 多 custom,断言顺序

**验收标准:**
- 8 个集成测试全绿
- `cargo clippy` 无新 warning
- `list_providers` 是纯函数(除了 fs 读),无 side effect

**Steps:**
- [ ] **Step 1:** 写 8 个失败集成测试(`tests/hermes_llm_introspect.rs`)
- [ ] **Step 2:** `cargo test -p gitim-runtime --test hermes_llm_introspect` 全部失败(模块缺函数)
- [ ] **Step 3:** 实现 `LlmProvider` 结构 + `ProviderKind` 枚举(在 introspect.rs 顶部)
- [ ] **Step 4:** 实现 .env 解析(可以用现有依赖 `dotenvy` 或简单手写,grep 现有 codebase 看哪条路已用)
- [ ] **Step 5:** 实现 config.yaml 解析(用 `serde_yaml`,grep workspace 看是否已有 dep,没有就加到 Cargo.toml)
- [ ] **Step 6:** 实现 list_providers 主体逻辑
- [ ] **Step 7:** 跑 `cargo test -p gitim-runtime --test hermes_llm_introspect`,全绿
- [ ] **Step 8:** 跑 `cargo test -p gitim-runtime hermes_llm` 确认 Task 1 测试仍绿
- [ ] **Step 9:** Commit `feat(runtime): introspect hermes home for configured LLM providers`

---

## Task 3: fetch_models — live HTTP fetch with httpmock

**Files:**
- Create: `crates/gitim-runtime/src/hermes_llm/models.rs`
- Modify: `crates/gitim-runtime/src/hermes_llm/mod.rs`(re-export `fetch_models` + `ModelListResult`)
- Modify: `crates/gitim-runtime/Cargo.toml`(加 `httpmock` 到 `[dev-dependencies]` 若未有)
- Test: `crates/gitim-runtime/tests/hermes_llm_fetch.rs`

**变更描述:**
- `ModelListResult { models: Vec<ModelEntry>, custom_allowed: bool, error: Option<String>, fetched_at_ms: u64 }` derive Serialize
- `ModelEntry { id: String, label: String }` derive Serialize
- `pub async fn fetch_models(provider: &LlmProvider, hermes_home: &Path) -> ModelListResult`
  1. 解析 base_url(provider.base_url.unwrap_or_else 失败 → error="missing base_url for provider <id>")
  2. 解析 models_path —— builtin 从 registry 取(默认 `"/models"`,Anthropic 是 `"/v1/models"`),custom 默认 `"/models"`
  3. 解析 API key:
     - kind=ApiKey:扫 .env 找该 provider env_vars 任一非空值
     - kind=Custom:从 config.yaml.custom_providers 找 `api_key` 字段
     - 找不到 → 返回 200 + error="missing api key for <id> — set <ENV_VAR> in ~/.hermes/.env" + models 空
  4. 构造 reqwest::Client(timeout 5s,无 retry,无 cache)
  5. GET `<base_url><models_path>`,header `Authorization: Bearer <key>`
  6. 错误分类(spec 失败模式表):
     - 网络/connect/dns 错 → error="network error: {e}"
     - timeout → error="timeout fetching ..."
     - HTTP 401/403 → error="auth failed (HTTP {code}) — verify api key"
     - HTTP 4xx/5xx 其他 → error="upstream HTTP {code}"
     - JSON parse 失败 → error="unexpected response schema (not OpenAI-compatible) — use Custom..."
     - `data` 字段缺/非数组 → 同上
  7. 成功 schema:`{"data": [{"id": "...", ...}]}` → 提取每个 `data[i].id` 作为 ModelEntry.id 和 .label
  8. `fetched_at_ms` = `SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64`
- `custom_allowed: true` 永远(spec L1)

**测试(`tests/hermes_llm_fetch.rs`,httpmock):**
1. `success_openai_compatible` —— mock `/models` 返 200 `{data: [{id: "m1"}, {id: "m2"}]}`,assert `models.len() == 2`,error is None
2. `http_401_returns_auth_failed_error` —— mock 401,assert error 含 "auth failed (HTTP 401)"
3. `http_500_returns_upstream_error` —— mock 500,assert error 含 "upstream HTTP 500"
4. `timeout_returns_timeout_error` —— mock delay 10s,assert error 含 "timeout fetching"
5. `parse_failure_returns_schema_error` —— mock 200 返 `{"unexpected": "shape"}`,assert error 含 "unexpected response schema"
6. `data_field_missing_returns_schema_error` —— mock 返 `{"object": "list"}`(缺 data),同上
7. `missing_api_key_returns_actionable_error` —— hermes_home 的 .env 不含对应 key,assert error 含 "missing api key for"
8. `error_message_does_not_leak_api_key` —— 设 KIMI_API_KEY="secret-token-xxx",触发 401,assert error 字符串**不**含 "secret-token-xxx"
9. `models_path_override_for_anthropic` —— 用 anthropic provider mock `/v1/models` 返 200(不是 `/models`),assert request 真实打到 `/v1/models`

**验收标准:**
- 9 个测试全绿
- `cargo clippy` 无新 warning
- 测试用 httpmock 不打外网,CI 友好
- API key 不出现在任何 error 字符串里(测试 8 显式断言)

**Steps:**
- [ ] **Step 1:** Cargo.toml 检查 / 加 `httpmock = "0.7"` 到 dev-dependencies(或 workspace 现有版本)
- [ ] **Step 2:** 写 9 个失败测试
- [ ] **Step 3:** `cargo test -p gitim-runtime --test hermes_llm_fetch` 失败
- [ ] **Step 4:** 实现 `ModelEntry` / `ModelListResult` 结构
- [ ] **Step 5:** 实现 `fetch_models` 主体(分支:base_url 解析 → key 解析 → reqwest GET → 错误分类 → schema 解析)
- [ ] **Step 6:** 跑测试,逐个绿化
- [ ] **Step 7:** 重点 verify 测试 8(错误不含 key)— 这是安全断言,失败要回到实现层修
- [ ] **Step 8:** `cargo test -p gitim-runtime hermes_llm` 三类(registry/introspect/fetch)全绿
- [ ] **Step 9:** Commit `feat(runtime): live-fetch /models with structured error fallback`

---

## Task 4: MeJson 加 llm_provider + llm_model 字段

**Files:**
- Modify: `crates/gitim-core/src/me_json.rs`
- Test: `me_json.rs` 内联 `#[cfg(test)] mod tests`(若已有就追加;否则建)

**变更描述:**
- `MeJson` struct 加两个字段(放在现有 `model` / `system_prompt` / `env` 同一组下方):
  ```
  /// Hermes-internal LLM provider id (e.g. `minimax-cn`, `custom:my-glm`).
  /// Only meaningful when `provider == "hermes"`. None for other providers.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub llm_provider: Option<String>,

  /// Hermes-internal LLM model id (e.g. `MiniMax-M2.7-highspeed`).
  /// Only meaningful when `provider == "hermes"`. None for other providers.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub llm_model: Option<String>,
  ```
- `merged_with`:加两个 `if patch.llm_provider.is_some() { self.llm_provider = patch.llm_provider; }` 块,跟现有 `model` 处理一致(merge 语义,daemon 已有先例)

**测试:**
1. `serde_roundtrip_includes_llm_fields` —— 构造 MeJson { llm_provider: Some("minimax-cn"), llm_model: Some("MiniMax-M2.7-highspeed"), ... },serialize → deserialize → 字段一致
2. `serde_skip_serializing_when_none` —— None 时 JSON 不出现该 key
3. `merged_with_overrides_llm_fields_when_some` —— base + patch(都有 llm_provider),patch 优先
4. `merged_with_preserves_llm_fields_when_patch_none` —— base 有 llm_provider,patch.llm_provider=None,merged 保留 base 值
5. `forward_compat_unknown_field_preserved` —— 现有 `extra: BTreeMap` 行为不破(老测试若有就跑一次)

**验收标准:**
- 5 个测试全绿
- `cargo test -p gitim-core` 全绿
- 现有 me.json 文件(老 agent 的)反序列化不破:加载老 me.json → llm_provider/llm_model 默认 None,其他字段 untouched

**Steps:**
- [ ] **Step 1:** 写 5 个失败测试
- [ ] **Step 2:** `cargo test -p gitim-core me_json` 失败
- [ ] **Step 3:** 修改 MeJson struct 加字段
- [ ] **Step 4:** 修改 merged_with 加两个 if 块
- [ ] **Step 5:** `cargo test -p gitim-core me_json` 全绿
- [ ] **Step 6:** `cargo test -p gitim-core` 全绿(确认没破现有测试)
- [ ] **Step 7:** Commit `feat(core): MeJson llm_provider + llm_model fields`

---

## Task 5: hermes_profile.apply_model_config — 3 次 shell out 序列

**Files:**
- Modify: `crates/gitim-runtime/src/hermes_profile.rs`
- Test: `crates/gitim-runtime/tests/hermes_profile.rs`(已存在,追加)

**变更描述:**
- 新增 `pub async fn apply_model_config(handler: &str, llm_provider: &str, llm_model: &str, base_url: Option<&str>) -> Result<(), HermesProfileError>`
- 内部:`apply_model_config_with(handler, llm_provider, llm_model, base_url, "hermes").await` (跟现有 ensure_profile / delete_profile 同款 testability 模式)
- `apply_model_config_with(handler, llm_provider, llm_model, base_url, bin)`:
  1. 顺序 spawn:
     - `<bin> -p gitim-<handler> config set model.provider <llm_provider>`
     - `<bin> -p gitim-<handler> config set model.default <llm_model>`
     - 仅 `base_url` 是 Some 时:`<bin> -p gitim-<handler> config set model.base_url <url>`
  2. 任一步退出码非 0 → 返回 `HermesProfileError::Other(format!("config set {key} failed: {stderr}"))`
  3. binary 不存在 → `HermesProfileError::CliNotFound`(对称 ensure_profile 的处理)

**测试(追加到 `tests/hermes_profile.rs`):**
1. `apply_model_config_with_nonexistent_binary_returns_cli_not_found` —— bin="/nonexistent/xyz",assert CliNotFound
2. `apply_model_config_with_failing_binary_returns_other_error` —— bin="/bin/false",assert Other 错误
3. `apply_model_config_step1_failure_does_not_run_step2` —— 用一个 fake bin script 第一次返非 0,验证 stderr 含 "model.provider"(不含 "model.default" — 因为没跑到那步)。可选,如果难写就降为手动验证
4. `apply_model_config_skips_base_url_when_none` —— base_url=None 时,只跑 2 次 shell out(可观察:用 fake bin 计数 invocation 次数,或捕获 stdout)
5. **集成测试 #[ignore]:** `apply_model_config_real_writes_config_yaml` —— 真实 hermes binary,先 ensure_profile + apply_model_config,然后 cat profile/config.yaml 断言 model.provider/model.default 写入

**验收标准:**
- 4 个 unit 测试全绿
- 集成测试 `--ignored` 手动跑通:`cargo test -p gitim-runtime --test hermes_profile -- --ignored apply_model_config_real`
- `cargo clippy` 无新 warning
- `apply_model_config` 函数 doc 注明 "On any failure, caller MUST `delete_profile` to avoid partial state — see add_agent flow"

**Steps:**
- [ ] **Step 1:** 写 4 个 unit 测试 + 1 个 ignored 集成测试
- [ ] **Step 2:** `cargo test -p gitim-runtime --test hermes_profile` 失败
- [ ] **Step 3:** 实现 `apply_model_config_with` + `apply_model_config` 包装
- [ ] **Step 4:** unit 测试全绿
- [ ] **Step 5:** 手动跑 `--ignored` 测试,verify 真实 hermes 写 yaml(参考 Task 0 step 1 的命令验证手感一致)
- [ ] **Step 6:** Commit `feat(runtime): hermes_profile::apply_model_config — 3-step shell out sequence`

---

## Task 6: GET /hermes/llm/providers handler

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(加 handler + 路由注册)
- Test: `crates/gitim-runtime/tests/hermes_llm_http.rs`(新建)

**变更描述:**
- 新增 `async fn list_hermes_llm_providers(State(state): State<SharedRuntimeState>) -> impl IntoResponse`
  - 解析 hermes_home:`std::env::var_os("HERMES_HOME").map(PathBuf::from).unwrap_or_else(|| dirs::home_dir().unwrap().join(".hermes"))`
  - 调 `crate::hermes_llm::list_providers(&hermes_home)`
  - 返回 `(StatusCode::OK, Json(json!({ "providers": providers })))`
- 路由注册:`.route("/hermes/llm/providers", axum::routing::get(list_hermes_llm_providers))`

**测试(`tests/hermes_llm_http.rs`):**
1. `get_providers_empty_when_no_hermes_home` —— 启动 runtime with `HERMES_HOME=<empty tempdir>`,GET /hermes/llm/providers,assert `{"providers": []}`
2. `get_providers_lists_env_configured` —— 写 .env 含 KIMI_API_KEY,assert 响应 providers 含 id=kimi-coding
3. `get_providers_includes_custom` —— 写 config.yaml 含 custom_providers,assert 响应含 id=custom:foo
4. `get_providers_status_200` —— 总是 200(任何失败都不 5xx)

**验收标准:**
- 4 个测试全绿
- 手动 `curl http://127.0.0.1:<port>/hermes/llm/providers` 返合法 JSON
- `cargo clippy` 无新 warning

**Steps:**
- [ ] **Step 1:** 写 4 个失败测试(用现有 runtime test harness — 参考 `runtime_http.rs` 启动 helper,如不存在,看 `tests/preflight*.rs` 学怎么 spawn runtime + 拿 port)
- [ ] **Step 2:** `cargo test -p gitim-runtime --test hermes_llm_http` 失败
- [ ] **Step 3:** 实现 handler + 路由注册
- [ ] **Step 4:** 测试全绿
- [ ] **Step 5:** 手动 curl 验证
- [ ] **Step 6:** Commit `feat(runtime): GET /hermes/llm/providers endpoint`

---

## Task 7: GET /hermes/llm/providers/{id}/models handler

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`
- Test: `crates/gitim-runtime/tests/hermes_llm_http.rs`(追加)

**变更描述:**
- 新增 `async fn list_hermes_llm_models(Path(provider_id): Path<String>, State(state): State<SharedRuntimeState>) -> impl IntoResponse`
  - 解析 hermes_home(同 Task 6)
  - 解析 provider_id:
    - 在 BUILTIN_PROVIDERS 找 → 构造 LlmProvider(kind=ApiKey)
    - 形如 `custom:<name>` → 读 config.yaml 找,构造 LlmProvider(kind=Custom)
    - 都不是 → return 400 + `{"error": "unknown provider id"}`
  - 调 `crate::hermes_llm::fetch_models(&provider, &hermes_home).await`
  - 返回 200 + `Json(result)`(无论 result.error 是否 Some,status 永远 200)
- 路由注册:`.route("/hermes/llm/providers/:id/models", get(list_hermes_llm_models))`

**测试(追加 `tests/hermes_llm_http.rs`):**
5. `get_models_unknown_provider_400` —— GET /hermes/llm/providers/totally-fake/models,assert 400
6. `get_models_builtin_returns_shape` —— 用 mock server 替代 hermes provider base_url(可能要让 BUILTIN_PROVIDERS 接受 env override 用于测试,或者只测 error path);GET 含 `error` 字段(没 key 时);v1 不强测 happy path,因为要 mock builtin base_url 比较复杂,挪到 e2e
7. `get_models_custom_provider_returns_shape` —— 写 custom_providers entry 含 base_url 指向 httpmock,GET 后 assert `{"models": [...], "error": null}`(成功 path)
8. `get_models_status_always_200_even_on_upstream_failure` —— mock 返 500,assert HTTP 200 + body.error 非空

**验收标准:**
- 4 个新测试全绿(`hermes_llm_http` 共 8 个)
- 手动 curl 验证
- `cargo clippy` 无新 warning

**Steps:**
- [ ] **Step 1:** 写 4 个失败测试
- [ ] **Step 2:** `cargo test -p gitim-runtime --test hermes_llm_http` 失败
- [ ] **Step 3:** 实现 handler(provider_id 解析 + 调 fetch_models + status 永远 200)
- [ ] **Step 4:** 路由注册
- [ ] **Step 5:** 测试全绿
- [ ] **Step 6:** 手动 curl 验证 builtin 路径(没 key 时返 error 字段)+ custom 路径(用本地 httpmock 临时模拟)
- [ ] **Step 7:** Commit `feat(runtime): GET /hermes/llm/providers/{id}/models endpoint`

---

## Task 8: 升级 add_agent — provider=hermes 校验 + apply_model_config + me.json + 回滚

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(`AgentAddRequest` + `agents_add` handler)
- Test: `crates/gitim-runtime/tests/hermes_llm_http.rs`(追加 `agents_add` 相关 cases)

**变更描述:**
- `AgentAddRequest` 加两个字段:
  ```
  #[serde(default)]
  pub llm_provider: Option<String>,
  #[serde(default)]
  pub llm_model: Option<String>,
  ```
- `agents_add` handler 在 `req.provider == "hermes"` 分支,**ensure_profile 之后、agent_loop 启动之前** 插入新逻辑:
  1. 校验:llm_provider/llm_model 有任一 missing/empty → 400 `{"error": "missing llm_provider/llm_model for hermes"}`
  2. 校验 llm_provider 在白名单:在 BUILTIN_PROVIDERS,或 `custom:<name>` 且 name 在 config.yaml.custom_providers → 否则 400 `{"error": "unknown llm_provider"}` 或 `{"error": "custom provider <name> not found"}`
  3. 解析 base_url:builtin → None(让 hermes 的 default 生效);custom → Some(<from custom_providers>)
  4. 调 `apply_model_config(handler, llm_provider, llm_model, base_url).await`
  5. 失败 → `delete_profile(handler).await`(best-effort) + `cleanup_agent_dir(handler)` + 返回 500 + actionable error message
  6. 成功 → me.json 写入加 `llm_provider` + `llm_model` 字段(merge 到现有 MeJson 写入逻辑;参考 me.json 现有 provider/model 写入位置 line ~1342-1357)

**测试(追加):**
9. `agents_add_hermes_missing_llm_provider_400` —— body provider=hermes,缺 llm_provider,assert 400 + error 文案
10. `agents_add_hermes_unknown_llm_provider_400` —— body llm_provider="not-a-thing",assert 400
11. `agents_add_hermes_custom_provider_not_in_config_400` —— body llm_provider="custom:nonexistent",assert 400
12. `agents_add_hermes_happy_path_writes_me_json` —— 用 fake hermes binary(env override `HERMES_BIN_OVERRIDE` 或 mock script,具体由 hermes_profile 现有测试模式决定),add_agent 成功后 read me.json 断言 llm_provider/llm_model 写入
13. `agents_add_hermes_apply_model_config_failure_rollbacks` —— fake hermes binary 第一次成功(ensure_profile),第二次失败(apply_model_config),assert agent dir 不存在 + profile 被删 + 响应 500 + error 文案

**验收标准:**
- 5 个新测试全绿
- 现有 add_agent 测试(claude/codex/mock provider)不破
- 手动:curl 完整 happy path 成功创建 hermes agent + 检查 ~/.hermes/profiles/gitim-<h>/config.yaml 含 model.provider/model.default

**Steps:**
- [ ] **Step 1:** 写 5 个失败测试
- [ ] **Step 2:** `cargo test -p gitim-runtime --test hermes_llm_http` 5 个新测试失败
- [ ] **Step 3:** 改 AgentAddRequest 加字段
- [ ] **Step 4:** 在 agents_add handler 插入校验 + apply_model_config + 回滚 + me.json 字段
- [ ] **Step 5:** 测试逐个绿化
- [ ] **Step 6:** `cargo test -p gitim-runtime` 全绿(确认没破其他)
- [ ] **Step 7:** 手动 e2e:`curl -X POST .../agents/add -d '{"handler":"alice","provider":"hermes","llm_provider":"minimax-cn","llm_model":"MiniMax-M2.7-highspeed",...}'`,verify 文件落地
- [ ] **Step 8:** Commit `feat(runtime): add_agent supports hermes llm_provider/llm_model with rollback`

---

## Task 9: preflight_hermes_with 升级 — 接受 llm_provider/llm_model

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs`(`preflight_hermes_with` 签名升级)
- Test: `crates/gitim-runtime/tests/preflight_hermes.rs`(扩展)

**变更描述:**
- `preflight_hermes_with` 当前签名带 `hermes_home: Option<&Path>` —— 加两个参数 `llm_provider: Option<&str>, llm_model: Option<&str>`
- `preflight_hermes`(无参版)继续传 None / None / None,**保持向后兼容**(老调用者不改)
- 实现:在拼 `hermes chat ...` 命令参数时,如果两个 llm 参数都 Some,追加 `--provider <X> --model <Y>`
- hermes_home 仍然可选——v1 在 default profile 上跑(L7 决定,不创建临时 profile)
- 验证逻辑不变(找 GITIM_OK 字符串)

**测试:**
1. `preflight_hermes_with_llm_overrides_passes_args` —— 用 fake bin (echo argv),assert argv 含 "--provider minimax-cn" "--model M2.7"。可以让 fake bin 是 `/bin/echo` 或写个一行 shell 脚本
2. `preflight_hermes_with_no_llm_overrides_omits_args` —— 不传 llm_*,assert argv 不含 "--provider"
3. **集成测试 #[ignore]:** `preflight_hermes_with_real_minimax_succeeds` —— 真实 hermes + 真实 minimax-cn key,assert available=true(用户当前 default 已配 minimax-cn,这个测试在你机器上能跑通)

**验收标准:**
- 2 个 unit 测试 + 1 个 ignored 集成测试编写
- `cargo test -p gitim-runtime --test preflight_hermes` unit 全绿
- `cargo test -p gitim-runtime --test preflight_hermes -- --ignored preflight_hermes_with_real_minimax_succeeds` 手动跑通

**Steps:**
- [ ] **Step 1:** 写 3 个测试(2 unit + 1 ignored)
- [ ] **Step 2:** `cargo test -p gitim-runtime --test preflight_hermes` unit 失败(签名不匹配)
- [ ] **Step 3:** 改 preflight_hermes_with 签名 + 拼参数逻辑
- [ ] **Step 4:** 修复其他调用点(preflight_hermes 包装函数 + 任何 imported caller)
- [ ] **Step 5:** unit 测试全绿
- [ ] **Step 6:** 手动跑 ignored 测试 verify
- [ ] **Step 7:** Commit `feat(runtime): preflight_hermes_with accepts llm_provider/llm_model`

---

## Task 10: preflight_handler 接受 query param

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(`preflight_handler`)
- Test: `crates/gitim-runtime/tests/runtime_http.rs`(若存在,扩展;否则用 hermes_llm_http.rs)

**变更描述:**
- `preflight_handler(Path(provider): Path<String>, Query(params): Query<PreflightQuery>) -> impl IntoResponse`
- 新结构 `PreflightQuery { llm_provider: Option<String>, llm_model: Option<String> }`,derive Deserialize
- 在 `provider == "hermes"` 分支,把 `params.llm_provider.as_deref()` + `params.llm_model.as_deref()` 传给 `preflight_hermes_with(None, llm_provider, llm_model)`
- claude / codex 分支保持现状(忽略 query param)

**测试:**
1. `preflight_hermes_query_param_passed_through` —— mock preflight_hermes_with 或在 PreflightResult 里捕获 model_used,assert 用了 query 里的 model;如果 mock 困难,降级为"调 GET /preflight/hermes?llm_provider=minimax-cn&llm_model=foo,assert 200 + provider 字段"+`#[ignore]` 跑真实
2. `preflight_other_provider_ignores_query_params` —— GET /preflight/claude?llm_provider=anything,assert 跟没 query 时行为一致

**验收标准:**
- 2 个测试编写,至少 1 个 unit 绿
- 手动 curl `/preflight/hermes?llm_provider=minimax-cn&llm_model=MiniMax-M2.7-highspeed` 返合法 PreflightResult

**Steps:**
- [ ] **Step 1:** 写 2 个测试
- [ ] **Step 2:** `cargo test -p gitim-runtime` preflight handler 测试失败
- [ ] **Step 3:** 改 preflight_handler 签名 + 调用 preflight_hermes_with 传 llm 参数
- [ ] **Step 4:** 测试绿
- [ ] **Step 5:** 手动 curl
- [ ] **Step 6:** Commit `feat(runtime): preflight_handler accepts llm_provider/llm_model query`

---

## Task 11: webui-v2 hermes-llm.ts — 类型 + helper

**Files:**
- Create: `webui-v2/src/lib/hermes-llm.ts`
- Modify: `webui-v2/src/lib/types.ts`(若 Agent 类型在这,加 `llmProvider?` / `llmModel?`)

**变更描述:**
- 定义 TypeScript 类型镜像后端 shape:
  - `HermesLlmProviderKind = "api_key" | "custom"`
  - `HermesLlmProvider { id: string; label: string; kind: HermesLlmProviderKind; base_url?: string }`
  - `HermesLlmModel { id: string; label: string }`
  - `HermesLlmModelList { models: HermesLlmModel[]; custom_allowed: boolean; error: string | null; fetched_at_ms: number }`
- 静态 builtin label fallback:`BUILTIN_LABELS: Record<string, string>`(6 项,用于前端显示美化但不重复 base_url)— 万一后端返 label 不友好,前端可叠加;v1 简化版可以省略,直接用后端返的 label
- helper:`isCustomProvider(id: string): boolean` 返 `id.startsWith("custom:")`

**测试:**
v1 不强加 webui 单元测试(multi-provider plan 也没有,前端测试基础设施缺)。

**验收标准:**
- `pnpm -C webui-v2 exec tsc --noEmit` 通过
- 其他文件 import 这些类型不报错

**Steps:**
- [ ] **Step 1:** 创建 hermes-llm.ts
- [ ] **Step 2:** 在 types.ts 加 Agent.llmProvider / Agent.llmModel
- [ ] **Step 3:** `pnpm -C webui-v2 exec tsc --noEmit`
- [ ] **Step 4:** Commit `feat(webui): hermes LLM types and helpers`

---

## Task 12: client.ts 接入新 endpoint

**Files:**
- Modify: `webui-v2/src/lib/client.ts`

**变更描述:**
- 加新方法:
  - `listHermesLlmProviders(): Promise<ApiResponse<{ providers: HermesLlmProvider[] }>>` — GET `/hermes/llm/providers`
  - `listHermesLlmModels(providerId: string): Promise<ApiResponse<HermesLlmModelList>>` — GET `/hermes/llm/providers/${encodeURIComponent(providerId)}/models`
- 升级 `preflightProvider`:接受可选 `{ llmProvider?, llmModel? }` 第二参数,hermes 时拼 `?llm_provider=X&llm_model=Y` query
- 升级 `addAgent`:接受 `llmProvider?` / `llmModel?` 字段;body 里加这俩(snake_case → JSON `llm_provider` / `llm_model` 跟后端 serde 对齐)
- mock client 同步签名(传入忽略,空 list / mock success 即可)
- `mapBackendAgent` 加 `llmProvider: backend.llm_provider`、`llmModel: backend.llm_model`

**验收标准:**
- `tsc --noEmit` 通过
- `addAgent` 调用方 type-check 不报错(尚未传新字段也 OK,因为可选)

**Steps:**
- [ ] **Step 1:** 加两个新方法 + 升级 preflightProvider 签名
- [ ] **Step 2:** 升级 addAgent 签名 + body 字段
- [ ] **Step 3:** mapBackendAgent 加字段
- [ ] **Step 4:** `tsc --noEmit`
- [ ] **Step 5:** Commit `feat(webui): client supports hermes LLM provider/model selection`

---

## Task 13: AddAgentDialog 内嵌 LLM 段 + state machine

**Files:**
- Modify: `webui-v2/src/components/management/add-agent-dialog.tsx`

**变更描述:**
- 新增 state:
  - `llmProvider: string`(默认 "")
  - `llmModel: string`(默认 "")
  - `llmProviders: HermesLlmProvider[]`(default [])
  - `llmProvidersLoading: boolean`
  - `llmModels: HermesLlmModel[]`(default [])
  - `llmModelsLoading: boolean`
  - `llmModelsError: string | null`
  - `customModelInput: string`(用户在 "Custom..." 选择后的输入框值)
- useEffect 监听 GitIM provider:
  - 切到 hermes → 调 `listHermesLlmProviders`,填 `llmProviders`
  - 切走 → 重置所有 llm-* state
- useEffect 监听 llmProvider 变化:
  - 非空 → 调 `listHermesLlmModels(llmProvider)`,填 `llmModels` + `llmModelsError`
  - llmModel state 重置为 ""
- UI(在现有 fields 之后,Detect 按钮之前插入):
  - 仅当 GitIM provider == "hermes" 时显示
  - 段标题 "Hermes LLM"
  - LLM Provider select:options 来自 llmProviders;空 list 时显示 helper 文案 "No LLM providers configured. Add an API key to ~/.hermes/.env or run hermes setup, then reopen this dialog."
  - LLM Model select:options 来自 llmModels;末尾固定 "Custom..." 选项;选 Custom 后变 input;`llmModelsError` 非空时强制 input mode + 显示 error 在 select 旁
- Submit 条件加入:
  - GitIM provider == hermes 时,llmProvider 非空 + llmModel 非空(或 customModelInput 非空)
- Submit 时若选了 Custom,把 customModelInput 当作 llmModel 传 client.addAgent
- Detect 按钮升级:provider==hermes 时调用 `preflightProvider("hermes", { llmProvider, llmModel: effectiveModel })`(其中 effectiveModel = customModelInput 或 llmModel,取非空者)
- Dialog `onOpenChange(false)` 重置全部 hermes-LLM state

**验收标准:**
- `tsc --noEmit` 通过
- 手动 QA:
  - GitIM provider=Hermes → LLM Provider dropdown 出现,fetch loading → 显示 introspection 结果
  - 选 minimax-cn → LLM Model dropdown 自动 fetch live → 显示 model 列表
  - 选某个 model → Detect 按钮 enabled → 点击 → 显示绿勾或红叉
  - Detect 通过 → Add 按钮 enabled
  - 切回 GitIM provider=Claude → LLM 段消失,state 清掉
  - 关闭 Dialog 再开 → state 全部初始

**Steps:**
- [ ] **Step 1:** 加 state hooks
- [ ] **Step 2:** 加两个 useEffect(provider 切 → fetch providers;llmProvider 切 → fetch models)
- [ ] **Step 3:** 加 UI 段(仅 hermes 显示)
- [ ] **Step 4:** Submit 条件加 hermes-LLM 校验
- [ ] **Step 5:** Detect 按钮升级传 llm 参数
- [ ] **Step 6:** Dialog close reset
- [ ] **Step 7:** `tsc --noEmit`
- [ ] **Step 8:** 手动 QA 走一遍
- [ ] **Step 9:** Commit `feat(webui): AddAgentDialog hermes LLM provider/model selectors`

---

## Task 14: backend e2e — 完整 add_agent flow 断言

**Files:**
- Create: `crates/gitim-runtime/tests/hermes_llm_e2e.rs`

**变更描述:**
- 顶层 `#[ignore]` 守门(需要真实 hermes binary + 至少一个真实 LLM key)
- 测试单 case `full_add_hermes_agent_with_minimax_cn`:
  1. setup runtime + workspace + git init(参考现有 e2e helpers)
  2. POST /agents/add `{handler:"e2e-alice", provider:"hermes", llm_provider:"minimax-cn", llm_model:"MiniMax-M2.7-highspeed", system_prompt:"..."}`
  3. assert 200 + agent_id
  4. read `~/.hermes/profiles/gitim-e2e-alice/config.yaml` (yaml parse) → assert model.provider="minimax-cn" + model.default="MiniMax-M2.7-highspeed"
  5. read `<workspace>/<handler>/.gitim/me.json` → assert llm_provider/llm_model 字段
  6. cleanup:DELETE /agents/{id} + verify profile 被删

**验收标准:**
- `cargo test -p gitim-runtime --test hermes_llm_e2e -- --ignored` 在用户机器上手动跑通(default 已配 minimax-cn,有 key)
- 不要求 unattended CI 跑

**Steps:**
- [ ] **Step 1:** 写测试 case
- [ ] **Step 2:** 手动跑 verify
- [ ] **Step 3:** Commit `test(runtime): e2e hermes multi-LLM agent provisioning`

---

## Task 15: UI E2E — Playwright 完整流程

**Files:**
- Create: `e2e/tests/ui-hermes-llm.spec.ts`

**变更描述:**
- `test.describe(...).skip(!process.env.E2E_REAL_PROVIDERS, "set E2E_REAL_PROVIDERS=1 to enable")`
- `beforeAll`:`buildRuntime()` + `startEnv()`
- 单测 case:"user adds hermes agent with selected LLM via UI"
  1. `page.goto(baseURL)`
  2. 完成启动流程(参考现有 startup.spec.ts helper)
  3. 导航到 Agents/Management 页
  4. 点 "Add Agent" 打开 Dialog
  5. 填 Name / display_name
  6. Provider select 选 Hermes
  7. 等 LLM Provider dropdown 出现 + 填充(timeout 10s)
  8. 选 minimax-cn(或读 list 选第一个 builtin)
  9. 等 LLM Model dropdown 加载完(timeout 15s,因为 live fetch)
  10. 选 MiniMax-M2.7-highspeed
  11. 点 Detect → 等绿勾(timeout 30s,真实 LLM 调用)
  12. 点 Add → Dialog 关闭 → agent 出现在列表
- 测试 timeout 设 180_000(覆盖 detect + add)

**验收标准:**
- 设 `E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test ui-hermes-llm` 手动跑通
- 未设时 skip(不烧钱)

**Steps:**
- [ ] **Step 1:** 研究 AddAgentDialog 各字段的 accessibility name / data-testid,决定 locator 策略
- [ ] **Step 2:** 写测试 case
- [ ] **Step 3:** 手动跑 verify
- [ ] **Step 4:** Commit `test(e2e): UI hermes LLM provider/model selection`

---

## Task 16: CLAUDE.md + qa-checklist + 文档收尾

**Files:**
- Modify: `CLAUDE.md`(Current Orientation 段 + Non-goals 段)
- Create: `docs/plans/hermes-multi-llm/03-qa-checklist.md`
- 删除:`docs/plans/hermes-multi-llm/02-plan.md` 顶部的 Task 0 baseline 临时段落(实施完了应该清理)

**变更描述:**

**CLAUDE.md Current Orientation** —— 加一段在已有 Hermes profile 隔离描述后:
> **Hermes 多 LLM 选择**已落地:WebUI 加 hermes agent 时可选具体 LLM provider × model;后端 introspect `~/.hermes/.env` + `config.yaml.custom_providers` 列出已配 provider,live-fetch `/models` 拉模型列表,创建 profile 后顺序 `hermes config set model.{provider,default,base_url}` 写入。回滚保证:任一 config-set 步骤失败 → delete_profile + cleanup_agent_dir,无半残状态。

**CLAUDE.md Non-goals(本次不做):**
- OAuth 类 LLM provider(Nous / openai-codex)—— v2 处理,涉及 auth.json clone 和 active_provider 切换
- 已有 agent 的 retroactive LLM 配置 —— 用户手动 `hermes -p gitim-<h> config set` 迁移
- 创建后编辑 LLM —— 涉及 hot-reload + session-migration 语义,单独立 plan
- `BUILTIN_PROVIDERS` 表跟 hermes 源码 CI 同步校验 —— 半年人工 PR

**QA Checklist 至少覆盖:**
1. GitIM provider=Hermes 时 LLM section 出现,切走时消失
2. 用户已配 provider 在 dropdown 出现(.env 加 KIMI_API_KEY 重启 runtime 验证)
3. 用户**未**配 provider 不在 dropdown(空 .env 验证)
4. 选 LLM provider 后 model 列表 live fetch loading 提示
5. Live fetch 失败 → 显示 error + Custom 输入框
6. Custom 输入提交后 me.json 落字段
7. Detect 按钮带 llm 参数验证真实 (provider, model) handshake
8. Detect 失败 → Add disabled
9. Add 成功 → ~/.hermes/profiles/gitim-<h>/config.yaml model 子树写入
10. apply_model_config 中途失败(可手动 break:rename hermes binary 或改 PATH)→ profile 被删 + 错误提示
11. 已有 claude/codex agent 不受影响
12. Dialog close 后再开 state 重置
13. POST /agents/add provider=hermes 缺 llm_provider 字段 → 400(curl 验证)
14. GET /hermes/llm/providers 在 hermes_home 缺失时返 200 空列表
15. GET /hermes/llm/providers/<unknown>/models → 400

**验收标准:**
- CLAUDE.md 修改提交
- 03-qa-checklist.md 15+ 条
- Task 0 临时 baseline 段落已删
- 本地手动过一遍 QA 清单,全部通过

**Steps:**
- [ ] **Step 1:** CLAUDE.md 加 Current Orientation 段 + Non-goals 段
- [ ] **Step 2:** 创建 03-qa-checklist.md
- [ ] **Step 3:** 删除 02-plan.md 顶部 baseline 段(若 Task 0 加了)
- [ ] **Step 4:** 手动过 QA 清单
- [ ] **Step 5:** Commit `docs(hermes-multi-llm): CLAUDE.md update + QA checklist`

---

## 总体验收(Phase 结束条件)

- [ ] `cargo test --workspace`(不含 `--ignored`)全绿
- [ ] `cargo clippy -p gitim-runtime -p gitim-core` 无新增 warning
- [ ] `cargo test -p gitim-runtime --test hermes_profile -- --ignored` 手动跑通(需要真实 hermes)
- [ ] `cargo test -p gitim-runtime --test preflight_hermes -- --ignored preflight_hermes_with_real_minimax_succeeds` 手动跑通
- [ ] `cargo test -p gitim-runtime --test hermes_llm_e2e -- --ignored` 手动跑通
- [ ] `E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test ui-hermes-llm` 手动跑通
- [ ] 手动 QA 清单 15 条全过
- [ ] 前端 `tsc --noEmit` 通过
- [ ] 前端 `pnpm -C webui-v2 lint` 不引入新 warning
- [ ] CLAUDE.md Current Orientation 反映新功能
- [ ] 03-qa-checklist.md 存在
- [ ] 02-plan.md 顶部 Task 0 baseline 临时段落已删

## Out of Scope(明确不做,跟 spec Non-goals 一致)

- OAuth 类 LLM provider 支持
- 已有 agent retroactive LLM 配置
- 创建后编辑 LLM(PATCH 路径)
- LLM 配置走 git 同步
- model id 美化 / display_name 翻译
- Live fetch /models 缓存
- API key 健康主动探测
- BUILTIN_PROVIDERS CI 同步校验
- Custom provider 在 WebUI 编辑 base_url 的入口

## Security posture

- API key 只用于 `/models` 请求的 Authorization header,不回传前端
- `error` 字段绝不含 key 字面量(Task 3 测试 8 显式断言)
- HTTP fetch 5s timeout 防 hang
- shell out 命令参数全部用 args[],不拼 shell 字符串(防 injection — handler / llm_provider / llm_model 进 hermes 子进程都是 argv 传)
- me.json 写 chmod 0600 跟现有 hermes profile 隔离 plan 对称
