# Cron Timeline Density Polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Day panel 在 >12 个 entry 时按 UTC 小时折叠分组,扩展 `+N more` 徽章和 day cell aria 携带 distinct cron 数,并在 TimelineEntry wire format 上加 `target` 字段,在 day panel 行内以 `@<target>` 形式显示执行 agent。

**Architecture:** runtime 端 `crons_timeline` 给 `TimelineEntry` 加 `target` 字段（直接从 `CronSummary.target` clone,无新 IPC）;前端 `CronTimelineEntry` 类型同步加;`cron-day-panel.tsx` 在 render 阶段按 threshold 选 flat / hour-grouped 两种分支,grouped 模式下每个 hour group 是受控折叠组件;`cron-calendar.tsx` 给 overflow 徽章加 `title`,扩展 `dayCellAriaLabel`;新增 `calendar-utils` 工具函数 `groupEntriesByHour` 和 `distinctCronCount` 作为单元测试锚点。

**Tech Stack:** Rust (axum / serde,runtime side),React 19 + TypeScript + Tailwind + Radix UI (frontend),Vitest + Testing Library (frontend tests),`cargo test -p gitim-runtime` (runtime tests)。

**Design doc:** [`design.md`](./design.md)

**Convention:** 每个 task 只包含文件路径、变更描述、验收标准。不内联代码;实现细节在执行阶段由具体编辑者根据上下文写。

---

## Phase 0 · Baseline

### Task 0：跑一次全量 baseline,排除祖传红测干扰判断

**Files:** 不改动。

- [ ] **Step 1:** 在 worktree 根跑 `cargo test`（runtime + core + sync 等全量）。期望 PASS。如果有红测,先记录是不是跟 cron / http / frontend timeline 相关 —— 不相关的红测属于祖传背景,跳过即可。
- [ ] **Step 2:** `cd products/gitim/frontend && npm test`（vitest 全量）。期望 PASS 或仅有已知红测。
- [ ] **Step 3:** 不 commit。结果只用于建 baseline。

---

## Phase 1 · Wire format（runtime side）

### Task 1：`TimelineEntry` 加 `target` 字段 + 三处构造点同步

**Files:**
- Modify: [`crates/gitim-runtime/src/http.rs`](../../../crates/gitim-runtime/src/http.rs:1617) — `TimelineEntry` 结构定义
- Modify: 同文件,`crons_timeline` 内三处 `TimelineEntry { ... }` 构造点（past / future / missed 各一处)
- Modify: 同文件 `~5400` 附近的 `timeline coupling guard` 测试

**变更描述:**
- 给 `TimelineEntry` 加 required field `target: String`,放在 `cron_name` 之后,在 `Serialize` 排序里就保持声明顺序即可。
- 在三处构造 `TimelineEntry` 的地方加 `target: summary.target.clone()`。`summary` 在三个 branch 都已经在作用域内。
- timeline coupling guard test 需要更新对 `CronSummary` 字段到位的断言,把 `target` 也列入"timeline 必需字段"集合。

**验收:**
- [ ] **Step 1:** 找一个或新建一个 `cargo test` 单元测试断言：`/crons/timeline` 返回的 entry 中 `target` 字段存在且与对应 spec 的 target 一致(可在 `crons_timeline_*` 现有集成测试里加断言)。
- [ ] **Step 2:** 先跑测试期望 FAIL(缺 target 字段或断言不通过)。
- [ ] **Step 3:** 实现修改,再跑测试期望 PASS。
- [ ] **Step 4:** `cargo test -p gitim-runtime` 全量 PASS。
- [ ] **Step 5:** Commit:`feat(runtime): add target field to cron timeline entries`

---

## Phase 2 · 前端类型 + 工具函数

### Task 2：前端 type 同步 `target`

**Files:**
- Modify: [`products/gitim/frontend/src/lib/types.ts`](../../../products/gitim/frontend/src/lib/types.ts) — `CronTimelineEntry` 接口
- Modify: 任何手写 fixture / mock 中的 `CronTimelineEntry`(用 `grep -rn "kind:\s*\"past\"" products/gitim/frontend/src` 等关键词找)

