# Card Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` to implement this plan task-by-task.

**Goal:** WebUI v2 接入 card 后端能力 — top-level `/cards` Kanban + `/cards/:ch/:id` 详情 + channel 内 card drawer + `/im/poll` 扩展覆盖 card 变更。

**Architecture:** 10 个 phase 顺序推进。Backend 扩 poll → Frontend types/client → Props 化 chat 组件重构 → Card store → Kanban 页 → Detail 页 → Create dialog → Channel drawer → Navigation 接入 → QA。每 phase 完成 `cargo test` / `tsc` / `eslint` 三联检再进下一个。

**Tech Stack:** Rust（gitim-daemon poll 扩展 + 集成测试），React 19 + Vite + Radix UI + Tailwind + Zustand + react-router（前端）。

**重要约束**：
- Plan 里**只写分工、文件路径、行为规范、验收标准**。不贴代码。实现代码由执行 agent 现写。
- TDD 优先：改 Rust 后端 → 先写测试；前端 pure function 逻辑（如 URL↔filter 转换、label 派生）可先写 vitest（但不引入框架；项目基线无，保持一致，用手测替代）。
- 每 Task 完成后 commit；每 Phase 完成后跑验收脚本。
- 严禁复制 `docs/plans/card-refactor/01-plan.md` 的冗长体例。

---

## 文件结构速查

### 新建

**Backend**
- `crates/gitim-daemon/tests/poll_cards_test.rs`

**Frontend**
- `webui-v2/src/hooks/use-card-store.ts`
- `webui-v2/src/components/cards/card-kanban.tsx`
- `webui-v2/src/components/cards/card-kanban-column.tsx`
- `webui-v2/src/components/cards/card-kanban-cell.tsx`
- `webui-v2/src/components/cards/card-filter-bar.tsx`
- `webui-v2/src/components/cards/card-detail.tsx`
- `webui-v2/src/components/cards/card-meta-bar.tsx`
- `webui-v2/src/components/cards/card-create-dialog.tsx`
- `webui-v2/src/components/cards/channel-card-drawer.tsx`
- `webui-v2/src/components/ui/label-chip-input.tsx`

### 修改

**Backend**
- `crates/gitim-daemon/src/handlers.rs`（poll path 解析）

**Frontend**
- `webui-v2/src/lib/types.ts`
- `webui-v2/src/lib/client.ts`
- `webui-v2/src/app.tsx`
- `webui-v2/src/components/layout/nav-tabs.tsx`
- `webui-v2/src/components/chat/chat-layout.tsx`
- `webui-v2/src/components/chat/message-list.tsx`
- `webui-v2/src/components/chat/input-area.tsx`
- `webui-v2/src/components/chat/thread-panel.tsx`
- `webui-v2/src/components/chat/header.tsx`

---

## Phase 1 · Backend poll 扩展

目标：让 `/im/poll` 输出 card 路径变更事件。

### Task 1.1 · 写失败集成测试

**File**: Create `crates/gitim-daemon/tests/poll_cards_test.rs`

- [ ] 测试用例 `poll_surfaces_card_meta`：
  - 启动 daemon，创建 channel C，用 `create_card(C, "t1", assignee=alice)` 建卡
  - 调 `poll(since=None)` 应返回 changes 含 `channel="card:C/<card_id>"`, `kind="card_meta"`, `entries=[]`
  - 调 `update_card(status=doing)`，poll（since=上次 commit）应再次返回同一 card 的 `card_meta` 事件
- [ ] 测试用例 `poll_surfaces_card_thread`：
  - 创建 card 后，用 `send_card_message(C, card_id, "hello")`
  - poll 应返回 `kind="card_thread"`, `entries` 含刚写的 message（line_number、author、body 正确）
- [ ] 测试用例 `poll_filters_card_by_channel_membership`：
  - User A 创建 private channel P 并建 card；User B 不是 P 成员
  - B 的 poll 不应返回 P 的 card_meta / card_thread 事件（channel membership filter 应对 card 路径生效）
- [ ] 测试惯例参考既有 `tests/board_test.rs` 或 `tests/card_test.rs`（如存在）；用 `serial_test` 串行执行
- [ ] 运行 `cargo test -p gitim-daemon --test poll_cards_test`，期望 **FAIL**（handler 还没加 card 分支）
- [ ] Commit：`test(daemon): add poll card event coverage`

