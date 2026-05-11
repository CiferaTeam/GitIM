# Persist UI Selection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 `gitim-ui-state:<workspace_key>` localStorage entry,聚合存 `channel` / `boardHandler` / `cardsShowArchived` 三个字段;`app.tsx` 的 `reloadActiveWorkspaceState` selection 解析改为 "in-session preserve → storage hydrate → general fallback" 三阶,workspace 删除时同步清 storage。

**Architecture:** 新增 `src/lib/ui-state.ts` 作为统一 read / write / clear helper(模式跟 `sidebar.tsx` 现有的 `readKnownAgentIds` / `writePinnedConversations` 同构);Store(`useChatStore` / `useBoardStore` / `useCardStore`)保持 pure UI state,storage write 在 caller 显式调;hydrate 集中在 `reloadActiveWorkspaceState` 内,replace 当前 `general` fallback 分支。

**Tech Stack:** React 19 + TypeScript + Zustand(frontend),Vitest + Testing Library(frontend tests),仓库根 `npm test` 跑 frontend 测试,`cargo test` 不动(本次纯前端)。

**Design doc:** [`design.md`](./design.md)

**Convention:** 每个 task 只包含文件路径、变更描述、验收标准。不内联代码;实现细节在执行阶段由具体编辑者根据上下文写。

---

## Phase 0 · Baseline

### Task 0:跑一次全量 baseline,排除祖传红测干扰判断

**Files:** 不改动。

- [ ] **Step 1:** worktree 根跑 `cd products/gitim/frontend && npm test` —— 期望 PASS 或仅有已知红测。
- [ ] **Step 2:** 不 commit。结果只用于建 baseline。

---

## Phase 1 · Storage helper

### Task 1:新建 `src/lib/ui-state.ts`

**Files:**
- Create: [`products/gitim/frontend/src/lib/ui-state.ts`](../../../products/gitim/frontend/src/lib/ui-state.ts)
- Create: [`products/gitim/frontend/src/lib/ui-state.test.ts`](../../../products/gitim/frontend/src/lib/ui-state.test.ts)

- [ ] **Step 1:** 定义 `UiState` 类型 `{ channel: string | null; boardHandler: string | null; cardsShowArchived: boolean }`,导出默认值常量,以及 storage key prefix `gitim-ui-state:`(对齐 `gitim-known-agents:` / `gitim-pinned-conversations:` 命名风格)。
- [ ] **Step 2:** 实现 `readUiState(workspaceKey: string | null): UiState` —— null key 返回默认值;`localStorage` 缺省返回默认值;JSON.parse 失败或字段类型不符的字段单独 fallback 到默认值(不整条丢弃,做字段级宽容)。
- [ ] **Step 3:** 实现 `writeUiState(workspaceKey: string, patch: Partial<UiState>): void` —— 先 `readUiState` 拿当前值,merge patch,写回完整 JSON。
- [ ] **Step 4:** 实现 `clearUiState(workspaceKey: string): void` —— `localStorage.removeItem`。
- [ ] **Step 5:** 在测试文件里覆盖:read 缺省、read 损坏 JSON、read 部分字段类型不符的字段级 fallback、write merge 不覆盖未传字段、clear、`workspaceKey === null` 返回默认值且不写。
- [ ] **Step 6:** 跑 `npm test -- ui-state`,期望全部 PASS。
- [ ] **Step 7:** Commit。

---

## Phase 2 · Hydrate selection 解析改造

### Task 2:`reloadActiveWorkspaceState` 接入 storage hydrate

**Files:**
- Modify: [`products/gitim/frontend/src/app.tsx:265-477`](../../../products/gitim/frontend/src/app.tsx:265)(`reloadActiveWorkspaceState` 主体)

- [ ] **Step 1:** 在文件顶部 import `readUiState` from `@/lib/ui-state`。
- [ ] **Step 2:** 在 `reloadActiveWorkspaceState` 内,把 `previousChannel` 解析后、`nextChannel` fallback 前的逻辑改为三阶:
  - in-session preserve(`options.preserveSelection && previousChannel && selectableChannels.some(...)`)— 走 `previousChannel`
  - 否则读 `readUiState(workspaceKey).channel`,如果非空且在 `selectableChannels` 里,用它
  - 否则 `general` → `nextChannels[0]` → `null`