**变更描述:**
- `CronTimelineEntry` 加 `target: string`(required)。
- 所有测试 fixture 中构造 entry 的地方补 `target` 字段。可用 `"@alice"` 之类不冲突的 handler。

**验收:**
- [ ] **Step 1:** `npm test` 或 `npm run typecheck`(看 package.json 实际命令),期望 TypeScript 编译失败,因为缺 `target` 字段的 fixture 没补全。
- [ ] **Step 2:** 把缺字段的 fixture 补齐,再跑测试期望 PASS。
- [ ] **Step 3:** Commit:`feat(frontend): extend CronTimelineEntry with target field`

### Task 3：`calendar-utils` 加 `groupEntriesByHour` 工具(TDD)

**Files:**
- Modify: [`products/gitim/frontend/src/components/crons/calendar-utils.ts`](../../../products/gitim/frontend/src/components/crons/calendar-utils.ts)
- Modify: [`products/gitim/frontend/src/components/crons/calendar-utils.test.ts`](../../../products/gitim/frontend/src/components/crons/calendar-utils.test.ts)

**变更描述:**
- 新增 pure function `groupEntriesByHour(entries)` → 返回按 UTC `HH` 分组的 ordered list(每个元素是 `{ hourKey: "00" | "01" | ... | "23", label: "00:00Z", entries: [...] }`)。
- 顺序:hour 升序;空 hour 不出现在返回中。
- 同 entry 内顺序保持入参顺序(假设入参已按 ts 升序,这是 backend 契约)。

**验收测试要覆盖的 case:**
- 空数组 → 空数组。
- 全在同一 hour → 单个 group。
- 跨 hour → 多个 group,按 hour 升序。
- 重复 hour 内多 entry → 同 group,顺序保留。
- 跨午夜不属于同 hour(UTC `00` 跟 UTC `23` 不会合并)。

**步骤:**
- [ ] **Step 1:** 先在 `calendar-utils.test.ts` 加上面 5 个 case 的 `describe / it` 断言。期望 FAIL(函数未导出)。
- [ ] **Step 2:** `npm test -- calendar-utils` 验证 FAIL。
- [ ] **Step 3:** 在 `calendar-utils.ts` 实现 `groupEntriesByHour`。
- [ ] **Step 4:** 再跑测试期望 PASS。
- [ ] **Step 5:** Commit:`feat(frontend): add groupEntriesByHour util`

### Task 4：`calendar-utils` 加 `distinctCronCount`(TDD)

**Files:**
- Modify: 同 Task 3 两个文件

**变更描述:**
- pure function `distinctCronCount(entries)` → 返回 `entries[].cron_name` 去重后的 size。
- 边界:空数组 → 0;单 entry → 1;同名重复 → 1。

**验收:**
- [ ] **Step 1:** test 加 3 个 case。FAIL。
- [ ] **Step 2:** 实现。PASS。
- [ ] **Step 3:** Commit:`feat(frontend): add distinctCronCount util`

---

## Phase 3 · Day panel hour grouping

### Task 5：阈值切换 flat / grouped 渲染分支

**Files:**
- Modify: [`products/gitim/frontend/src/components/crons/cron-day-panel.tsx`](../../../products/gitim/frontend/src/components/crons/cron-day-panel.tsx)
- Modify: [`products/gitim/frontend/src/components/crons/cron-day-panel.test.tsx`](../../../products/gitim/frontend/src/components/crons/cron-day-panel.test.tsx)

**变更描述:**
- 常量 `HOUR_GROUPING_THRESHOLD = 12`,放在文件顶部带注释解释来源(见 design doc)。
- 在 `view.kind === "list"` 的 render 分支里,根据 `entries.length` 走两条路径:
  - `≤ 12`:保持现有 `entries.map(...)` 扁平列表。
  - `> 12`:调用 `groupEntriesByHour(entries)`,渲染 hour-group 列表。
- hour-group 渲染:每个 group 是一个受控 `<details>` 或自定义 disclosure。**首选自定义 disclosure** 以匹配 panel 现有视觉(看 cron-spec-detail / cron-run-viewer 同源 panel 是否有现成 pattern,有就复用)。
- 折叠 state:`Map<hourKey, boolean>` 存在 panel 局部 state,跟现有 `viewState.key` 一样按 `panelKey` 隔离 —— panel 重开时全部重置为折叠。
- header 显示:`HH:00Z · N 个任务` + 一个 kind 微图(past/future/missed 三个小圆点,只显示 count > 0 的那种)。kind 配色复用 `kindStyle` / `KIND_STYLES`。
- 默认全部折叠。

