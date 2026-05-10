# Agent Token Usage Statistics — Implementation Plan

**Design**: [`design.md`](design.md)
**Branch**: `claude/loving-nobel-eb573a`
**Approach**: TDD per task。每个 task 先写测试 / 修测试,再改实现,跑测试通过再 commit。Task 之间有强依赖 —— 先底层(provider trait)再中层(state / agent_loop)再上层(HTTP / WebUI),实现顺序就是 task 编号顺序。

## 前置 baseline

Task 0 之前必须跑一次全量 `cargo test --workspace`,**确认 main 是绿的**。如果有红测试,先记录在案,后续任务的 regression 判断要 exclude 这些;不要试图先修。

## Task 列表

### Task 1 · Provider trait 加语义声明 method

**目标**:`Provider` trait 增加两个 method 让各 provider 声明 usage 语义,移除 agent_loop 里 `provider_reports_usage` 的 hardcode。

**改动文件**:
- `crates/gitim-agent-provider/src/provider.rs` — Provider trait 加默认实现 `reports_usage()` / `usage_is_cumulative()`
- `crates/gitim-agent-provider/src/claude/mod.rs` — override `usage_is_cumulative() = false`
- `crates/gitim-agent-provider/src/codex/mod.rs` — override `usage_is_cumulative() = true`
- `crates/gitim-agent-provider/src/opencode/mod.rs` — override `usage_is_cumulative() = false`
- `crates/gitim-agent-provider/src/pi/mod.rs` — override `usage_is_cumulative() = false`
- `crates/gitim-agent-provider/src/hermes/mod.rs` — 暂声明 `usage_is_cumulative() = false`,Task 2 验证后调整
- `crates/gitim-agent-provider/src/gemini/mod.rs` — override `reports_usage() = false`
- `crates/gitim-agent-provider/src/openclaw/mod.rs` — override `reports_usage() = false`
- `crates/gitim-agent-provider/src/mock/mod.rs` — 加 setter 让测试控制返回值

**测试**:
- `crates/gitim-agent-provider/tests/` 新增 `provider_trait_declarations.rs` 单测,assert 每个 provider 的 trait method 返回值
- 现有 provider 测试 baseline 保持绿

**验证命令**:`cargo test -p gitim-agent-provider`

**Commit message** 模板:`feat(provider): declare reports_usage and usage_is_cumulative on Provider trait`

---

### Task 2 · Hermes provider usage 语义审计

**目标**:确认 hermes 的 `result.usage` 是 turn 增量还是 session 累计,据此调整 Task 1 中 `usage_is_cumulative()` 声明。明确 mid-stream usage_update events 不进入 result.usage。

**操作步骤**:
1. 阅读 `crates/gitim-agent-provider/src/hermes/mod.rs` 现有 usage_update 解析(line 239-280, 320, 405, 568)
2. 写新测试 `crates/gitim-agent-provider/tests/hermes_usage_semantics_test.rs` —— 模拟连续两 turn,断言 result.usage 行为(用 mock SDK / fixture data)
3. 根据观察行为,调整 hermes mod.rs 的 `usage_is_cumulative()` 返回值
4. 如果发现 mid-stream events 当前会污染 result.usage,加代码区分:`latest_usage` 只跟随 result.usage,mid-stream events 单独 broadcast

**改动文件**:
- `crates/gitim-agent-provider/src/hermes/mod.rs` — 可能需要分离 mid-stream event 处理路径
- `crates/gitim-agent-provider/tests/hermes_usage_semantics_test.rs`(新)

**验证命令**:`cargo test -p gitim-agent-provider hermes`

**Commit message** 模板:`fix(provider/hermes): pin result.usage to turn-final value, exclude mid-stream events`

---

### Task 3 · `AgentState` 加 `last_session_usage`

**目标**:在持久化的 `AgentState` 上加 normalize baseline 字段。生命周期跟 session_token 绑,session 切换 / reset 时清零。

**改动文件**:
- `crates/gitim-runtime/src/state.rs` — `AgentState` 加 `last_session_usage: Option<LastSessionUsage>` 字段,`LastSessionUsage { session_id: String, usage: ProviderUsage }`;`AgentState::reset_session()` 清零该字段;`save` / `load` 通过 serde 自动处理

**测试**:
- `crates/gitim-runtime/src/state.rs` 内联测试 —— round-trip 序列化(包含 `last_session_usage`);`reset_session` 行为
- 既有 state 测试不破

**验证命令**:`cargo test -p gitim-runtime state`

**Commit message** 模板:`feat(runtime/state): persist last_session_usage baseline for cumulative providers`

---

### Task 4 · `usage_log.rs` 模块(独立持久化)