- [ ] **Step 3:** 对 `selectedBoardHandler` 做对称改造:`boardState.selectedHandler && exists ? : storedBoardHandler && exists ? : boards[0]?.handler ?? null`。`storedBoardHandler` 同样来自 `readUiState(workspaceKey).boardHandler`。
- [ ] **Step 4:** 跑 `npm test -- app`(或其他相关 app 测试,如果存在)。如果现有测试覆盖 selection 解析,可能需要更新;新增 selection 解析测试推迟到 Phase 4。期望已有测试 PASS。
- [ ] **Step 5:** Commit。

### Task 3:`reloadActiveWorkspaceState` 把 hydrate 出的 `cardsShowArchived` 写回 `useCardStore`

**Files:**
- Modify: [`products/gitim/frontend/src/app.tsx`](../../../products/gitim/frontend/src/app.tsx)(`reloadActiveWorkspaceState`)
- Modify: [`products/gitim/frontend/src/hooks/use-card-store.ts`](../../../products/gitim/frontend/src/hooks/use-card-store.ts)(确认是否需要 setter; `toggleShowArchived` 当前是 toggle,需要一个 `setShowArchived(value: boolean)` 入口供 hydrate 使用)

- [ ] **Step 1:** 在 `useCardStore` 加 `setShowArchived(v: boolean)` action,行为为 `set({ showArchived: v })`。其他既有 action 不动。
- [ ] **Step 2:** 在 `reloadActiveWorkspaceState` 末尾(`setCards` 调用之后)调 `setShowArchived(readUiState(workspaceKey).cardsShowArchived)`。`preserveSelection: true` 分支也走这条 —— 因为 cards 偏好不属于 in-session selection 范畴,统一以 storage 为准。
- [ ] **Step 3:** 跑 `npm test`。期望 PASS。
- [ ] **Step 4:** Commit。

---

## Phase 3 · Write 时机接入 callsite

### Task 4:`chat-layout.tsx` 主动 channel-select 三处接入 `writeUiState`

**Files:**
- Modify: [`products/gitim/frontend/src/components/chat/chat-layout.tsx:133-154`](../../../products/gitim/frontend/src/components/chat/chat-layout.tsx:133)(`handleChannelSelect`)
- Modify: 同文件 [`chat-layout.tsx:325-354`](../../../products/gitim/frontend/src/components/chat/chat-layout.tsx:325)(`handleNavBack`)

- [ ] **Step 1:** 在文件顶部 import `writeUiState` from `@/lib/ui-state`。
- [ ] **Step 2:** `handleChannelSelect` 内 `selectChannel(name)` 后:`if (workspaceKey) writeUiState(workspaceKey, { channel: name })`。
- [ ] **Step 3:** `handleNavBack` 内 `selectChannel(entry.channel)` 后做同样的 `writeUiState`。
- [ ] **Step 4:** **不**改 `secondary fallback effect`([:156-162](../../../products/gitim/frontend/src/components/chat/chat-layout.tsx:156)) —— 它走的也是 `handleChannelSelect`,write 自动跟着发生。
- [ ] **Step 5:** **不**改 `sidebar.tsx:317-336` 的 hidden DM auto-fallback —— 它走 `onChannelSelect` callback,落到 `handleChannelSelect`,write 自动跟着。
- [ ] **Step 6:** 跑 `npm test -- chat-layout`(如果有相关测试),期望 PASS。
- [ ] **Step 7:** Commit。

### Task 5:Board switcher 接入 `writeUiState`

**Files:**
- Modify: 调用 `useBoardStore.setSelectedHandler` 的 caller(在 `src/components/boards/` 下,执行时用 ripgrep 定位精确 callsite)
- Modify(可选): [`products/gitim/frontend/src/hooks/use-board-store.ts`](../../../products/gitim/frontend/src/hooks/use-board-store.ts) —— 不动 store 内部,仅在外部 caller 加 write。