**验收测试要覆盖:**
- 12 个 entry:断言渲染 flat list,DOM 里没有 hour group header。
- 13 个 entry:断言渲染 hour group(至少有一个 header element)。
- 同 hour 多 entry:断言 header 显示总数。
- 空 hour 不出现:13 个 entry 全在 2 个 hour,断言只有 2 个 header。
- 键盘:Tab 到 header → Enter / Space 展开 → 子项可见,`aria-expanded` 翻转。
- panel 重开(切 dayKey)→ 所有 group 回到折叠态。

**步骤:**
- [ ] **Step 1:** test 先加上述 case。FAIL。
- [ ] **Step 2:** 实现常量 + 渲染分支 + group 组件。
- [ ] **Step 3:** 测试 PASS。
- [ ] **Step 4:** Commit:`feat(frontend): hour-group day panel entries above threshold`

### Task 6:hour group 显示 kind 分布微图

**Files:** 同 Task 5

**变更描述:**
- 每个 hour group header 右侧 / 旁边显示 1-3 个小圆点(8px 圆,sm gap),对应该 hour 内 past/future/missed 各自的存在性。颜色复用 `KIND_STYLES[kind]` 的现有 token。
- 仅显示该 hour 内 count > 0 的 kind(空 kind 不画圆)。
- 圆点不带 hover 文字 —— `header` 整体的 `aria-label` 把 kind breakdown 读出来(类似 `dayCellAriaLabel` 但 hour 级)。

**验收:**
- 单 kind hour:只有一个圆点。
- 三 kind 都有:三个圆点。
- header `aria-label` 包含 kind breakdown 字符串(同 `dayCellAriaLabel` 风格)。

**步骤:**
- [ ] **Step 1:** 加 test。FAIL。
- [ ] **Step 2:** 实现。PASS。
- [ ] **Step 3:** Commit:`feat(frontend): hour group kind distribution glyph`

---

## Phase 4 · `+N more` + day cell aria

### Task 7:overflow 徽章加 `title`,扩展 `dayCellAriaLabel`

**Files:**
- Modify: [`products/gitim/frontend/src/components/crons/cron-calendar.tsx`](../../../products/gitim/frontend/src/components/crons/cron-calendar.tsx) — `DayCell` 函数,`dayCellAriaLabel` 函数
- Modify: [`products/gitim/frontend/src/components/crons/cron-calendar.test.tsx`](../../../products/gitim/frontend/src/components/crons/cron-calendar.test.tsx)

**变更描述:**
- `+N more` 徽章 element 加 `title` 属性:`<总数> 个任务（<distinct cron 数> 个 cron）`。distinct cron 数走 `distinctCronCount(entries)`。
- `dayCellAriaLabel` 在现有 kind breakdown 串后追加 `, <distinct cron 数> 个 cron`,只在 distinct count > 1 时追加(单 cron 时这个信息无意义)。
- entries 为空时,`dayCellAriaLabel` 保持现有 "无任务" 行为不变。

**验收测试:**
- entries 长度 3 → 没有 overflow 徽章,无 `title`。
- entries 长度 4,全同名 cron → overflow 徽章 `title` = `"4 个任务（1 个 cron）"`,但 day cell aria-label 不追加 cron 数(因为 distinct = 1)。
- entries 长度 5,2 个不同 cron → overflow 徽章 `title` 显示 `(2 个 cron)`,aria-label 追加 `2 个 cron`。

**步骤:**
- [ ] **Step 1:** test 加 case。FAIL。
- [ ] **Step 2:** 实现。PASS。
- [ ] **Step 3:** Commit:`feat(frontend): +N more title and aria distinct cron count`

---

## Phase 5 · `@target` inline label

### Task 8:day panel 行内显示 `@<target>`