**目标**:实现 `crates/gitim-runtime/src/usage_log.rs` 完整模块,跟 `agent-state.json` 物理解耦。

**改动文件**:
- `crates/gitim-runtime/src/usage_log.rs`(新文件) — 包含 `AgentUsageLog`、`UsageBucket`、`UsageSummary`、`DayEntry` 类型 + 完整 impl
- `crates/gitim-runtime/src/lib.rs` — 加 `pub mod usage_log`

**模块要点**(对应 design.md 第 2b 节):
- 文件路径函数 `path(workspace_root, handler)`
- `load_or_default(...)`:文件不存在 → 默认值;JSON 解析失败 → log error + 默认值;成功 → 反序列化
- `accumulate(today, delta: Option<&ProviderUsage>, now_iso)`:turns +=1;`last_updated = max(prev, now_iso)`;首次创建 `first_seen = now_iso`;`provider_reports_usage = false` 跳过 token 累加;saturating_add 4 字段
- `save(&self, workspace_root)`:`prune_by_day` → 序列化 → 原子写(`*.tmp` + rename)→ chmod 0600
- `delete(workspace_root, handler)`:静态方法,best-effort
- `prune_by_day(today)`:删 `today - 90 days` 之前的 entries
- `last_30_days(today) -> Vec<DayEntry>`:补 0 填齐 30 天窗口
- `summary(today) -> UsageSummary`:转换为 HTTP 暴露 view shape

**测试**(`crates/gitim-runtime/src/usage_log.rs` 内联 `#[cfg(test)] mod tests`):
- round-trip serde
- lazy init(tmpdir 文件不存在)
- accumulate 增量行为(input/output/cache_read/cache_creation/turns 各加正确)
- `provider_reports_usage = false` 只加 turns
- 90 天滚动裁剪(造 100 天数据,save 后留 90)
- last_30_days 补 0 行为(造稀疏数据,断言 30 行齐整)
- chmod 0600 验证(`fs::metadata().permissions().mode() & 0o777 == 0o600`)
- last_updated clock-jump 防御(`now_iso < self.last_updated` 时不回退)
- delete 静态方法 happy path + 文件不存在 best-effort
- summary() 转换 shape 正确

**验证命令**:`cargo test -p gitim-runtime usage_log`

**Commit message** 模板:`feat(runtime): add usage_log module for per-agent token accumulator`

---

### Task 5 · `agent_loop` normalize + accumulate hook

**目标**:在 `update_session_usage()` 末尾追加 normalize(cumulative → delta + saturating_sub)+ accumulate(usage_log)+ patch in-memory state + 扩展 SSE event。

**改动文件**:
- `crates/gitim-runtime/src/agent_loop.rs` — 扩展 `update_session_usage` 签名(加 `provider: &dyn Provider`、`workspace_root: &Path`、`agent_state: &mut AgentState`、`session_id: &str` 等;具体看 closure capture 现状)
- `crates/gitim-runtime/src/state.rs` — `SharedRuntimeState` 加 `usage_save_failures: AtomicU64`;`AgentInfo` 加 `usage_summary: Option<UsageSummary>` 字段(serde derive 包括 Option = None 时 skip 序列化以保持兼容);broadcast helper 函数 `broadcast_usage_event`(扩展现有 `"usage"` event 推送,加 sibling `usage_summary` 字段)

**核心逻辑**(伪码,实现见 design.md 第 3 节):
1. `if !provider.reports_usage()` → delta = None;else if `provider.usage_is_cumulative()` → 走 saturating_sub normalize 路径,更新 agent_state.last_session_usage;else → delta = provider_reported.cloned()
2. `AgentUsageLog::load_or_default → accumulate → save`;失败 fetch_add usage_save_failures
3. `summary = log.summary(today)`,patch `runtime_state.workspaces[slug].agents[handler].usage_summary`
4. broadcast SSE event:既有 SessionUsageSnapshot 字段保 inline,加 sibling `usage_summary`

**测试**:
- `crates/gitim-runtime/src/agent_loop.rs` 内联测试 —— mock provider(用 `mock/mod.rs` 的 setter)的 cumulative + incremental 两种语义,断言 normalize 后的 delta 正确;断言 agent_state.last_session_usage 被更新;断言 session_id 变化时 baseline 清零;断言 negative cache_read delta 触发 saturating_sub + warn log
- 失败 save 不 panic + counter 自增

**验证命令**:`cargo test -p gitim-runtime agent_loop`

**Commit message** 模板:`feat(runtime/agent_loop): normalize provider usage and accumulate to usage_log`

---

### Task 6 · HTTP 暴露 + workspace startup load + hard delete + /health