### Task 1.2 · 实现 poll path 解析扩展

**File**: Modify `crates/gitim-daemon/src/handlers.rs:947-1005`

- [ ] 在现有 `if let Some(name) = path_str.strip_prefix("channels/") { ... }` 分支里，**先**尝试匹配嵌套 card 路径，再 fall back 到原有 `.thread` / `.meta.yaml`：
  - `channels/<ch>/cards/<card_id>/card.meta.yaml` → `(channel=format!("card:{}/{}", ch, card_id), kind="card_meta")`, `entries=[]`
  - `channels/<ch>/cards/<card_id>/discussion.thread` → `(channel=format!("card:{}/{}", ch, card_id), kind="card_thread")`, `entries=parse_thread(added_content)`
- [ ] Membership filter 部分：card 路径按**其所在 channel**的成员关系过滤（从 `path_str` 提取外层 `<ch>`，查 `channel_membership.get(ch)`）
- [ ] 如果用户是当前 user 但非 channel 成员，跳过（和现有 channel 逻辑一致）
- [ ] 运行测试：`cargo test -p gitim-daemon --test poll_cards_test`，期望 **PASS**
- [ ] 运行全量：`cargo test -p gitim-daemon`，期望所有既有测试不受影响
- [ ] Commit：`feat(daemon): surface card path changes in /im/poll`

### Phase 1 验收

- [ ] `cargo test -p gitim-daemon` 全绿
- [ ] `cargo build -p gitim-daemon --release` 成功

---

## Phase 2 · Frontend types + client

目标：前端拿到类型安全的 card API。

### Task 2.1 · 扩 types.ts

**File**: Modify `webui-v2/src/lib/types.ts`

- [ ] 新增 `CardStatus = "todo" | "doing" | "done"` 字面量 union
- [ ] 新增 `Card` interface，字段与后端 `list_cards` 响应一一对应：`card_id`, `channel`, `title`, `status`, `labels`, `assignee?`, `created_by`, `created_at`, `updated_at`
- [ ] 收紧 `PollChange.kind` 为 union：`"channel" | "channel_meta" | "dm" | "card_meta" | "card_thread"`
- [ ] Commit：`feat(webui): add Card types; tighten PollChange.kind`

### Task 2.2 · 加 client 方法

**File**: Modify `webui-v2/src/lib/client.ts`

- [ ] `createCard(channel, title, opts?: {labels?, assignee?, status?}) → ApiResponse<{channel, card_id, title}>`
  - `POST /im/cards`，body = `{channel, title, labels?, assignee?, status?}`
- [ ] `listCards(filter?: {channel?, labels?, status?, assignee?}) → ApiResponse<{cards: Card[]}>`
  - `GET /im/cards?...`，query 参数编码（labels 重复 key）
- [ ] `readCard(channel, cardId, opts?: {limit?, since?}) → ApiResponse<{meta: Card, entries: Message[]}>`
  - `GET /im/cards/:channel/:card_id?limit=&since=`
- [ ] `sendCardMessage(channel, cardId, body, replyTo?) → ApiResponse<{line_number}>`
  - `POST /im/cards/:channel/:card_id/messages`，body = `{body, reply_to?}`
- [ ] `updateCard(channel, cardId, patch: {status?, labels?, assignee?}) → ApiResponse<{...}>`
  - `PATCH /im/cards/:channel/:card_id`，body = patch
- [ ] 所有方法和既有 `send` / `read` 同风格（useConnectionStore baseUrl、await res.json()）
- [ ] 运行 `npx tsc --noEmit`，期望绿
- [ ] Commit：`feat(webui): add card client methods`

### Phase 2 验收

- [ ] `npx tsc --noEmit` 绿
- [ ] `npm run lint` 绿

---

## Phase 3 · Props 化 chat 组件

目标：`MessageList` / `InputArea` / `ThreadPanel` 从 store-driven 改成 props-driven，为 card detail 复用做准备。**关键阶段，影响 /chat 主线**。

### Task 3.1 · Props 化 MessageList

**File**: Modify `webui-v2/src/components/chat/message-list.tsx`

