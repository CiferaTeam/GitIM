# Card Frontend — WebUI v2 接入后端 Card 能力

**Status**: Design (eng review passed 2026-04-17)
**Author**: lewis
**Date**: 2026-04-17
**Branch**: `feature/card-frontend`
**Predecessor**: `docs/plans/card-refactor/00-design.md`（后端已落地）

---

## 1. 动机

`docs/plans/card-refactor/` 交付了后端 card 能力（5 个 `/im/cards/...` HTTP 端点 + events + CLI）。设计 §12 明确把"前端看板视图 UI"列为范围外。本 plan 补齐前端，让用户能在 WebUI v2 看板化管理任务卡。

## 2. 核心决策（Phase 2-3 grill-me + eng-review 已对齐）

| # | 决策 | 理由 |
|---|------|------|
| D1 | 导航：top-level `/cards` tab + channel 内 card drawer | 看板是人类鸟瞰；channel 内是聚焦视图。双入口互补 |
| D2 | `/cards` 主形态：Kanban 三列（todo / doing / done） | 和后端 status 固定三态同构；status 流转最高频 |
| D3 | Channel 内 card 面板：header 按钮 + 右抽屉 | 不占既有三列；抽屉关了不留痕 |
| D4 | Card 数据推送：扩 `/im/poll` 识别 card 路径 | 复用既有 3s polling cadence；新 kind：`card_meta` / `card_thread` |
| D5 | Card 详情：独立路由 `/cards/:ch/:id` + 复用 MessageList/InputArea/ThreadPanel | discussion.thread 和 channel .thread 同格式，组件应该 props 化复用 |
| D6 | 创建 UX：统一 Modal + 创建后跳详情 | 两入口共用 `<CardCreateDialog>` |
| D7 | Filter bar：顶部 always-visible；URL query 同步 | Kanban 高频过滤场景；`useSearchParams` 作为 SOT |
| D8 | 分页：MVP 不做；前端 sort updated_at DESC + Done 列默认 20 | 活跃卡数有界（归档另一 ticket 托底） |
| D9 | Label input：chip + autocomplete + inline create | 映射后端"label 隐式，出现即存在" |
| D10 | 未读：MVP 不做 | `updated_at` 排序 + 时间戳提供新鲜度信号 |

## 3. 架构决策（eng review Issue 表）

### I1 · MessageList / InputArea / ThreadPanel 的复用策略 — **Props 化**

当前三个组件深度耦合 `useChatStore`（读 7 个 state、写 2 个 action、localStorage 草稿 key 含 channel）。为让 card detail 复用，**一次性 props 化**：

- 组件不再直接读 `useChatStore`；改为接受 props
- 调用方（`ChatLayout` / `CardDetail`）各自从对应 store 读，作为 props 传入
- `localStorage` 草稿 key 从 `gitim:draft:${channel}` 泛化为 `gitim:draft:${scopeKey}`，`scopeKey` 由调用方决定（channel 名 / `card:<ch>/<id>`）

### I2 · `/im/poll` 路径扩展

`crates/gitim-daemon/src/handlers.rs:947` 的 path 解析扩展：

- `channels/<ch>/cards/<id>/card.meta.yaml` → `kind="card_meta"`, `entries=[]`, `channel` 字段编码为 `card:<ch>/<id>`
- `channels/<ch>/cards/<id>/discussion.thread` → `kind="card_thread"`, `entries=<parse_thread added content>`, `channel` 编码同上
- **Membership filter 复用**：card 路径借用其所在 channel 的 membership 检查（`handlers.rs:972`），非成员不看

### I3 · `PollChange.kind` 类型收紧

前端 `types.ts` 把 `kind: string` 改成 union：
```
"channel" | "channel_meta" | "dm" | "card_meta" | "card_thread"
```
消费端 switch 漏分支变编译错误。

### I4 · Channel drawer 状态位置

ChatLayout 组件内 `useState<boolean>`，**切 channel 时自动关**。非全局 store，非持久。

