# Persist per-workspace UI selection (channel / board / cards 偏好)

## Goal

刷新页面 / 二次打开 webui 时,**用户回到上次离开的位置**,而不是被强制 fallback 回 `general`。统一以 "per-workspace `gitim-ui-state:<workspace_key>`" 这一类 localStorage entry 承载这类"上次位置"语义,跟现有的 `gitim-pinned-conversations:`、`gitim-known-agents:`、`gitim:cursor:` 同构。

## Background

### 当前 bug 现场

[products/gitim/frontend/src/app.tsx:368-378](../../../products/gitim/frontend/src/app.tsx:368) 的 `reloadActiveWorkspaceState`:

```
let nextChannel: string | null = null;
if (
  options.preserveSelection &&
  previousChannel &&
  selectableChannels.some((c) => c.name === previousChannel)
) {
  nextChannel = previousChannel;
}
nextChannel ??=
  nextChannels.find((c) => c.name === "general")?.name ??
  nextChannels[0]?.name ??
  null;
```

[products/gitim/frontend/src/app.tsx:738-787](../../../products/gitim/frontend/src/app.tsx:738) 的 `init()` 用 `preserveSelection: false` 调它,导致首次启动 / 刷新走 fallback 分支。`useChatStore.currentChannel` 是纯 in-memory zustand state([products/gitim/frontend/src/hooks/use-chat-store.ts:68-83](../../../products/gitim/frontend/src/hooks/use-chat-store.ts:68)),刷新即丢,所以 `previousChannel` 永远是 `null`。

同类问题在 [`useBoardStore.selectedHandler`](../../../products/gitim/frontend/src/hooks/use-board-store.ts:15) 和 [`useCardStore.showArchived`](../../../products/gitim/frontend/src/hooks/use-card-store.ts) 上也存在 —— 都是 zustand in-memory state,刷新就掉。

### 现有 storage 全景(已持久化部分)

| Key | 维度 | 内容 |
|---|---|---|
| `gitim-runtime-port` / `gitim-connection-mode` | global | runtime 接入 |
| `gitim-active-workspace` / `gitim-active-browser-workspace` | per-mode | 当前 ws slug |
| `gitim-browser-workspaces-v2` | local mode | browser ws 注册表 |
| `gitim-browser-token:<id>` (sessionStorage) | per-ws | token(故意不入 localStorage) |
| `gitim:cursor:<ws_key>` | per-ws | poll cursor |
| `gitim-known-agents:<ws_key>` | per-ws | 已知 agent id 集合 |
| `gitim-pinned-conversations:<ws_key>` | per-ws | sidebar 置顶 channels / dms |
| `gitim-theme` | global | zustand persist middleware |
| `gitim:uuid` | global | device UUID |

**已经有成熟的 per-ws storage helper 模式**:`sidebar.tsx` 里的 `readKnownAgentIds` / `writeKnownAgentIds` / `readPinnedConversations` / `writePinnedConversations`,key 后缀用 `workspaceIdentity(mode, workspace)`。

### 没存的 UI 位置(本次目标)

| 状态 | 当前位置 | 用户感知 |
|---|---|---|
| `currentChannel` | `useChatStore` | ★ 高(此 bug 主体) |
| `selectedHandler` (board) | `useBoardStore` | 中(/boards 路由也跳第一个 board) |
| `showArchived` (cards) | `useCardStore` | 中(用户偏好) |

顶层路由(`/chat` `/cards` `/boards` `/management` 以及详情页 `/cards/:channel/:card_id`、`/management/:agentId`)走 React Router,刷新天然不丢,**不在本次 scope**。Sidebar 折叠态、`/` index 落点(`/chat` vs `/management`)的默认规则也保持现状,YAGNI。

## Approach

### 单一 storage key,聚合 JSON

新建一个 `gitim-ui-state:<ws_key>` localStorage entry,JSON shape:

```
{
  "channel": string | null,        // useChatStore.currentChannel
  "boardHandler": string | null,   // useBoardStore.selectedHandler
  "cardsShowArchived": boolean     // useCardStore.showArchived
}
```

**为什么聚合不分 key**:三个字段都是"用户在某 ws 里上次的位置/偏好",一次 hydrate 读一次 storage、一次 write 写一次,语义内聚。三个独立 key 会让 storage namespace 噪声更大,跨字段一致性也更难保证(虽然单字段独立写入也不算大问题)。

**为什么不用 `zustand/middleware` 的 `persist`**:`persist` 是 per-store 而非 per-workspace。要让它支持 ws 维度需要动态 storage key,跟现有 4 个 per-ws key 不同构,而且 `useChatStore` 还有大量非位置字段(messages / replyTo / threadRoot 等)绝对不能 persist —— 用 `partialize` 也要小心维护。手写 helper 跟 `sidebar.tsx` 现有风格一致,更可控。

### Storage helper(新 lib)

新建 `src/lib/ui-state.ts`:

- `readUiState(workspaceKey: string | null): UiState` —— null ws key 返回默认值;损坏 JSON 返回默认值
- `writeUiState(workspaceKey: string, patch: Partial<UiState>): void` —— merge-write,只覆盖传入字段
- `clearUiState(workspaceKey: string): void` —— hard delete(workspace 被删除时调)

字段缺省值: `{ channel: null, boardHandler: null, cardsShowArchived: false }`。