- [ ] 当前组件直接读 `useChatStore` 的：`messages`, `currentChannel`, `replyTo`, `highlightLine`, `pendingScrollLine` + 2 setters
- [ ] 重构为接受 props：`messages: Message[]`, `scopeKey: string | null`（替代 currentChannel 作为"是否为空"判断 + 空状态文案用的标识）, `replyTo: Message | null`, `highlightLine: number | null`, `pendingScrollLine: number | null`, `onHighlightLine(line|null)`, `onClearPendingScroll()`
- [ ] 保留原有 on-\* 回调 props
- [ ] 空状态文案：`scopeKey==null` → "Select a channel to start chatting" **或** 调用方可传 `emptyHint` prop 覆盖（card detail 场景下是 "Write the first note..."）
- [ ] `data-message-scroll` / `data-line` DOM 属性保留
- [ ] Commit：`refactor(webui): props-ify MessageList`

### Task 3.2 · Props 化 InputArea

**File**: Modify `webui-v2/src/components/chat/input-area.tsx`

- [ ] 移除对 `useChatStore` 的直接读（`currentChannel`, `replyTo`, `setReplyTo`, `users`, `isGuest`）
- [ ] 改为 props：`scopeKey: string`（用作 draft localStorage key + disable 判断）, `replyTo: Message | null`, `onReplyToChange(Message|null)`, `mentionCandidates: string[]`, `disabled?: boolean`
- [ ] `onSend` prop 维持（已有）
- [ ] Draft localStorage key：`gitim:draft:${scopeKey}`（channel 场景传 channel 名，card 场景传 `card:${channel}/${card_id}`，向后兼容现有 draft key 格式）
- [ ] `mentionCandidates` 由调用方合并 users+agents 后传入
- [ ] Commit：`refactor(webui): props-ify InputArea`

### Task 3.3 · Props 化 ThreadPanel

**File**: Modify `webui-v2/src/components/chat/thread-panel.tsx`

- [ ] 读一遍确认 store 耦合点
- [ ] 对应 props：`root: Message | null`, `messages: Message[]`, `onClose()`, `onReply(Message)`, `onSend(body, pointTo) → Promise<ApiResponse>`
- [ ] 调用方负责 fetch thread messages 后传入
- [ ] Commit：`refactor(webui): props-ify ThreadPanel`

### Task 3.4 · ChatLayout 适配（从 store 读，往下传）

**File**: Modify `webui-v2/src/components/chat/chat-layout.tsx`

- [ ] 新增 store-to-props 桥：ChatLayout 读所有 chat-specific store state，作为 props 向下传
- [ ] `scopeKey` 对 channel 场景 = `currentChannel`（原样）
- [ ] mentionCandidates = users + agents dedup（既有逻辑挪过来）
- [ ] onSend = 现有 send handler
- [ ] 确保既有行为 100% 等价
- [ ] Commit：`refactor(webui): wire ChatLayout to props-ified children`

### Task 3.5 · 手动回归 /chat 主线

- [ ] `npm run dev`，浏览器打开 /chat
- [ ] 走 happy path：发消息 → reply → 点 thread → 切 channel → 草稿恢复 → mention popup → 发 mention → pending → synced
- [ ] 如发现 regression，回到对应 Task 修复后重新回归
- [ ] 所有路径通过再进 Phase 4

### Phase 3 验收

- [ ] `npx tsc --noEmit` 绿
- [ ] `npm run lint` 绿
- [ ] `npm run build` 绿
- [ ] /chat 所有既有功能无 regression

---

## Phase 4 · useCardStore

目标：Zustand store 管 card 列表 + 详情消息。

### Task 4.1 · 新建 use-card-store.ts

**File**: Create `webui-v2/src/hooks/use-card-store.ts`

- [ ] State：
  - `cards: Card[]` — 全量列表（filter 由组件按需做；列表短不分 store）
  - `cardMessagesByPath: Record<string, Message[]>` — key = `${channel}/${card_id}`
  - `loading: boolean`
  - `error: string | null`
