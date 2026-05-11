# Cron timeline density polish

## Goal

让 webui v2 的 cron 日历在 entry 密度变高、cron 数变多、跨多个 agent 时仍然可读 —— 不为极端 workload（`* * * * *` 一类）做特殊化，但让普通用户在中等密度（半小时 cron × 多 agent）时不需要被迫面对一面墙的 chip 才能找到自己关心的信息。

## Background

当前 `/workspaces/<slug>/crons/timeline` 返回扁平 entry 数组，runtime 端有 `MAX_TIMELINE_ENTRIES_PER_CRON = 10_000` 单 spec 上限做兜底。前端两层渲染：

- **日历主格**（[cron-calendar.tsx](../../products/gitim/frontend/src/components/crons/cron-calendar.tsx)）：每天最多 3 个 chip + `+N more` 徽章，DOM 节点 cap 在 ~168，无问题。
- **Day panel**（[cron-day-panel.tsx](../../products/gitim/frontend/src/components/crons/cron-day-panel.tsx)）：点开某一天后 `entries.map(...)` 全部渲染。半小时 cron 一天 48 条 — 实测无压力；分钟级 cron 一天 1440 条 — DOM 节点偏多，但更大的问题是 *用户视觉上找不到 anchor*。

同时，TimelineEntry 当前只带 `cron_name`，**不带 handler 字段**。一面 e2e-online-check × 48 的列表，用户看不出哪些是 alice 跑、哪些是 bob 跑。这是这次一起补的语义缺口。

## Scope（三件事）

### 1. Day panel hour-grouping（阈值触发）

- **阈值**：当 `entries.length > 12` 时启用按小时分组；≤12 保持现在的 flat 行为。
- **分组键**：`ts` 的 UTC `HH` 部分。每个 hour group 渲染为一个可折叠的 header 行 + 折叠状态下的折叠 body。
- **默认状态**：全部折叠。Header 显示 `HH:00Z · N 个任务`，按 kind 分布给一个微图（past/future/missed 的色点）。
- **折叠展开**：键盘 Enter / Space / 鼠标点击；`aria-expanded` 跟随状态；每个 hour group 独立维护展开态（panel 重开重置）。
- **空 hour 不渲染**：没有 entry 的 hour 直接不出现在列表里（不是 24 行全列）。

**为什么这样**：
- 阈值 12 是 "≤12 的密度本来就一屏看完，强制分组反而是负担" 的经验数字；同时是 `*/30` cron 半天的边界，符合人对 cron 节奏的认知。
- 全折叠默认 = 把 1440 条压到最多 24 条 DOM 节点，性能压力消失；用户感兴趣时点开。
- 不渲染空 hour = 避免一天只有 3 个 entry 时被迫看 21 行空槽。

**Non-goals**：
- 不做虚拟化（`react-virtual`）。阈值分组已经把最坏 case 压到 24 + N（用户主动展开的部分）。
- 不做"展开全部"按钮。每个 hour 单独展开是足够细的控制粒度。
- 不做记忆展开态跨 panel 重开。一次性 UI。

### 2. `+N more` 徽章增强（aria-label + native title tooltip）

- 当前 `+N more` 徽章只显示数字。
- 新行为：徽章 `title` 属性 = `"<总数> 个任务（<distinct cron 数> 个 cron）"`，hover 显示原生 tooltip。
- 同步扩展 day cell 的 `aria-label`（[cron-calendar.tsx:45 `dayCellAriaLabel`](../../products/gitim/frontend/src/components/crons/cron-calendar.tsx:45)）：在现有 kind 分布后追加 `, <distinct cron 数> 个 cron`。例：`May 18, 2026, 48 未执行, 3 个 cron`。
- distinct cron 数定义：`entries[].cron_name` 去重后的 size。

**为什么这样**：
- desktop 用户能 hover 看；screen reader 用户能读到；mobile 无 hover 但本来就靠点开 day panel，信息不丢。
- 零样式工作、零依赖，aria-label 是现成的扩展点。

### 3. Day panel 行内 handler 标注（`target`）

