# Agent Token Usage Statistics — Design

**Date**: 2026-05-10
**Slug**: `2026-05-10-agent-token-usage`
**Status**: Approved by user, ready for plan review
**Scope**: `crates/gitim-runtime/`, `crates/gitim-agent-provider/codex/`, `crates/gitim-agent-provider/hermes/`, `products/gitim/frontend/`

## Goal

在 runtime 层为每个 agent 持续记录 token 使用量,提供:

- **每个 agent**:累计 token(input + output + cache_read + cache_creation)+ 累计 turns + 30 天 by-day breakdown
- **workspace 总量**:所有 agent 加总 + 按 provider 分组(前端聚合,无后端 endpoint)
- **WebUI 展示**:agents list 顶部 workspace header + list 行小标签 + detail 页详细 sparkline

## Non-goals

- **不做** turn 级别原始 log(只存日聚合;90 天 by_day 滚动)
- **不做** token-to-cost 计费换算(留 v2)
- **不做** 历史回填(daemon 不存 turn-by-turn,物理上不可能)
- **不做** 跨 workspace 聚合
- **不通过** git 同步统计数据(per-clone 本地数据,跟 me.json 同地位)
- **不为** gemini / openclaw 估算 token(这俩 provider 不上报就只记 turn 计数)
- **不暴露** workspace 总量的后端 endpoint(前端 reduce 即可)

## 架构决策汇总

| 决策 | 选择 | 否决项 |
|------|------|--------|
| 持久化层 | `<workspace>/.gitim-runtime/usage/<handler>.json`,每 agent 一文件,日聚合 | JSONL turn 级原始数据(粒度过细);workspace 单文件(并发竞态);`<agent-clone>/.gitim/`(职责边界混乱) |
| 时间口径 | UTC 截日,前端展示用 local 转换 | 本地时区分桶(多机不一致 + 夏令时毛刺) |
| agent 归属 | handler 当 key | agent_id(handler 已 immutable,等价但更直观) |
| 跨日 turn | 按 turn 完成时间归当天,不切片 | 按毫秒比例切分(over-engineered) |
| hard delete | 一起删 usage 文件 | 保留(re-add 同名 = 全新统计,跟 hermes profile 行为对齐) |
| soft delete | 保留不动 | 同 soft delete 保留所有数据语义 |
| 数字定义 | total = input + output + cache_read + cache_creation;tooltip 拆 4 字段 + turns | 仅 input+output(忽略 cache,Claude 场景失真);仅累计 turns(无法回答"用了多少 token") |
| 不上报 usage 的 provider | 只记 turns,token 全 0,schema 标记 `provider_reports_usage: false` | tokenizer 估算兜底(估算和真实混在一起,跨 agent 比较失真) |
| ProviderUsage 归一化 | **agent_loop 持有 `last_session_usage` baseline,做 normalize**;Provider trait 加 `fn usage_is_cumulative(&self) -> bool { false }` 让各 provider 声明语义;cumulative 的 provider(codex)走 `delta = saturating_sub(current, last_seen)` | provider 内部维护 last_seen(Provider::execute 是 `&self`,需要 `Arc<Mutex<HashMap>>`,不优雅;且 provider instance 共享时易污染) |
| Provider 是否上报 token | **Provider trait 加 `fn reports_usage(&self) -> bool { true }`**,gemini / openclaw override 为 false | agent_loop hardcode `match provider == "gemini" \| "openclaw"`(cross-crate coupling,新 provider 易遗漏) |
| session reset | usage 文件不动(物理解耦) | 自动派生 |
| HTTP 暴露 | `AgentInfo` 加 `usage_summary`,GET /agents 和 GET /agents/{id} 都带;SSE `usage` event 扩展简版 | 单独 endpoint(数据量小不值当拆) |
| WebUI 展示 | agents list 顶部 Workspace header + list 行小标签 + detail 页详细卡片 | 仅 detail / 仅 list / 顶栏总览(信号位错位) |
| Workspace 聚合 | 客户端 reduce,无后端 endpoint | 后端 `/workspaces/{slug}/usage` 端点(数据已在 agent list 里,多端点风险数字不一致) |
| Backfill | 不做,lazy init,first_seen = 第一次 turn 完成时间 | 物理上不可能 |

## 数据模型