- [ ] **Step 1:** `rg "setSelectedHandler" products/gitim/frontend/src/components` 找到 board switcher 入口(预期在 `boards-view.tsx` 或类似)。
- [ ] **Step 2:** 在该 caller 内,`setSelectedHandler(handler)` 调用之后,`if (workspaceKey) writeUiState(workspaceKey, { boardHandler: handler })`。如果 caller 当前没拿 `workspaceKey`,从 `useConnectionStore` + `useWorkspaceStore` 算出来(对照 `chat-layout.tsx` 现有写法)。
- [ ] **Step 3:** **不**在 `setSelectedHandler` action 内 write —— 因为 [`use-board-store.ts:20-34 setBoards`](../../../products/gitim/frontend/src/hooks/use-board-store.ts:20) 内部也会调 `selectedHandler = keep ?? boards[0]?.handler ?? null`,那个是从 poll 触发的 derived selection,不能落 storage。store 内部不区分场景,所以 write 只放在用户主动入口。
- [ ] **Step 4:** 跑相关 board 测试,期望 PASS。
- [ ] **Step 5:** Commit。

### Task 6:Cards archived toggle 接入 `writeUiState`

**Files:**
- Modify: 调用 `useCardStore.toggleShowArchived` 的 caller(在 `src/components/cards/` 下,执行时用 ripgrep 定位)

- [ ] **Step 1:** `rg "toggleShowArchived" products/gitim/frontend/src` 找到 toggle 入口。
- [ ] **Step 2:** 在该 caller 内 toggle 后,`if (workspaceKey) writeUiState(workspaceKey, { cardsShowArchived: <new value> })`。注意 toggle 是 flip,需要 caller 自己持有 next value(可以读 `useCardStore.getState().showArchived` 之后再算)。
- [ ] **Step 3:** 跑相关 cards 测试,期望 PASS。
- [ ] **Step 4:** Commit。

---

## Phase 4 · Workspace 删除清理

### Task 7:`useWorkspaceStore.remove` 删除成功后 `clearUiState`

**Files:**
- Modify: [`products/gitim/frontend/src/hooks/use-workspace-store.ts:108-121`](../../../products/gitim/frontend/src/hooks/use-workspace-store.ts:108)(`remove` action)

- [ ] **Step 1:** 在文件顶部 import `clearUiState` from `@/lib/ui-state`。
- [ ] **Step 2:** 在 `client.deleteWorkspace(slug)` 返回 `ok` 之后、`fetchAll()` 之前,根据 `slug` 算出当前 mode 下的 `workspaceKey`(`workspaceIdentity(mode, workspace)`,workspace 从删除前的 `get().workspaces` 里捞),调 `clearUiState(workspaceKey)`。如果 workspace 已经在 store 里找不到(并发删除等),skip。
- [ ] **Step 3:** 跑 `npm test -- use-workspace-store`,期望 PASS。如果原测试不覆盖 storage 清理,在同测试文件加一条新断言:删除 ws 后该 ws 的 `ui-state` storage entry 不存在。
- [ ] **Step 4:** Commit。

---

## Phase 5 · 手测 + 收尾

### Task 8:手测刷新场景

**Files:** 不改动。

- [ ] **Step 1:** `cd products/gitim/frontend && npm run dev` 起 webui;runtime 单独跑(`gitim-runtime --port 16868 -d` 或现有 dev 流程)。
- [ ] **Step 2:** 进入某 ws,选一个非 `general` 的 channel(比如 `design` 或一个 DM),刷新浏览器 → 期望仍停在该 channel,而不是跳回 `general`。
- [ ] **Step 3:** 切到 `/boards`,选一个非默认 board,刷新 → 期望仍停在该 board。
- [ ] **Step 4:** 切到 `/cards`,切到 archived 视图,刷新 → 期望仍是 archived 视图。
- [ ] **Step 5:** 切到另一个 workspace,选另一个 channel,刷新,切回第一个 ws → 期望两个 ws 各自记住自己的 channel。
- [ ] **Step 6:** 删除某 ws → DevTools → Application → Local Storage,确认 `gitim-ui-state:<被删 ws 的 key>` 已清。
- [ ] **Step 7:** 把存储里的 channel 改成一个不存在的名字,刷新 → 期望 fallback 到 `general`,且 storage 里被纠正成 `general`(下次 select 时会被覆盖)。
- [ ] **Step 8:** 不 commit(纯手测验证)。

### Task 9:跑 full frontend 测试 sanity

**Files:** 不改动。

- [ ] **Step 1:** `cd products/gitim/frontend && npm test` 全量,期望 PASS 或仅有 baseline 时已存在的祖传红测。
- [ ] **Step 2:** 跑 typecheck / lint 如果项目有现成 npm script(`npm run typecheck` / `npm run lint`),期望干净。
- [ ] **Step 3:** 不 commit。