### I5 · Create / Update 失败回滚

- Create 成功 → 乐观 push 到 `useCardStore.cards` → `navigate(/cards/:ch/:id)`
- Create 失败 → inline error，dialog 保留字段
- Update（status/labels/assignee）乐观更新 store → PATCH 失败 → 回滚 store + sonner toast 错误
- Detail 页 mount：独立 `readCard(ch, id)` + 拉 meta，**不依赖 store**有无该 card（直刷 URL / 乐观 race 都能 work）

### I6 · Filter URL roundtrip

`useSearchParams` 作为 SOT；mount 读 URL → state → `listCards(params)`；filter change 写回 URL；multi-value 用重复 key（`?channel=a&channel=b`）。

### I7 · Assignee picker 数据源

`users + agents` 合并去重（复用 `input-area.tsx:26-30` 的 `mentionCandidates` 同逻辑）。

## 4. 文件结构总览

### Create — Backend

- `crates/gitim-daemon/tests/poll_cards_test.rs` — `poll_surfaces_card_meta`, `poll_surfaces_card_thread`, `poll_filters_card_by_channel_membership`

### Modify — Backend

- `crates/gitim-daemon/src/handlers.rs:947-1005` — path 解析扩两个分支（card_meta / card_thread）

### Create — Frontend

- `webui-v2/src/hooks/use-card-store.ts` — Zustand store（cards, cardMessagesByPath, allLabels derived）
- `webui-v2/src/components/cards/card-kanban.tsx` — `/cards` 页 Kanban 容器
- `webui-v2/src/components/cards/card-kanban-column.tsx` — 单列（含 Done 的 collapse-20）
- `webui-v2/src/components/cards/card-kanban-cell.tsx` — 单张卡渲染（title, assignee, labels, updated_at）
- `webui-v2/src/components/cards/card-filter-bar.tsx` — 顶部 filter bar + URL sync
- `webui-v2/src/components/cards/card-detail.tsx` — `/cards/:ch/:id` 详情页
- `webui-v2/src/components/cards/card-meta-bar.tsx` — 详情页顶部 meta 编辑区
- `webui-v2/src/components/cards/card-create-dialog.tsx` — 创建 Modal
- `webui-v2/src/components/cards/channel-card-drawer.tsx` — channel 内 card 右抽屉
- `webui-v2/src/components/ui/label-chip-input.tsx` — 通用 chip input（filter / create 共用）

### Modify — Frontend

- `webui-v2/src/lib/types.ts` — 加 Card / CardStatus / 收紧 PollChange.kind
- `webui-v2/src/lib/client.ts` — 加 `createCard` / `listCards` / `readCard` / `sendCardMessage` / `updateCard`
- `webui-v2/src/app.tsx` — 加 /cards 和 /cards/:ch/:id 路由；poll loop 识别 card_meta / card_thread
- `webui-v2/src/components/layout/nav-tabs.tsx` — 加 Cards 第三栏
- `webui-v2/src/components/chat/chat-layout.tsx` — 传 props 给 MessageList/InputArea/ThreadPanel；集成 channel drawer 触发
- `webui-v2/src/components/chat/message-list.tsx` — Props 化（移除 useChatStore 依赖）
- `webui-v2/src/components/chat/input-area.tsx` — Props 化（移除 useChatStore 依赖）
- `webui-v2/src/components/chat/thread-panel.tsx` — Props 化
- `webui-v2/src/components/chat/header.tsx` — 加 `Cards · N` 按钮

## 5. Phase 分工