### 文件:`<workspace>/.gitim-runtime/usage/<handler>.json`

权限:`chmod 0600`(跟 `<workspace>/.gitim-runtime/config.json` 一致)

```json
{
  "version": 1,
  "handler": "alice",
  "provider": "claude",
  "model": "claude-sonnet-4-6",
  "provider_reports_usage": true,
  "first_seen": "2026-05-10T08:23:11Z",
  "last_updated": "2026-05-11T15:02:33Z",
  "totals": {
    "input": 12345,
    "output": 6789,
    "cache_read": 50000,
    "cache_creation": 1234,
    "turns": 17
  },
  "by_day": {
    "2026-05-10": { "input": 1000, "output": 500, "cache_read": 5000, "cache_creation": 100, "turns": 3 },
    "2026-05-11": { "input": 11345, "output": 6289, "cache_read": 45000, "cache_creation": 1134, "turns": 14 }
  }
}
```

### 字段语义

| 字段 | 语义 |
|------|------|
| `version` | schema 版本,v1 hardcode `1`;未来变 schema 时由读取端做 best-effort 兼容 |
| `handler` | agent handler,必须 = 文件名 stem |
| `provider` / `model` | 从 `me.json` 拷贝过来的冗余字段,审计用,immutable(provider/model 修改是 v2+ 话题) |
| `provider_reports_usage` | 标记该 provider 是否上报 token 数;false 时 input/output/cache 全 0,只 `turns` 累加 |
| `first_seen` | 第一次产生 usage 数据的 UTC ISO8601 |
| `last_updated` | 最近一次 accumulate 的 UTC ISO8601 |
| `totals` | 全历史累计(不跟 by_day 90 天裁剪同步) |
| `by_day` | 按 UTC 日期分桶,90 天滚动窗口;key = `YYYY-MM-DD` |

### 保留期

- `totals` = 全历史累计,永不裁剪
- `by_day` = 90 天滚动窗口,save 时清理超期 entry
  - 上限确定:文件 < 10KB(90 个 day entry × ~80 字节 + header)
  - 30 天 sparkline 完全覆盖,90 天足以满足"近 3 个月"回看

## Runtime 改动(三层)

### 1. Provider 层 — `crates/gitim-agent-provider/`

**契约**:Provider 不被强制归一化 ProviderUsage,而是通过 trait method 声明自己的语义。agent_loop 拿到声明后做 normalize。

**Trait 加两个 method**(默认值最常见):

```rust
trait Provider {
    // ... 既有
    fn reports_usage(&self) -> bool { true }       // 是否上报 token
    fn usage_is_cumulative(&self) -> bool { false } // 上报的是 session 累计还是 turn 增量
}
```

**`Session::usage` 是 canonical 的**:Provider 实现把"本 turn 最终 usage"挂在 `ExecResult.usage` 上(execute 结束时确定的最终值)。**mid-stream usage 事件(hermes 的 usage_update / claude 的 iteration usage)是 display-only,不传到 agent_loop 做 accumulate**。

| Provider | `reports_usage` | `usage_is_cumulative` | 当前实现 | 改动 |
|---------|----------------|----------------------|---------|------|
| `claude/mod.rs` | true | false | iteration usage 是增量,result.usage 取末值 | **覆盖默认**(声明 cumulative=false 显式化) |
| `codex/mod.rs` | true | **true** | `token_count` event 报 session 累计 | **声明 cumulative=true**;Provider 实现不再做 delta 计算(交 agent_loop) |
| `opencode/mod.rs` | true | false | StepFinish 是 step 增量 | **覆盖默认** |
| `hermes/mod.rs` | true | TBD | usage_update 语义不明 | **审计 + 声明** —— 加测试验证 result.usage 是 turn 增量还是 cumulative,据此声明;mid-session events 不进 result.usage |
| `pi/mod.rs` | true | false | turn_end 是 turn 增量 | **覆盖默认** |
| `gemini/mod.rs` | **false** | (irrelevant) | usage = None | **声明 reports_usage=false** |
| `openclaw/mod.rs` | **false** | (irrelevant) | usage = None | **声明 reports_usage=false** |

**测试**:每个上报 usage 的 provider(claude / codex / opencode / hermes / pi)加一个 integration test:连续两个 turn 喂相同 prompt,断言 ProviderUsage 的语义匹配 `usage_is_cumulative()` 的声明(累计型: turn2 ≥ turn1;增量型: turn2 ≈ turn1)。