**目标**:把 `usage_summary` 通过 HTTP / SSE 暴露;workspace recovery 在插入 ctx.agents 前 load 完 usage 文件;hard delete 一起清掉 usage 文件;`/runtime/health` 暴露 `usage_save_failures`。

**改动文件**:
- `crates/gitim-runtime/src/http.rs`:
  - `agents_list` / `agents_get` 的 response 包含新字段 `usage_summary`(via `AgentInfo` 已经在 Task 5 加了字段,这里只需序列化路径过得去)
  - `recover_agents_for_workspace`:在插入 ctx.agents 前,扫 `<workspace>/.gitim-runtime/usage/<handler>.json` 加载并 patch in-memory `AgentInfo.usage_summary`
  - `agents_remove` 的 hard delete 路径:在 `hard_delete_agent_dir` 之后追加 `let _ = AgentUsageLog::delete(&workspace_root, &handler);`
  - `/runtime/health` 端点 response 新增 `usage_save_failures` 字段

**测试**:
- `crates/gitim-runtime/tests/` 既有 HTTP 集成测试加断言:`GET /agents` 返回的 AgentInfo 含 `usage_summary`(空 agent 时 = None);30 天 by_day 补 0 填齐
- `/runtime/health` 返回含 `usage_save_failures`
- workspace recovery race 测试(模拟先有 usage 文件,recovery 后立即一个 turn,断言文件里数据被 in-memory 累加而不是覆盖)
- hard delete 路径测试:删 agent → usage 文件不存在 → re-add 同 handler → `usage_summary == None`(直到首个 turn)

**验证命令**:`cargo test -p gitim-runtime http`(或对应 test 文件名)

**Commit message** 模板:`feat(runtime/http): expose usage_summary on agents and health endpoint`

---

### Task 7 · E2E round-trip 测试

**目标**:加一个跨层测试,确保从 mock provider 喂数据到 GET /agents response 一气呵成。

**改动文件**:
- `crates/gitim-runtime/tests/usage_e2e.rs`(新)— 启动真实 runtime,wire mock provider(配置 `reports_usage=true, usage_is_cumulative=true` 一组,`reports_usage=true, usage_is_cumulative=false` 一组),驱动 turn,断言:
  - 第一次 turn 后 `GET /agents` 的 `usage_summary.totals` 等于第一次 turn 的 token 数
  - 第二次 turn 后 cumulative provider 的 `totals.input` 等于两次 turn 增量之和(不是双计)
  - SSE event payload 同时含 SessionUsageSnapshot 字段(inline)和 `usage_summary` sibling
  - `<workspace>/.gitim-runtime/usage/<handler>.json` 真实落盘且 chmod 0600

**验证命令**:`cargo test -p gitim-runtime --test usage_e2e`

**Commit message** 模板:`test(runtime): add e2e round-trip for token usage tracking`

---

### Task 8 · Frontend SSE handler 兼容性

**目标**:扩展 `use-agent-activity.ts` 让它消费新的 `usage_summary` sibling 字段,同时保持老逻辑(SessionUsageSnapshot inline 字段)正常工作。

**改动文件**:
- `products/gitim/frontend/src/hooks/use-agent-activity.ts` — destructure 时同时读 SessionUsageSnapshot 字段(已有)+ `usage_summary` sibling(新);patch agent store 时分别更新 `sessionUsage` 和 `usageSummary`(简版,只 totals + today + last_updated)

**测试**:
- `products/gitim/frontend/src/hooks/use-agent-activity.test.ts`(若不存在则新增)— 模拟 SSE event payload 两种 shape:① 老 shape(只有 inline 字段)② 新 shape(inline + usage_summary sibling),断言两种都不 crash

**验证命令**:`cd products/gitim/frontend && npm run test -- use-agent-activity`

**Commit message** 模板:`feat(frontend): consume usage_summary sibling on SSE usage events`

---

### Task 9 · Frontend 数据格式化 util

**目标**:实现 `format-tokens.ts` 工具函数。

**改动文件**:
- `products/gitim/frontend/src/lib/format-tokens.ts`(新) — `formatTokens(n: number): string`,行为:`< 1000 → "12"`、`1000+ → "12.3K"`、`1_000_000+ → "1.2M"`,保 1 位精度(`.toFixed(1)` + 去掉 `.0`)

**测试**:
- `products/gitim/frontend/src/lib/format-tokens.test.ts` — 边界(0 / 999 / 1000 / 1500 / 1_000_000 / 1_500_000)+ 不溢出大数

**验证命令**:`cd products/gitim/frontend && npm run test -- format-tokens`

**Commit message** 模板:`feat(frontend): add format-tokens util`

---

### Task 10 · Frontend `agent-store` 扩展 + `use-workspace-usage` hook

**目标**:zustand store 加 `usageSummary` 字段;新 hook reduce 出 workspace 聚合。