| Phase | 范围 | 依赖 |
|-------|------|------|
| P1 · Backend poll 扩展 | Rust：path 解析 + 集成测试 | — |
| P2 · Frontend types + client | TS types + client methods | — |
| P3 · Props 化重构 | MessageList / InputArea / ThreadPanel / ChatLayout | P2（类型稳定） |
| P4 · useCardStore | Zustand store + 派生 allLabels | P2 |
| P5 · `/cards` Kanban 页 | Kanban + filter bar + URL sync + Done 折叠 | P4 |
| P6 · Card detail 页 | 详情路由 + MetaBar + 复用 MessageList/InputArea | P3, P4 |
| P7 · Create dialog | Modal + LabelChipInput | P4 |
| P8 · Channel drawer | Drawer + header 按钮 + ChatLayout 集成 | P4 |
| P9 · Navigation | NavTabs + 路由注册 + poll loop 接入 | P5, P6, P8 |
| P10 · QA + Polish | 端到端手测 + tsc/eslint 清洁 + DESIGN.md 对齐 | ALL |

每个 Phase 完成后 `cargo test` / `tsc --noEmit` / `eslint` 三联检，绿了才进下一个。

## 6. 失败模式清单

| 场景 | 预期行为 | 测试责任 |
|------|----------|----------|
| 创建卡片离线 / 500 | Dialog 内 inline error，字段保留 | 手测 |
| PATCH status 失败 | 回滚 store + toast | 手测（主动 Kill daemon 模拟） |
| `navigate(/cards/:ch/:id)` 时 store 无该 card | 详情页独立拉 `readCard`，无 store 依赖 | 手测（直接粘 URL 刷新） |
| Channel 切换时 drawer 开着 | Drawer 自动关 | 手测 |
| Poll 识别 card_thread 丢包 | 刷新页面 / 打开详情 `readCard` 补齐 | 后端测试覆盖 + 手测 |
| 非成员 channel 的 card 被泄露 | Poll 过滤，前端拿不到 | **后端集成测试覆盖** |

## 7. NOT in scope（显式排除）

- Agent mention → auto-create card（设计 §12 已排除）
- Card 分页 / limit / cursor / since（归档由另一窗口处理，活跃卡有界）
- Card 未读 / per-user read state / badges（Q9 决定 MVP 不做）
- 前端测试框架（vitest 等）搭建 — 项目基线 0 前端测试，保持一致
- Card description / priority / deadline / dependencies（设计 §9 已排除）
- Label 全局管理界面
- Kanban 之外的视图切换（List / Grouped）
- Bulk 操作
- Drag-and-drop 改 status（点 status chip 下拉选够用）

## 8. 验收标准

1. `cargo test` 全绿（含新增 `poll_cards_test.rs` 3 个用例）
2. `npx tsc --noEmit` 绿
3. `npm run lint` 绿
4. `npm run build` 绿
5. 手测通过：
   - 创建 channel C → 在 C 里 `Cards·0` → 开 drawer → 点 "+ New card" → 填 title → 跳详情页 → 写第一条 discussion → 返 `/cards` → 看到卡在 todo 列
   - 在 C 的 drawer 里点该卡 → 跳详情 → 改 status 成 doing → 返 `/cards` → 卡移到 doing 列
   - 改 assignee + labels → filter bar 按 label 过滤 → 只剩该卡
   - 直接粘 `/cards/:ch/:id` URL 刷新 → 详情页正常渲染
   - 开两个浏览器 tab 同步看 Kanban → 一侧改 status → 另一侧 3s 内自动更新
   - Kill daemon → 改 status → 看到 toast 错误 + UI 回滚
   - 切 channel 时 drawer 自动关
6. DESIGN.md 对齐：字体 / 色值 / spacing 无违规

## 9. Rollout 风险

- **Props 化重构（P3）影响面**：MessageList/InputArea/ThreadPanel 是 /chat 页核心，重构后若 regression 会打断主线使用。**缓解**：P3 完成后手测 /chat 完整 happy path（发消息、reply、thread、切 channel、草稿恢复）；P10 再做端到端回归
- **Backend poll 扩展**：`handlers.rs` 的 path 解析顺序依赖不能搞错（card 路径要在 `.meta.yaml` / `.thread` 顶层 match 之前匹配，否则被吃）。**缓解**：P1 测试 `poll_surfaces_card_meta` 专门盯这个