### 2. State 层

#### 2a. `AgentState` 扩展(`crates/gitim-runtime/src/state.rs`)

`AgentState` 当前持久化在 `<agent-clone>/.gitim/agent-state.json`。加一个字段保存 cumulative provider 的 normalize baseline:

```rust
pub struct AgentState {
    // ... 既有
    pub last_session_usage: Option<LastSessionUsage>,
}

pub struct LastSessionUsage {
    pub session_id: String,
    pub usage: ProviderUsage,
}
```

**生命周期**:
- 跟 `session_token` 同生命周期 —— session 切换(session_id 变化)时清零;session reset(用户手动 / context 满)时清零
- 这是 cumulative provider 的 normalize 状态,**不是统计累计**;统计累计在 `usage_log.rs`(独立文件,跟 session reset 解耦)

#### 2b. `usage_log.rs`(新文件)

```rust
pub struct AgentUsageLog {
    pub version: u32,
    pub handler: String,
    pub provider: String,
    pub model: String,
    pub provider_reports_usage: bool,
    pub first_seen: String,
    pub last_updated: String,
    pub totals: UsageBucket,
    pub by_day: BTreeMap<String, UsageBucket>,
}

pub struct UsageBucket {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub turns: u64,
}

impl AgentUsageLog {
    pub fn path(workspace_root: &Path, handler: &str) -> PathBuf;
    pub fn load_or_default(
        workspace_root: &Path,
        handler: &str,
        provider: &str,
        model: &str,
        provider_reports_usage: bool,
    ) -> Self;
    pub fn accumulate(&mut self, today: &str, delta: Option<&ProviderUsage>, now_iso: &str);
    pub fn save(&self, workspace_root: &Path) -> io::Result<()>;
    pub fn delete(workspace_root: &Path, handler: &str) -> io::Result<()>;
    pub fn prune_by_day(&mut self, today: &str);  // 保留 90 天
    pub fn last_30_days(&self, today: &str) -> Vec<DayEntry>;  // 补 0 填齐 30 天
    pub fn summary(&self, today: &str) -> UsageSummary;  // 转成 HTTP 暴露的 view shape
}
```

**accumulate 行为**:
- 总是 `turns += 1`
- `last_updated = max(self.last_updated, now_iso)` —— 防 NTP 时钟回拨导致时间戳倒流
- 首次创建时 `first_seen = now_iso`
- `provider_reports_usage = false` → token 字段不动,只记 turns
- 否则 → `today_bucket.input = today_bucket.input.saturating_add(delta.input.unwrap_or(0))`,output / cache_read / cache_creation 同理;`totals` 同步 saturating_add
- 注意:`delta` 已经被 agent_loop normalize 过(cumulative provider 已经做过 saturating_sub),这里保持 saturating_add 是双保险

**save 行为**:
- 先 `prune_by_day` 裁剪到 90 天
- 序列化为 JSON,原子写入(写到 `*.tmp` 然后 rename)
- chmod 0600(跟 `WorkspaceConfig` 一致)
- 父目录 `<workspace>/.gitim-runtime/usage/` 不存在时创建

**load_or_default 行为**:
- 文件不存在 → 返回新 `AgentUsageLog`,`first_seen` / `last_updated` 留空字符串(由 accumulate 第一次填)
- 文件存在但 JSON 解析失败 → log error + 返回默认值(等同重建,丢失数据但不 crash)
- 文件存在且 `provider` / `model` 跟 me.json 不一致 → 用文件里的(immutable;不一致是 bug 信号但不 crash)

### 3. agent_loop 层 — `crates/gitim-runtime/src/agent_loop.rs`

**Hook 点**:`update_session_usage()`(line 206-290)。在它末尾追加 normalize + 累计逻辑(以下为伪码;实现阶段会扩展函数签名拿到 `workspace_root` / `handler` / `provider_obj` / `model` / `runtime_state` / `slug` / `agent_state` / `session_id`):