**Files:**
- Modify: [`products/gitim/frontend/src/components/crons/cron-day-panel.tsx`](../../../products/gitim/frontend/src/components/crons/cron-day-panel.tsx)
- Modify: 对应 test 文件

**变更描述:**
- entry row 现有渲染结构(`<时间> · <cron 名>` 或类似)改为 `<时间> · @<target> · <cron 名>`。
- `@<target>` 用 `text-muted-foreground` 调色,不用任何 per-handler hue。
- flat list 和 hour-grouped 模式下都生效。
- `kindStyle` / 现有视觉 token 不动。

**验收:**
- flat list 模式下:每行包含 `@<target>` 文本。
- grouped 模式下展开的子项:也包含 `@<target>` 文本。
- 不同 target 的两个 entry:测试断言两个不同 handler 文本都出现。

**步骤:**
- [ ] **Step 1:** test 加 case。FAIL。
- [ ] **Step 2:** 实现。PASS。
- [ ] **Step 3:** Commit:`feat(frontend): show @target handler inline in day panel`

---

## Phase 6 · 收尾

### Task 9:跑 scoped 测试 + 看一眼实际渲染

**Files:** 无改动

**步骤:**
- [ ] **Step 1:** `cargo test -p gitim-runtime` 全 PASS。
- [ ] **Step 2:** `cd products/gitim/frontend && npm test` 全 PASS,无新增 warning。
- [ ] **Step 3:** `npm run dev` 起前端,手动:
  - 看一天 ≤12 entry → flat。
  - 看一天 >12 entry → 默认全折叠的 hour groups。
  - 展开一个 hour → 子项可见,`@<target>` 显示。
  - hover 任意 `+N more` 徽章 → tooltip 显示总数 + cron 数。
  - Tab 键导航 hour group header → Enter 展开,Escape 关 panel,焦点回 day cell。
- [ ] **Step 4:** 如有 visual nit,inline fix + 跟前面 task 合并 commit。

### Task 10:全量回归

**Files:** 无改动

**步骤:**
- [ ] **Step 1:** worktree 根跑 `cargo test` 全量。期望 PASS(或仅祖传红测,跟 baseline 对比)。
- [ ] **Step 2:** `cd products/gitim/frontend && npm test` 全量 PASS。
- [ ] **Step 3:** 如有 regression,定位 root cause(systematic-debugging),不要绕过测试。
- [ ] **Step 4:** 没问题 → 进入 finishing-a-development-branch flow,让用户验收。

---

## File summary（执行时备查）

**Runtime:**
- `crates/gitim-runtime/src/http.rs` — TimelineEntry struct + 三处构造点 + coupling guard test。

**Frontend types & utils:**
- `src/lib/types.ts` — CronTimelineEntry 加 target。
- `src/components/crons/calendar-utils.ts` — 新增 groupEntriesByHour, distinctCronCount。
- `src/components/crons/calendar-utils.test.ts` — 对应单测。

**Frontend components:**
- `src/components/crons/cron-day-panel.tsx` — 阈值分支 + hour group 组件 + `@target` inline。
- `src/components/crons/cron-day-panel.test.tsx` — 对应组件测试。
- `src/components/crons/cron-calendar.tsx` — overflow `title` + dayCellAriaLabel 扩展。
- `src/components/crons/cron-calendar.test.tsx` — 对应测试。

**Other test fixtures:**
- 任何手写 CronTimelineEntry 的测试 fixture 都要补 target。Task 2 完成 typecheck 时会暴露具体清单。

---

## Self-check after writing

- [x] Spec coverage:design doc 三件事 → Task 1 (target wire) / Task 5+6 (hour grouping) / Task 7 (+N more) / Task 8 (@target inline) 全覆盖。
- [x] Placeholder scan:无 TBD / TODO / "implement later"。
- [x] Type consistency:`TimelineEntry.target` / `CronTimelineEntry.target` 命名一致;`HOUR_GROUPING_THRESHOLD = 12` 在 design doc 和 plan 中数字一致;`distinctCronCount` / `groupEntriesByHour` 函数名前后一致。
- [x] Task 顺序:wire format(Task 1) → 前端 type(Task 2) → utils(Task 3/4) → UI(Task 5-8) → 验证(Task 9-10)。无 forward dependency。