**改动文件**:
- `products/gitim/frontend/src/hooks/use-agent-store.ts` — Agent 类型加 `usageSummary?: UsageSummary` 字段;SSE handler 已经在 Task 8 patched
- `products/gitim/frontend/src/hooks/use-workspace-usage.ts`(新) — selector 出所有 agents(raw map),`useMemo` reduce 出 totals / today / by_day(同日 sum)/ by_provider(group by `agent.provider`);**严格 selector 稳定性**:不在 selector 里返回新数组 / `?? []` / `.filter()` / 新对象字面量

**测试**:
- `products/gitim/frontend/src/hooks/use-workspace-usage.test.ts` — 多 agent + 单 agent + 全 None usage_summary 的 reduce 边界;by_provider 分组正确;30 天同日 sum 正确;selector 稳定性(同一 input 返回同一 reference)

**验证命令**:`cd products/gitim/frontend && npm run test -- use-workspace-usage`

**Commit message** 模板:`feat(frontend): add use-workspace-usage hook for client-side aggregation`

---

### Task 11 · Frontend 三个新组件

**目标**:实现 detail / list / workspace header 三个组件。

**改动文件**:
- `products/gitim/frontend/src/components/management/agent-usage-card.tsx`(新)
- `products/gitim/frontend/src/components/management/agent-usage-tag.tsx`(新)
- `products/gitim/frontend/src/components/management/workspace-usage-header.tsx`(新)
- `products/gitim/frontend/src/components/management/agent-detail.tsx`:挂载 `<AgentUsageCard />`
- `products/gitim/frontend/src/components/management/agent-card.tsx`:行内挂 `<AgentUsageTag />`
- `products/gitim/frontend/src/components/management/agent-list.tsx`:顶部挂 `<WorkspaceUsageHeader />`

**视觉**:复用 `lib/sparkline.ts::sparklinePath`;复用 `usage-indicator.tsx` 的 stroke / size 风格;Radix HoverCard for tooltip(若已引则复用,否则跟 usage-indicator design 同款新增封装)

**降级行为**:
- `provider_reports_usage = false`:agent-usage-card 显示"该 provider 不上报 token · turns: N";agent-usage-tag 显示 `— · N turns`;workspace-usage-header 不计入 by_provider 分组的 token 数,但 turns 仍计
- `usage_summary = null`:三个组件都 hide 或显示骨架占位

**测试**(每个组件一份 `.test.tsx`):
- `agent-usage-card.test.tsx`:正常渲染 4 字段 tooltip;unsupported provider 渲染降级文本;`usage_summary = null` 不 render 任何东西
- `agent-usage-tag.test.tsx`:`today.turns = 0` 渲染 `—`;有数据正常渲染
- `workspace-usage-header.test.tsx`:多 agent reduce 渲染;by_provider 分组显示正确;空 list 不 crash

**验证命令**:`cd products/gitim/frontend && npm run test -- management`

**Commit message** 模板(分两个 commit):
- `feat(frontend): add agent and workspace usage components`
- `feat(frontend): mount usage components in management views`

---

## 全局验证

完成所有 Task 后:

1. `cargo test --workspace` — 全量回归(任务末尾的耗时操作,不要中途跑)
2. `cd products/gitim/frontend && npm run test` — 前端全量
3. `cd products/gitim/frontend && npm run lint && npm run build` — 类型检查 + lint + build
4. **Phase 6 review**:Claude code-reviewer subagent 审 diff;Codex review 审 diff;agree / disagree 列出
5. **Phase 7**:回报状态等用户验收

## 风险与降级

- **Hermes 语义无法测明**:Task 2 若无法用 mock 复现真实 hermes 行为,降级方案 = 声明 `usage_is_cumulative = false`(默认),记入 `last_session_usage` baseline 即使是 incremental 也无害(baseline 永远 = current,delta = 0 累计错误);出问题在 Phase 7 用户验收时再调
- **Provider trait 改动 break 既有 stub**:`crates/gitim-agent-provider/src/stubs/`(若存在)和 `mock/mod.rs` 必须 implement 新 trait method;Task 1 完成时 verify
- **agent_loop 函数签名扩展过深**:若 `update_session_usage` 已经 5+ 参数,继续加 5 个参数会 ugly;考虑加 `UpdateSessionUsageContext` 结构体打包(implementation phase 临时决策)
- **Frontend zustand selector 稳定性回归**:Task 10 必须显式跑 selector identity test(memory 提到的 pitfall),不通过则 implementation 阶段返工

## Out of scope(再次确认)

参见 design.md 的 Non-goals 节。本 plan 不包含:计费换算、跨 workspace 聚合、turn-level log、git 同步统计、estimator fallback。