```rust
// Step 1: Normalize provider usage to turn-delta
let delta: Option<ProviderUsage> = if !provider_obj.reports_usage() {
    None
} else if provider_obj.usage_is_cumulative() {
    let current = provider_reported.cloned().unwrap_or_default();
    let last = match &agent_state.last_session_usage {
        Some(prev) if prev.session_id == session_id => prev.usage.clone(),
        _ => ProviderUsage::default(),  // 新 session 或首次,baseline = 0
    };
    let d = ProviderUsage {
        input_tokens: Some(current.input_tokens.unwrap_or(0).saturating_sub(last.input_tokens.unwrap_or(0))),
        output_tokens: Some(current.output_tokens.unwrap_or(0).saturating_sub(last.output_tokens.unwrap_or(0))),
        cache_read_tokens: Some(current.cache_read_tokens.unwrap_or(0).saturating_sub(last.cache_read_tokens.unwrap_or(0))),
        cache_creation_tokens: Some(current.cache_creation_tokens.unwrap_or(0).saturating_sub(last.cache_creation_tokens.unwrap_or(0))),
        used_percent: current.used_percent,
    };
    // 警告日志 if 检测到 negative delta(cache invalidation 等异常)
    if current.cache_read_tokens.unwrap_or(0) < last.cache_read_tokens.unwrap_or(0) {
        tracing::warn!(handler = %handler, "cache_read decreased between turns (cache invalidation?)");
    }
    agent_state.last_session_usage = Some(LastSessionUsage { session_id: session_id.clone(), usage: current });
    // agent_state 持久化由既有 path 处理(state.save)
    Some(d)
} else {
    provider_reported.cloned()  // incremental,直接用
};

// Step 2: Accumulate to usage log
let mut log = AgentUsageLog::load_or_default(
    &workspace_root,
    &handler,
    &provider_name,
    &model,
    provider_obj.reports_usage(),
);
let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
let now_iso = chrono::Utc::now().to_rfc3339();
log.accumulate(&today, delta.as_ref(), &now_iso);
if let Err(e) = log.save(&workspace_root) {
    tracing::warn!(handler = %handler, error = %e, "failed to save usage log");
    runtime_state.usage_save_failures.fetch_add(1, Ordering::Relaxed);
}

// Step 3: Patch in-memory state(同 lock 内 read-back 也 OK,见 workspace recovery race 段)
let summary = log.summary(&today);
{
    let mut state = runtime_state.lock().unwrap();
    if let Some(agent) = state.workspaces.get_mut(slug).and_then(|w| w.agents.get_mut(&handler)) {
        agent.usage_summary = Some(summary.clone());
    }
}

// Step 4: 扩展 SSE event payload —— 用 sibling 字段,不 wrap 老 shape
// 老 shape: SessionUsageSnapshot 字段 inline 在 event.detail
// 新 shape: 既有字段保持 inline,加 sibling `usage_summary`
broadcast_usage_event(&handler, &session_usage_snapshot, &summary);
```

**关键 invariants**:
- `save` 失败不能让 turn 失败。Token 统计是次要数据,失败 log warn + 计数器自增,不阻塞主消息流
- `provider_obj.reports_usage()` 和 `usage_is_cumulative()` 由 trait 提供,不再 hardcode 在 agent_loop。新 provider 在 trait override 即声明语义
- agent_state.last_session_usage 跟 session 同生命周期,session_token reset 时一并清零

**SSE event 兼容性**:扩展后的 payload 必须保持既有 SessionUsageSnapshot 字段在顶层(`session_id` / `input_tokens` / `output_tokens` / `max_tokens` / `used_percent` / `source` / `updated_at`),只**新增** `usage_summary` 作为 sibling 字段。`use-agent-activity.ts` 现有 destructure 不破。

**Workspace recovery race 防护**:
- recover_workspace 必须先 load 所有 agent 的 `usage_summary` 进 in-memory state,**再** 调 `start_agent_loop` —— 确保第一次 turn 完成时,agent_loop 写入的 `usage_summary` 不会被 recovery 的延迟写覆盖
- 实现位置:`recover_agents_for_workspace`(http.rs:2999 附近)在插入 `ctx.agents` 前完成 usage 加载

## HTTP 改动 — `crates/gitim-runtime/src/http.rs` + `state.rs`

### `AgentInfo` 加字段