### Write 时机(只在"用户主动选"时写)

| 操作 | Caller | 在哪里 write |
|---|---|---|
| 用户点 channel / DM | `chat-layout.handleChannelSelect` / `handleNavBack` | caller 显式调 `writeUiState` |
| 用户切 board | `useBoardStore.setSelectedHandler` 的 caller | caller 显式调 |
| 用户切 cards archived 视图 | `useCardStore.toggleShowArchived` 的 caller | caller 显式调 |

**不写**的场景:`setChannels` / `setBoards` 从 poll 来的数据触发的 derived selection(`setBoards` 内自动落到第一个 board)、`resetForWorkspaceSwitch` 时(切 ws 不能污染目标 ws 的 storage)、`init()` 内 hydrate 出来的回填 selection。

**Store 不直接调 storage**:`useChatStore` / `useBoardStore` / `useCardStore` 不引用 `ui-state.ts`,保持 pure UI state;写入由 caller 在已经知道 `workspaceKey` 的上下文里发起。`chat-layout` / `app.tsx` 已经在算 `workspaceKey`,不增加新依赖。

### Read / Hydrate 时机

[app.tsx:738 `init()`](../../../products/gitim/frontend/src/app.tsx:738) 调 `reloadActiveWorkspaceState(slug, key, { preserveSelection: false })`。

把 selection 解析逻辑统一到 `reloadActiveWorkspaceState` 内部:

1. 如果 `options.preserveSelection && previousChannel && exists`,用 `previousChannel`(in-session reload 走这支,保留 SSE/poll-reset 期间用户的选择)
2. 否则读 `readUiState(workspaceKey).channel`,如果还在 `selectableChannels` 里,用它
3. 否则 fallback `general` → `nextChannels[0]`

对应的 board hydrate 也合并进来:相同流程,选 `boardHandler`(已经有 keep-selected-if-still-present 逻辑,改成 `selectedHandler ?? storedHandler ?? boards[0]`)。

`cardsShowArchived`:第一次 `useCardStore` mount 时一次性 hydrate(在 `app.tsx` 的 workspace switch effect 内,跟 `resetForWorkspaceSwitch` 后立刻 set)。

### Workspace 切换语义

- 切 ws 时:in-memory `reset*ForWorkspaceSwitch()`(已有);**不**清 storage(每个 ws 的"上次位置"独立)
- 切回旧 ws 时:`init()` 重新读 storage hydrate
- 删除 ws 时:`useWorkspaceStore.remove(slug)` 在 `client.deleteWorkspace` 成功后顺手 `clearUiState(workspaceKey)`

这跟现有 `gitim-known-agents:` / `gitim-pinned-conversations:` 已经验证过的模式一致。

### Hidden DM auto-fallback 不动

[sidebar.tsx:317-336](../../../products/gitim/frontend/src/components/chat/sidebar.tsx:317) 的 "当前 DM 因 agent 离开变 hidden 时自动跳走" 是另一语义(运行期 selection 失效),不是"刷新跳 general",**保留现状**。

### chat-layout secondary fallback 不动

[chat-layout.tsx:156-162](../../../products/gitim/frontend/src/components/chat/chat-layout.tsx:156) 的 effect 在 `currentChannel === null && general exists` 时调 `handleChannelSelect("general")`。`init()` 修好后这个 effect 实际上永远不会触发(channels 到位前 currentChannel 已经 hydrate 完毕),但保留不影响正确性,这次 **不删**,避免 scope 蔓延。

## Non-goals

- URL-first 方案(channel 进 path,DM 名 encode):成本大、暴露 channel 名,改路由结构,留作未来"可分享链接"feature。
- `/` index 路由的 mode-dependent 落点改成"上次 tab":URL 不在 store 维度,跟本次 storage helper 形态不一致,YAGNI。
- Sidebar archived sections / dm-search 折叠态持久化:刷新跨度上对用户感知低,这次不做。
- 跨设备同步:`gitim-ui-state:` 是浏览器 local;agent 同步语义本身就走 git,UI 偏好不入 git。
- 字段 schema 演进 / migration:全新 key,无旧版本数据;损坏 JSON 已 fallback 默认值。

## Risks

- **Write 漏点**:用户主动选 channel / board / cards 偏好的入口分散,容易漏写。Mitigation:三个入口点(`chat-layout.handleChannelSelect` / `handleNavBack` / board switcher / cards archived toggle)都在 caller 显式调一个统一 helper,单元测试覆盖每个入口的写入断言。
- **Hydrate 时机竞态**:`init()` 内 hydrate 跟 `chat-layout.tsx:156-162` 的 effect 可能竞争首次 select。Mitigation:`init()` 在 `reloadActiveWorkspaceState` 返回前已经把 `nextChannel` 落入 store,effect 见到 `currentChannel !== null` 直接 short-circuit,无竞态。
- **不存在 selection 兜底**:storage 里的 channel / board 在 server 端被删了 → 读出后校验 `selectableChannels.some(...)`,失败则丢弃并 fallback,跟 `preserveSelection` 现有校验一致。
- **多 tab 不同步**:同一 ws 在两个 tab 各自选不同 channel,storage 互相覆盖,刷新只剩最后一个。这是 localStorage 固有行为,跟现有 `gitim-pinned-conversations:` 同样问题,**接受现状**。