- [ ] Derived selector（在 store 外部写，不是 action）：`selectAllLabels(state)`, `selectFiltered(state, filter)`, `selectCardById(state, ch, id)`
- [ ] Actions：
  - `setCards(Card[])` — 整体替换
  - `upsertCard(Card)` — 按 `<channel>/<card_id>` 去重合并（create / update 成功时用）
  - `removeCard(ch, id)` — 预留（当前不用）
  - `setCardMessages(pathKey, Message[])` — 整体替换
  - `addCardMessages(pathKey, Message[])` — 按 line_number 去重追加（poll 推送时用）
  - `addPendingCardMessage(pathKey, Message)` — 乐观发送
  - `markPendingCardSent(pathKey, pendingId, lineNumber)`, `markPendingCardFailed(pathKey, pendingId)`, `removePendingCardMessage(pathKey, pendingId)`
- [ ] 风格完全对齐 `use-chat-store.ts` 的 pending / dedup 模式
- [ ] Commit：`feat(webui): add use-card-store`

### Task 4.2 · 把 poll loop 接入 card 事件

**File**: Modify `webui-v2/src/app.tsx:82-126`（runPoll 函数）

- [ ] 遍历 `changes`，识别 `change.kind`：
  - `"channel"` / `"channel_meta"` / `"dm"` — 原逻辑
  - `"card_meta"` — `change.channel` 形如 `card:<ch>/<id>`；解析出 `ch, id`；当前 `/cards` 页 / card detail 页挂载时触发 `listCards` refetch；同时如果 detail 当前就是这张卡，调 `readCard` 取 meta；不记未读
  - `"card_thread"` — 解析 `change.channel`；`entries` 追加到 `useCardStore.cardMessagesByPath[<ch>/<id>]`；不记未读
- [ ] 因 app.tsx 已复杂，新增一个辅助函数 `handleCardChange(change)` 在 app.tsx 顶层，runPoll 调用
- [ ] Commit：`feat(webui): wire poll loop to card events`

### Phase 4 验收

- [ ] `npx tsc --noEmit` 绿
- [ ] `npm run lint` 绿

---

## Phase 5 · `/cards` Kanban 页

### Task 5.1 · LabelChipInput（通用 chip input）

**File**: Create `webui-v2/src/components/ui/label-chip-input.tsx`

- [ ] Props：`value: string[]`, `onChange(string[])`, `suggestions: string[]`, `allowCreate?: boolean`（默认 true；filter 场景 false）, `placeholder?: string`, `maxChips?: number`（默认 10，对齐后端）
- [ ] UI：已选 label 显示为 chip（x 可删 / Backspace 删末尾）；输入框 focus 时下拉显示建议（Radix Popover + Command 列表，filter 输入实时）；Enter 命中下拉项 = 选中；Enter 无命中 + allowCreate → 新建 chip
- [ ] 字符集校验：`a-z 0-9 - _`，长度 1-32（对齐 backend `validate_label`）；违规时红边 + inline 错误（"allowed: a-z 0-9 - _, length 1-32"）
- [ ] Commit：`feat(webui): add LabelChipInput component`

### Task 5.2 · CardKanbanCell（单卡）

**File**: Create `webui-v2/src/components/cards/card-kanban-cell.tsx`

- [ ] Props：`card: Card`, `onClick(card)`, `onStatusChange(card, newStatus)`
- [ ] 渲染：title（truncate）, assignee（avatar + handle，可选）, labels（chip 列表，最多显示 3 个 + `+N`）, updated_at（format "2m ago" / "Apr 17"）
- [ ] 右上角 status chip 点击 → 下拉切 status（Radix DropdownMenu，3 个选项）
- [ ] hover 样式（bg-muted/50），cursor-pointer
- [ ] 视觉遵循 DESIGN.md；先读一遍 DESIGN.md 再动手
- [ ] Commit：`feat(webui): add CardKanbanCell`

### Task 5.3 · CardKanbanColumn（单列）

**File**: Create `webui-v2/src/components/cards/card-kanban-column.tsx`

- [ ] Props：`status: CardStatus`, `cards: Card[]`（已排序）, `onCardClick(card)`, `onStatusChange(card, newStatus)`
- [ ] 列头：status label + count `(N)`
- [ ] 列体：cards 垂直堆叠（CardKanbanCell）；空态"No cards"灰字
- [ ] **Done 列专属**：cards > 20 时默认渲染前 20；底部 `Show all (N)` 按钮展开（局部 useState）
- [ ] Commit：`feat(webui): add CardKanbanColumn`