```rust
pub struct AgentInfo {
    // ... 既有字段
    pub session_usage: Option<SessionUsageSnapshot>,  // 既有
    pub usage_summary: Option<UsageSummary>,           // 新增
}

#[derive(Clone, Serialize)]
pub struct UsageSummary {
    pub provider_reports_usage: bool,
    pub first_seen: String,
    pub last_updated: String,
    pub totals: UsageBucket,
    pub today: UsageBucket,        // 便利字段:by_day[today_utc] 或全 0
    pub by_day: Vec<DayEntry>,     // 30 天窗口,补 0 填齐(最新 = 今天)
}

#[derive(Clone, Serialize)]
pub struct DayEntry {
    pub date: String,
    pub bucket: UsageBucket,
}
```

### Workspace 启动时 load

`workspace.rs::recover_workspace`(或对应启动路径)扫 `<workspace>/.gitim-runtime/usage/*.json`,把每个 agent 的 `AgentUsageLog` 加载进 `SharedRuntimeState.workspaces[slug].agents[handler].usage_summary`。

文件不存在的 agent → `usage_summary = None`(WebUI 渲染时 hide 区块或显示"暂无数据")。

### SSE event 扩展(backward compat)

现有 `"usage"` event 在 `agent_loop.rs:286` broadcast,payload 是 `SessionUsageSnapshot` 字段直接 inline 在 `event.detail`。Frontend `use-agent-activity.ts` 直接 destructure(`snap.session_id`, `snap.input_tokens`, ...)。

**不能** 改成 `{ session_usage: {...}, usage_summary: {...} }` 这种嵌套 wrapper —— 老 frontend 连 new runtime 时所有 destructure 拿到 undefined,HUD 静默坏掉。

**正确做法**:既有字段保持 inline,**只新增** `usage_summary` 作为 sibling:

```json
{
  "session_id": "abc-123",
  "input_tokens": 12345,
  "output_tokens": 6789,
  "cache_read_tokens": 50000,
  "cache_creation_tokens": 1234,
  "max_tokens": 200000,
  "used_percent": 0.62,
  "source": "ProviderReported",
  "updated_at": "2026-05-10T08:23:11Z",

  "usage_summary": {
    "totals": { ... },
    "today": { ... },
    "last_updated": "..."
  }
}
```

老 frontend 读 SessionUsageSnapshot 字段不变(忽略 usage_summary);新 frontend 同时读两块。

不带 `by_day`(同日 SSE 增量更新即可;跨日时 WebUI 重新拉 GET /agents 拿完整 by_day)。

### Endpoints

- `GET /agents` → `AgentsListResponse`,每个 `AgentInfo` 含 `usage_summary`(完整 30 天 by_day)
- `GET /agents/{id}` → 同上
- `PATCH /agents/{id}` → 不接受 `usage_summary` 字段(只读)

### Hard delete 钩子

`http.rs::agents_remove` 走 `hard_delete_agent_dir` 之后追加:

```rust
let _ = AgentUsageLog::delete(&workspace_root, &handler);  // best-effort
```

agents_remove 既有路径已经从 `ctx.agents` 移除该 handler;in-memory `usage_summary` 随之消失。re-add 同 handler 时(handler conflict 防护可能拒绝;若先 hard delete 再 add)`load_or_default` 看到文件不存在 → 返回新 log;in-memory `usage_summary = None` 直到第一个 turn 完成。

### 观测:save 失败计数器

`SharedRuntimeState` 加字段:

```rust
pub usage_save_failures: AtomicU64,
```

每次 `AgentUsageLog::save` 失败时 `fetch_add(1, Relaxed)`。在 `/runtime/health` 端点的 response 加 `usage_save_failures: u64` 字段,运维出问题时一眼能看到累计失败数。无 alert / threshold 逻辑(v1 不做),只是 surface 出来。

## WebUI 改动 — `products/gitim/frontend/src/`

### 新组件

| 文件 | 职责 |
|------|------|
| `components/management/agent-usage-card.tsx` | detail 页底部 "Token Usage" 区块。30 天 sparkline(复用 `lib/sparkline.ts::sparklinePath`)+ 大数字 + 4 字段 hover tooltip。`provider_reports_usage = false` 时显示"该 provider 不上报 token · turns: N" |
| `components/management/agent-usage-tag.tsx` | list 行小标签:`Today: 45K · 17 turns`;无数据 / unsupported 显示 `—` |
| `components/management/workspace-usage-header.tsx` | agents list 页顶部卡片。客户端 reduce 所有 agent 的 usage_summary,得到 workspace totals + today + by_day(同日 sum) + by_provider(group by `agent.provider`) |
| `lib/format-tokens.ts` | 数字格式化:`12345 → "12.3K"`,`1_234_567 → "1.2M"`,`< 1000 → "12"` |