- **wire format 改动**：`TimelineEntry`（runtime Rust 端）新增 `target: String` 字段，从 `CronSummary.target` 直接 copy。同步加到前端 `CronTimelineEntry` type。
- **渲染位置**：仅 day panel。calendar cell chip 不动（避免再加字符）。
- **行内格式**：每条 entry 行的现有 `<时间> · <cron 名>` 改为 `<时间> · @<target> · <cron 名>`。hour group 模式下，展开后的子项同样按这格式。
- **颜色**：`@<target>` 文字用现有 muted-foreground 调色（不引入 per-handler hue，避免 DESIGN.md 颠覆）。

**为什么这样**：
- "做这个任务" 在中文语义下指执行者 = `target`。`created_by` 是审计信息，cron-spec-detail 详情页已经覆盖。
- 仅 day panel 是 *primary "看一天有啥" 的 surface*；calendar chip 已经在挤 width。
- 不再按 handler 分组 = hour grouping 已经压了 DOM，再加一层维度是认知负担。`@alice` inline 让"同一个 handler 的 entry 视觉成块"自然发生。

## Wire format diff

```diff
 struct TimelineEntry {
     ts: String,
     kind: &'static str,
     cron_name: String,
+    target: String,
     thread_url: Option<String>,
     reason: Option<String>,
 }
```

前端 `CronTimelineEntry` 加 `target: string` 字段；mock fixtures / tests 同步加。

`target` 是 required 字段（每个 cron spec 一定有 target），不走 Option。

## Files involved

**Rust（runtime）**
- [crates/gitim-runtime/src/http.rs](../../crates/gitim-runtime/src/http.rs)：`TimelineEntry` 结构 + 构建 past / future / missed entry 的三处插入点都补 `target: summary.target.clone()`。
- 对应 timeline endpoint coupling test（line ~5392）：补 `target` 字段验证。

**Frontend**
- [products/gitim/frontend/src/lib/types.ts](../../products/gitim/frontend/src/lib/types.ts)：`CronTimelineEntry` 加 `target`。
- [products/gitim/frontend/src/components/crons/cron-day-panel.tsx](../../products/gitim/frontend/src/components/crons/cron-day-panel.tsx)：
  - 加 hour-grouping 渲染分支（threshold = 12）。
  - 加 `@target` inline 渲染。
- [products/gitim/frontend/src/components/crons/cron-calendar.tsx](../../products/gitim/frontend/src/components/crons/cron-calendar.tsx)：
  - `+N more` 徽章加 `title`。
  - `dayCellAriaLabel` 扩展 distinct cron 数。
- [products/gitim/frontend/src/components/crons/calendar-utils.ts](../../products/gitim/frontend/src/components/crons/calendar-utils.ts)：
  - 加 `groupEntriesByHour(entries) → Map<hourKey, CronTimelineEntry[]>` 工具。
  - 加 `distinctCronCount(entries) → number` 工具（也供 aria-label 用）。
- 相关测试文件同步加测：`calendar-utils.test.ts`、`cron-day-panel.test.tsx`、`cron-calendar.test.tsx`、`client.cron.test.ts`（type 验证）。

## Testing strategy

- **calendar-utils**：纯函数，单测 `groupEntriesByHour` 边界（空数组、单 entry、跨 hour、UTC vs local）和 `distinctCronCount`（重复 cron_name 去重）。
- **cron-day-panel**：
  - threshold 触发：12 → flat，13 → grouped。
  - hour group 折叠 / 展开行为（aria-expanded, 键盘交互）。
  - `@target` 在 flat 和 grouped 两种模式都出现。
- **cron-calendar**：
  - `dayCellAriaLabel` 包含 distinct cron count。
  - `+N more` 徽章的 `title` attribute 在 entry > 3 的 cell 上出现。
- **runtime http**：现有 `crons_timeline` 测试加 `target` 字段断言；timeline coupling guard test 同步更新。

## Non-goals / future

- 不做 calendar cell chip 按 handler 上色 — 需要新色板 + 色弱兼容 + DESIGN.md 协调，是层 2 工作。
- 不做 day panel 按 handler 分组 — 双重 grouping 认知负担过大。
- 不做后端 per-day cap — 现有 10000 per-spec cap 配合前端 hour grouping 已经把最坏 case 兜住。
- 不做 day panel 虚拟化 — 阈值分组让虚拟化的收益曲线极扁。
- 不暴露 `created_by` — 审计信息走 cron-spec-detail 详情页。

## Open questions

无。所有 UX 细节已经在 brainstorm 阶段 lock。