### Task 5.4 · CardFilterBar

**File**: Create `webui-v2/src/components/cards/card-filter-bar.tsx`

- [ ] Props：`value: {channels: string[], labels: string[], assignee: string|null, mineOnly: boolean}`, `onChange(newValue)`
- [ ] 5 个控件：
  - Channel multi-select（combobox 从 `useChatStore.channels` 取 kind=channel 的）
  - Labels multi-select（用 LabelChipInput allowCreate=false；suggestions 从 `useCardStore.allLabels` 派生）
  - Assignee single-select（options = users+agents+"Anyone"+"Unassigned"）
  - `My cards` toggle（Switch；开启时 assignee disabled 显示"(you)"）
  - `Clear all` 按钮
- [ ] URL sync 不在这里做，由父组件（CardKanban）处理
- [ ] Commit：`feat(webui): add CardFilterBar`

### Task 5.5 · CardKanban 页

**File**: Create `webui-v2/src/components/cards/card-kanban.tsx`

- [ ] 顶层 /cards route 对应的 page 组件
- [ ] 用 `useSearchParams` 读 URL → filter state；filter change → `setSearchParams` 写回
- [ ] Mount + filter change → `listCards(filter)` → `setCards`
- [ ] Render：`<CardFilterBar>` + 三列 `<CardKanbanColumn>`
- [ ] Cards 按 `updated_at DESC` 排序后传给列
- [ ] 顶栏右侧 `+ New card` 按钮 → 唤起 `<CardCreateDialog>`（Phase 7 提供；此时可占位用 console.log）
- [ ] 空态（无卡 + 无 filter）：中心大字 "No cards yet. Create one to get started." + CTA
- [ ] 空态（有 filter，无结果）："No cards match these filters." + `Clear all`
- [ ] Commit：`feat(webui): add CardKanban page`

### Phase 5 验收

- [ ] `npx tsc --noEmit` 绿
- [ ] `npm run lint` 绿
- [ ] 手测：手工在 git 仓库下 `gitim card create` 建几张卡 → 访问 `/cards`（需 Phase 9 注册路由后）→ 能看到 Kanban；filter 工作

---

## Phase 6 · Card detail 页

### Task 6.1 · CardMetaBar

**File**: Create `webui-v2/src/components/cards/card-meta-bar.tsx`

- [ ] Props：`card: Card`, `onUpdate(patch)`（乐观更新父组件状态 + 调 updateCard + 失败回滚）
- [ ] 渲染：title（只读，design §9 排除改标题）, status chip（下拉改）, assignee（combobox，可清空）, labels（LabelChipInput 可编辑）
- [ ] 所有可编辑控件 onBlur / onChange 触发 `onUpdate`
- [ ] Commit：`feat(webui): add CardMetaBar`

### Task 6.2 · CardDetail 页

**File**: Create `webui-v2/src/components/cards/card-detail.tsx`

- [ ] 对应 route `/cards/:channel/:card_id`
- [ ] 用 `useParams` 取 channel / card_id
- [ ] Mount：调 `readCard(channel, card_id)` → 拿到 meta + entries → 写入 `useCardStore`（upsertCard + setCardMessages）
- [ ] 顶部 breadcrumb：`← Back` 按钮（history.back，fallback `/cards`）+ 路径 `#channel / card-id`
- [ ] 渲染：`<CardMetaBar card={...} onUpdate={async patch => { 乐观更新 store → updateCard → 失败回滚 + sonner toast }}>`
- [ ] 下方：复用 Phase 3 props 化后的 `<MessageList>` 和 `<InputArea>`：
  - `scopeKey = "card:" + channel + "/" + card_id`
  - `messages` = `useCardStore.cardMessagesByPath[<ch>/<id>]`
  - `onSend(body, pointTo)` → addPendingCardMessage → sendCardMessage → markPendingCardSent / Failed
  - mentionCandidates = users+agents
- [ ] ThreadPanel 先不接（card discussion 里的 reply 链 phase 后补；若时间充裕接上）
- [ ] 直刷 URL + store 没该卡：`readCard` 404 → "Card not found" empty state
- [ ] Commit：`feat(webui): add CardDetail page`

### Phase 6 验收