### Hooks 改动

- `hooks/use-agent-store.ts` 既有 zustand store,把 `AgentInfo.usage_summary` 一起塞进 agent state
- SSE `"usage"` event handler(已存在,处理 session_usage)扩展为同时 patch usage_summary 简版字段(`totals` + `today` + `last_updated`),不动 `by_day`
- 新 hook `hooks/use-workspace-usage.ts`:从 agent-store selector 出所有 agent 的 usage_summary,memoized reduce 出 workspace 聚合
  - **zustand selector 稳定性**:不要在 selector 里返回新对象字面量 / `?? []` / `.filter(...)` —— 每次 render 都会构造新引用,触发无限重渲染。selector 只 raw 取出 `agents` map 引用,reduce 在 `useMemo` 里做。

### 挂载

- `agent-list.tsx` 顶部插入 `<WorkspaceUsageHeader />`
- `agent-card.tsx` 行内插入 `<AgentUsageTag />`
- `agent-detail.tsx` 底部插入 `<AgentUsageCard />`

### 视觉规格

参考 `usage-indicator.tsx` 已有的 sparkline 风格:`stroke="currentColor"`,色值 `text-primary`,`<path>` 直接渲染,无填充。

## 测试策略

### Rust(`cargo test -p gitim-runtime` + `-p gitim-agent-provider`)

| 范围 | 测试 |
|------|------|
| `usage_log.rs` 模块 | round-trip 序列化;lazy init(文件不存在);accumulate 增量行为;`provider_reports_usage = false` 只加 turns;90 天保留期裁剪;hard delete 调用 `delete()`;chmod 0600 验证(`fs::metadata().permissions().mode() & 0o777 == 0o600`);`last_updated` clock-jump 防御(`now_iso < self.last_updated` 时不回退) |
| Provider trait 声明 | 每个 provider 的 `reports_usage()` / `usage_is_cumulative()` 返回值正确(unit test) |
| codex provider 语义 | `usage_is_cumulative() == true`;`token_count` event 报 session 累计(turn2 ≥ turn1) |
| agent_loop normalize 逻辑 | cumulative provider:连续两 turn delta 正确;session_id 变化清零 baseline;negative cache_read 触发 saturating_sub + warn log;incremental provider 不走 normalize 路径 |
| 其他增量型 provider | smoke test 验证 trait 声明:claude / opencode / pi / hermes 各一个 |
| `agent_loop` 集成 | mock provider emit 不同 usage shape → 验证 normalize → usage 文件落盘;失败 save 不 panic;in-memory state.usage_summary 跟文件一致;`usage_save_failures` 计数器递增 |
| **e2e round-trip** | 启动真实 runtime + mock provider + 单个 agent → 驱动一个 turn → 断言 `GET /agents` 返回的 `usage_summary.totals.input == expected_delta`;断言 SSE event payload 含 `session_usage` 内联字段 + `usage_summary` sibling |
| Workspace recovery race | 在 `recover_agents_for_workspace` 模拟"agent 进 ctx.agents 前 usage 文件已存在",首次 turn 完成后 in-memory state 不被 recovery 写覆盖 |
| HTTP `GET /agents` | response 含 `usage_summary` 字段;30 天 by_day 补 0 填齐;`/runtime/health` 含 `usage_save_failures` |
| `hard_delete_agent_dir` | 一起删 usage 文件;hard delete 后 ctx.agents 清空该 handler;re-add 同 handler 后 `usage_summary == None` 直到第一个 turn |

### Frontend(`npm run test`)

| 范围 | 测试 |
|------|------|
| `format-tokens.ts` | 边界 (0, 999, 1000, 1_000_000, 1_500_000);保 1 位精度 |
| `agent-usage-card` | 4 字段 tooltip 渲染;`provider_reports_usage = false` 渲染降级文本 |
| `agent-usage-tag` | `today.turns = 0` 渲染 `—`;有数据正常渲染 |
| `workspace-usage-header` | 多 agent reduce 结果正确;by_provider 分组正确;30 天同日 sum |
| `use-workspace-usage` | selector 稳定性(对同一 input 返回同一引用,见 zustand pitfalls memory) |

不测 SSE 实时刷(信任浏览器 EventSource)和 zustand 内部 reactive 行为。

## 上线顺序

1. **Runtime 后端**(provider 增量化 + usage_log + agent_loop hook + HTTP 字段 + SSE 扩展 + hard delete 钩子)
   - 一次性合,无外部依赖
2. **Frontend**(组件 + 挂载 + 客户端聚合)
   - 后端发版后跟进。前端兼容 `usage_summary = null`(老 runtime payload 无此字段),hide 区块即可

## 回滚

- Runtime:回退 binary,`<workspace>/.gitim-runtime/usage/` 目录留着不清理;下次升级直接 resume(数据兼容)
- Frontend:revert commit,`usage_summary` 字段在 payload 里被忽略

## 已有 agent 升级路径

无需 migration。第一次 turn 完成 → `load_or_default` 创建空文件 → `accumulate` → save。`first_seen` 是该 agent 第一次产生数据的 UTC 时间。前端展示时可以诚实标注"统计自 YYYY-MM-DD 起"。

## Plan-eng-review findings & resolutions

(留作 design 演化 trace,2026-05-10 review 后修订)

| # | Severity | Finding | Resolution |
|---|----------|---------|------------|
| 1 | IMPORTANT | Provider::execute 是 `&self`,session-scoped `last_seen` 不能放 provider 内部局部变量,需要 Mutex 或反转设计 | **反转 B1**:agent_loop 持有 `last_session_usage` baseline(放 AgentState,跟 session_token 同生命周期),provider 通过 trait method 声明 `usage_is_cumulative()` |
| 2 | IMPORTANT | Mid-stream usage events(hermes usage_update / claude iteration usage)可能与 result.usage 语义不一致,导致 accumulate 混乱 | 明确 invariant:`Session::usage` 是 canonical 的 turn-final 值,mid-stream events display-only,不进 accumulate |
| 3 | IMPORTANT | cache_read / cache_creation 不单调,delta 可能负;u64 underflow | agent_loop normalize 用 saturating_sub;accumulate 用 saturating_add 双保险;negative delta 触发 warn log |
| 4 | IMPORTANT | SSE event payload 改 wrapper shape 会 break 老 frontend `use-agent-activity.ts` 的 destructure | 保持既有 SessionUsageSnapshot 字段在顶层 inline,只新增 `usage_summary` sibling |
| 5 | NIT | workspace recovery 与首次 turn 写存在竞态(recovery 加载晚于 turn 写) | recover_agents_for_workspace 在插入 ctx.agents 前完成 usage 加载;加测试 |
| 6 | NIT | NTP 时钟回拨导致 `last_updated` 倒流 | accumulate 用 `last_updated = max(prev, now_iso)` |
| 7 | NIT | hard delete 后 in-memory state 行为未明确测试 | 加测试:hard delete → ctx.agents 清空 → re-add 同 handler → usage_summary = None until first turn |
| 8 | NIT | save 失败仅 warn,无观测 | SharedRuntimeState 加 `usage_save_failures: AtomicU64`,`/runtime/health` 暴露 |
| 9 | NIT | 缺 e2e round-trip 测试 | crates/gitim-runtime/tests/ 加 e2e:provider → agent_loop → file → HTTP response → SSE event |
| 10 | NIT | `provider_reports_usage` hardcode 是 cross-crate coupling | Provider trait 加 `reports_usage()` 默认 true,gemini/openclaw override false |
| 11 | NIT | 是否拆 plan(provider 增量化 vs storage layer) | 保留单一 plan,但 implementation 顺序:provider trait 加 method + 各 provider override + 测试 → 再上 agent_loop normalize → 再上 storage layer。后续步骤强依赖前面 green |

## v2+ 后续

- token-to-cost 价格换算(各 provider 价格表)
- workspace 跨 workspace 聚合
- 历史归档 / export(JSONL dump)
- "estimated" fallback 给 gemini / openclaw(若有需求)
- 按 model 而非 provider 的细粒度分组视图
- token 统计的 git 同步(目前是 per-clone 本地数据)