- [ ] `npx tsc --noEmit` 绿
- [ ] `npm run lint` 绿
- [ ] 手测：直粘 URL `/cards/:ch/:id` → 正常渲染；改 status → 返 /cards 看到已同步

---

## Phase 7 · Create dialog

### Task 7.1 · CardCreateDialog

**File**: Create `webui-v2/src/components/cards/card-create-dialog.tsx`

- [ ] Props：`open: boolean`, `onOpenChange(bool)`, `presetChannel?: string`, `onCreated(card)` 回调
- [ ] Radix Dialog
- [ ] 字段：
  - Title（input，必填，maxLength 200）
  - Channel（select；有 presetChannel 则 disabled 显示 preset；否则从 `useChatStore.channels` kind=channel 选）
  - Assignee（select，users+agents+空）
  - Labels（LabelChipInput allowCreate=true）
  - Status（select，默认 todo）
- [ ] 提交：`createCard(...)` → 成功：upsertCard（乐观）+ onCreated + navigate(/cards/:ch/:id) + 关 dialog；失败：inline error，dialog 保留
- [ ] ESC / 外部点击关闭；sending 时 disabled
- [ ] Commit：`feat(webui): add CardCreateDialog`

### Phase 7 验收

- [ ] `npx tsc --noEmit` 绿
- [ ] 手测：/cards 页点 `+ New card` → 填 title + 选 channel → 提交 → 跳详情页，卡已存在

---

## Phase 8 · Channel drawer

### Task 8.1 · ChannelCardDrawer

**File**: Create `webui-v2/src/components/cards/channel-card-drawer.tsx`

- [ ] Props：`channel: string`, `open: boolean`, `onOpenChange(bool)`
- [ ] Radix Dialog + `side="right"` 变体（或用 drawer 库；如没用 Radix 就直接写 overlay + fixed）
- [ ] Mount / open 时 `listCards(channel=channel)` → 仅本 channel 的 cards
- [ ] 渲染：顶部 header `Cards in #channel` + `+ New card` 按钮（presetChannel=当前 channel）→ CardCreateDialog
- [ ] List 紧凑行：title + status 小徽标 + assignee avatar + 最后更新时间；按 `updated_at DESC` 排
- [ ] 点一行 → `navigate(/cards/${channel}/${card_id})` 关 drawer
- [ ] 空态 "No cards in this channel yet."
- [ ] Commit：`feat(webui): add ChannelCardDrawer`

### Task 8.2 · 集成到 ChatLayout

**File**: Modify `webui-v2/src/components/chat/header.tsx` + `webui-v2/src/components/chat/chat-layout.tsx`

- [ ] header.tsx 加 `Cards · N` 按钮（N = 本 channel card 数，由 ChatLayout 传入）
- [ ] ChatLayout 加 local `const [drawerOpen, setDrawerOpen] = useState(false)`
- [ ] 切 channel 时 `useEffect(..., [currentChannel])` 自动关 drawer
- [ ] N 的获取：ChatLayout mount / currentChannel 变化时 `listCards({channel: currentChannel})` → 在 useCardStore 里筛（或 local state 缓存 count）
- [ ] header 按钮 onClick → `setDrawerOpen(true)`
- [ ] Render `<ChannelCardDrawer channel={currentChannel!} open={drawerOpen} onOpenChange={setDrawerOpen} />`
- [ ] Commit：`feat(webui): integrate ChannelCardDrawer into ChatLayout`

### Phase 8 验收

- [ ] 手测：进 /chat → 选 channel → header 看到 `Cards · N` → 点开 drawer → 看到本 channel 卡列表 → 点卡跳详情 → 切 channel drawer 自动关

---

## Phase 9 · Navigation 接入

### Task 9.1 · 路由注册

**File**: Modify `webui-v2/src/app.tsx`

- [ ] `<Routes>` 里在 `/chat` 之后加：
  - `<Route path="/cards" element={<CardKanban />} />`
  - `<Route path="/cards/:channel/:card_id" element={<CardDetail />} />`
- [ ] Commit：`feat(webui): register /cards routes`

### Task 9.2 · NavTabs 加 Cards

**File**: Modify `webui-v2/src/components/layout/nav-tabs.tsx`

- [ ] 读当前 2 个 tab 的 pattern
- [ ] 插入 Cards tab（第三个，跟在 Management / Chat 之后或之前按项目风格决定；读 DESIGN.md 确认顺序）
- [ ] 图标用 lucide-react 里的 `LayoutGrid` 或 `Kanban`
- [ ] Commit：`feat(webui): add Cards NavTab`

### Phase 9 验收

- [ ] `npm run build` 绿
- [ ] 浏览器点 NavTabs 能切到 /cards

---

## Phase 10 · QA + Polish

### Task 10.1 · 端到端手测（黄金路径）

- [ ] 准备：启动 daemon + webui-v2 dev，登录；打开两个浏览器 tab 模拟多用户同步（用同一身份也行）
- [ ] 场景 1：创建 → 编辑 → 移动
  - [ ] /chat 进一个 channel C
  - [ ] 开 drawer，`+ New card`，填 title "Plan card UI"，提交
  - [ ] 跳详情 → 写第一条 discussion "Starting work"
  - [ ] 返 /cards → 看到卡在 todo 列
  - [ ] 点卡进详情 → 改 status → doing
  - [ ] 返 /cards → 卡移到 doing 列
  - [ ] 再改 → done → 移到 done 列
- [ ] 场景 2：Filter
  - [ ] 多建几张不同 channel / label / assignee 的卡
  - [ ] Filter by channel → 只剩该 channel 卡
  - [ ] Filter by label → 只剩带该 label
  - [ ] `My cards` → 只剩 assignee=me 的
  - [ ] URL 可分享：复制 URL 到新 tab → filter 保留
- [ ] 场景 3：Drawer
  - [ ] channel A drawer 打开 → 切 channel B → drawer 自动关
  - [ ] channel A 里的卡在 drawer 显示正确
- [ ] 场景 4：同步
  - [ ] Tab 1 在 /cards；Tab 2 在 /chat#C 改 card status
  - [ ] Tab 1 在 3-5s 内自动更新 Kanban（poll cadence）
- [ ] 场景 5：失败路径
  - [ ] Kill daemon
  - [ ] 改 status → 预期 toast 错误 + UI 回滚
  - [ ] 重启 daemon，确认恢复
- [ ] 场景 6：直链
  - [ ] 复制 `/cards/:ch/:id` URL 粘到新 tab → 正常渲染（无 store 依赖）
  - [ ] `/cards/nonexistent/fake` → "Card not found"
- [ ] 场景 7：/chat 回归
  - [ ] 发消息 / reply / thread / 切 channel / 草稿 — 全部如常

### Task 10.2 · 清洁度

- [ ] `npx tsc --noEmit` 绿
- [ ] `npm run lint` 绿，0 warnings
- [ ] `npm run build` 绿
- [ ] `cargo test` 全绿
- [ ] `cargo clippy -- -D warnings` 绿

### Task 10.3 · DESIGN.md 对齐

- [ ] 对照 DESIGN.md 审 /cards 页和 /cards/:ch/:id 页：字体、色值、spacing、chip 样式
- [ ] 和 /chat 既有页面视觉一致性检查

### Phase 10 验收

- [ ] 所有黄金路径 + 失败路径通过
- [ ] 三联（tsc / lint / build / cargo test / clippy）全绿
- [ ] DESIGN.md 对齐无违规

---

## 全局约定

- 每个 Task commit message 前缀：`feat(webui)` / `feat(daemon)` / `refactor(webui)` / `test(daemon)` 等 conventional style
- Task 1.x 后端跑 `cargo test -p gitim-daemon`；Task 2.x+ 前端跑 `npx tsc --noEmit && npm run lint`
- Phase 切换时重跑全量（`cargo test` + `npx tsc --noEmit && npm run lint && npm run build`）
- 任何 regression 立刻停下修复，不累积
- 不引入新的前端依赖（Radix UI / Tailwind / Zustand / react-router / sonner / lucide-react 已够用）

## 回退预案

- P3 props 化若 /chat 回归严重：revert 该 phase 的 commits，改用"复制粘贴"方案（给 card 单独写 CardMessageList/CardInputArea），接受 DRY 违反换稳定性
- P1 backend poll 若 membership filter 有 corner case：在 daemon 日志加 warn，保守不推送，靠前端 mount refetch 兜底
