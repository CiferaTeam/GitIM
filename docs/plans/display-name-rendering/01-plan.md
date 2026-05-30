# Display Name 前端渲染层 — 实现计划

> **For agentic workers:** 用 TDD，每个 Task 完成即 commit。本计划是 `00-requirements.md` 的实现分解。

**Goal:** 让人在聊天界面看到 `display_name @handler`（@ / wake-up / routing 仍 100% 走 `handler`），通过一个全局 `handler → display_name` directory + 一个共享 `<HandlerName>` 组件实现。

**Architecture:** 后端唯一改动是让 `list_users` **加性**带上真人用户的 `display_name`（`user_infos` sidecar，`users: Vec<String>` 不变 → 零破坏 + 前端 store 零迁移）。前端建一个 React Context directory（agents ∪ 真人用户），所有 handler 露出点路由过同一个 `<HandlerName>` 组件，查不到一律回退裸 `@handler`。

**Tech Stack:** Rust（gitim-core / gitim-daemon）、daemon-web（TS）、React 19 + Zustand + Vite。

---

## 关键设计决策（已定）

### D1. Wire 兼容：加性 sidecar，不改 `users` 元素类型

设计文档自身有张力：「升级 users 字段」vs「旧前端读新 daemon 不挂」。把 `users: Vec<String>` 改成对象数组**不是加性的** —— auto-update 窗口内缓存的旧前端会把用户列表渲染成 `[object Object]`，违反硬约束。

**定论**：保留 `users: Vec<String>` 不变，**加性**新增 `user_infos: Option<Vec<ActiveUserEntry>>`。
- 旧前端读新 daemon：读 `users`（strings），忽略 `user_infos` → 正常。
- 新前端读旧 daemon：`user_infos` 缺失 → directory 无真人 display_name → 回退裸 handler（设计文档的 graceful 降级）。
- 前端 store 的 `users: string[]` 完全不动，零迁移。
- 代价：handler 在 `users` 和 `user_infos` 各出现一次（可忽略）。

### D2. Directory 数据源 —— 不单独存 me 的 display_name（YAGNI）

设计文档列三源：agents / 自己(/im/me) / 其他真人(list_users)。但**自己是注册用户，必在 `users/<me>.meta.yaml`**，所以 `user_infos` 已覆盖自己。`/im/me` 仍是 currentUser **handler** 的来源，但不需要单独的 me-display-name 管线。directory = `user_infos` ∪ `agents`。

### D3. 共享组件 `<HandlerName>` + Context directory

所有 handler 露出点换成 `<HandlerName handler={h} />`。组件从 Context 读 directory，查到渲染 `display_name` + muted/mono `@handler`，查不到（或 display_name === handler）渲染裸 `@handler`。非 JSX 场景（aria / title）用 `formatHandlerLabel(handler, directory): string`。

### D4. Render-site scope —— 聊天面 + 镜像，排除技术 ID 面

全仓 40+ 个 `@{handler}`。本层只覆盖**人在聊天语境**的点；cards/boards/crons/flows 里 handler 作技术 ID（mono 渲染，符合 DESIGN.md「mono = 技术值」），保持裸 `@handler`。完整 in/out 清单见末尾「Render-site 清单」。

---

## File Structure

**新建**
- `crates/gitim-core/src/responses.rs` — 加 `ActiveUserEntry` struct + `ListUsersResponse.user_infos` 字段（同文件）
- `products/gitim/frontend/src/lib/format-handler-display.ts` — `resolveDisplayName` / `formatHandlerLabel` 纯函数
- `products/gitim/frontend/src/hooks/use-display-name-directory.tsx` — `useDirectory` hook + `DisplayNameDirectoryProvider` Context
- `products/gitim/frontend/src/components/chat/handler-name.tsx` — `<HandlerName>` 组件
- 对应 `.test.ts(x)`

**修改**
- `crates/gitim-daemon/src/handlers/read.rs` — `handle_list_users` best-effort 读 meta
- `products/gitim/frontend/src/daemon-web/handlers.ts` — `users()` 加 `user_infos`
- `products/gitim/frontend/src/lib/types.ts` — `Agent.handler` 字段
- `products/gitim/frontend/src/lib/client.ts` — `mapBackendAgent` 填 `handler`
- `products/gitim/frontend/src/hooks/use-chat-store.ts` — 加 `userInfos` + setter
- `products/gitim/frontend/src/hooks/use-poll-loop.ts` — bootstrap + refresh 灌 `userInfos`
- 各 render-site（见清单）+ 在 chat 根挂 `DisplayNameDirectoryProvider`

---

## Task 1 — Rust 后端：list_users 加性带 display_name

**Files:** `crates/gitim-core/src/responses.rs`、`crates/gitim-daemon/src/handlers/read.rs`、`crates/gitim-daemon/tests/archive_user_test.rs`

- [ ] **Step 1.1 — 加 `ActiveUserEntry` + `user_infos` 字段**（responses.rs，紧邻 `ListUsersResponse`）

```rust
/// One row in `ListUsersResponse.user_infos`. Mirrors `ArchivedUserEntry`:
/// `handler` always present, `display_name` best-effort (omitted when the
/// active `users/<handler>.meta.yaml` is missing or unparseable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveUserEntry {
    pub handler: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}
```

`ListUsersResponse` 加字段（`users` / `archived_users` 不动）：

```rust
pub struct ListUsersResponse {
    pub users: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_users: Option<Vec<String>>,
    /// Wire-additive enrichment: per-active-user display_name. Always emitted
    /// by new daemons (best-effort), absent from old daemons. Frontends build
    /// their handler→display_name directory from this and fall back to bare
    /// handler when it's missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_infos: Option<Vec<ActiveUserEntry>>,
}
```

- [ ] **Step 1.2 — `handle_list_users` best-effort 读 meta**（read.rs:232，复刻 `handle_list_archived_users` 的 284-287 模式）

```rust
pub async fn handle_list_users(state: SharedState, include_archived: bool) -> Response {
    let users = state.users.read().await;
    let mut sorted: Vec<String> = users.clone();
    sorted.sort();

    // Best-effort display_name per active user. A read/parse failure means the
    // entry simply has no display_name on the wire — never an error for the list.
    let users_dir = state.repo_root.join("users");
    let user_infos: Vec<gitim_core::responses::ActiveUserEntry> = sorted
        .iter()
        .map(|handler| {
            let path = users_dir.join(format!("{handler}.meta.yaml"));
            let display_name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_yaml::from_str::<UserMeta>(&c).ok())
                .map(|m| m.display_name);
            gitim_core::responses::ActiveUserEntry {
                handler: handler.clone(),
                display_name,
            }
        })
        .collect();

    let archived_users = if include_archived {
        // ... 原逻辑不动 ...
    } else {
        None
    };

    let payload = gitim_core::responses::ListUsersResponse {
        users: sorted,
        archived_users,
        user_infos: Some(user_infos),
    };
    Response::json(payload)
}
```

> `UserMeta` 已在 read.rs 顶部 import（archived 路径用）。确认 import 在。

- [ ] **Step 1.3 — 更新 wire-shape 单测**（responses.rs:821 `list_users_response_wire_shape` / :841）：struct 字面量补 `user_infos: None`（默认调用不强制带），断言 `user_infos` 字段 `skip_serializing_if` 行为；加一个带 `user_infos: Some(vec![...])` 的 case 断言 JSON 形状 `{"handler":..,"display_name":..}`。
- [ ] **Step 1.4 — 更新 daemon 集成测试**（archive_user_test.rs:798 / :834）：反序列化用新 `ListUsersResponse`（`users` 仍 `Vec<String>`，不变）；加断言 `user_infos` 含写入的 display_name。
- [ ] **Step 1.5 — 跑 scoped 测试**：`cargo test -p gitim-core list_users` + `cargo test -p gitim-daemon --test archive_user_test`。CLI(`admin.rs`)/runtime(`http.rs`) 原样透传，无需改。
- [ ] **Step 1.6 — commit** `feat(daemon): list_users carries per-user display_name (additive user_infos)`

---

## Task 2 — daemon-web + 前端数据管线

**Files:** `daemon-web/handlers.ts`、`lib/types.ts`、`lib/client.ts`、`hooks/use-chat-store.ts`、`hooks/use-poll-loop.ts`

- [ ] **Step 2.1 — daemon-web `users()` 加性带 `user_infos`**（handlers.ts:639，镜像 Rust wire）

```ts
export async function users(): Promise<ApiResponse> {
  const s = getState();
  await refreshUsersCache();
  const userList = Array.from(s.users.keys());
  const userInfos = Array.from(s.users.entries()).map(([handler, meta]) => ({
    handler,
    display_name: meta.display_name,
  }));
  return ok({ users: userList, user_infos: userInfos });
}
```

- [ ] **Step 2.2 — `Agent.handler` 字段**（types.ts:59）：`Agent` 加 `/** 协议 handler，与 id/name 解耦 —— directory keying + agent-card 用 */ handler: string;`。`UserInfo`（types.ts:156）已存在，复用。
- [ ] **Step 2.3 — `mapBackendAgent` 填 handler**（client.ts:1354）：`return` 对象加 `handler: (raw.handler ?? raw.id) as string,`。
- [ ] **Step 2.4 — chat store 加 `userInfos`**（use-chat-store.ts）：state 加 `userInfos: UserInfo[]`（初值 `[]`，含 reset 路径 599-601），action 加 `setUserInfos: (u: UserInfo[]) => void` → `set({ userInfos: u })`。import `UserInfo`。
- [ ] **Step 2.5 — poll loop 灌 userInfos**（use-poll-loop.ts:418 bootstrap + :761 refresh）

```ts
// bootstrap (~419)
if (usersRes.ok && usersRes.data) {
  chatStore.setUsers(usersRes.data.users as string[]);
  chatStore.setUserInfos((usersRes.data.user_infos as UserInfo[] | undefined) ?? []);
}
// refresh (~761) — 沿用现有 diff 后，无条件 setUserInfos（cheap；或同样 diff）
if (usersRes.ok && usersRes.data) {
  const next = usersRes.data.users as string[];
  const current = useChatStore.getState().users;
  const changed = /* 原逻辑 */;
  if (changed) useChatStore.getState().setUsers(next);
  useChatStore.getState().setUserInfos(
    (usersRes.data.user_infos as UserInfo[] | undefined) ?? [],
  );
}
```

- [ ] **Step 2.6 — 测试**：`mapBackendAgent` 单测断言 `handler`；daemon-web `handlers.test.ts` users() 断言 `user_infos` 形状；chat store 测试断言 `setUserInfos`。
- [ ] **Step 2.7 — commit** `feat(web): plumb per-user display_name into frontend (Agent.handler + userInfos)`

---

## Task 3 — Directory 基础设施（hook + context + 组件 + formatter）

**Files:** 新建 `lib/format-handler-display.ts`、`hooks/use-display-name-directory.tsx`、`components/chat/handler-name.tsx` + 测试

- [ ] **Step 3.1 — `format-handler-display.ts`（TDD：先写测试）**

```ts
/** display_name 查表。未知、或 display_name === handler（避免 "alice @alice" 冗余）
 *  时返回 undefined，调用方据此渲染裸 @handler。 */
export function resolveDisplayName(
  handler: string,
  directory: ReadonlyMap<string, string>,
): string | undefined {
  const name = directory.get(handler);
  if (!name || name === handler) return undefined;
  return name;
}

/** aria-label / title 等纯字符串场景。 */
export function formatHandlerLabel(
  handler: string,
  directory: ReadonlyMap<string, string>,
): string {
  const name = resolveDisplayName(handler, directory);
  return name ? `${name} (@${handler})` : `@${handler}`;
}
```

测试覆盖：查到 → name；查不到 → undefined / `@handler`；name===handler → undefined / `@handler`。

- [ ] **Step 3.2 — `use-display-name-directory.tsx`：Context + Provider + 内容稳定 memo**

```tsx
import { createContext, useContext, useMemo, useRef, type ReactNode } from "react";
import { useAgentStore } from "./use-agent-store";
import { useChatStore } from "./use-chat-store";

const DirectoryContext = createContext<ReadonlyMap<string, string>>(new Map());
export function useDirectory(): ReadonlyMap<string, string> {
  return useContext(DirectoryContext);
}

export function DisplayNameDirectoryProvider({ children }: { children: ReactNode }) {
  const agents = useAgentStore((s) => s.agents);
  const userInfos = useChatStore((s) => s.userInfos);

  const built = useMemo(() => {
    const map = new Map<string, string>();
    for (const u of userInfos) {
      if (u.display_name && u.display_name !== u.handler) map.set(u.handler, u.display_name);
    }
    for (const a of agents) {
      const h = a.handler ?? a.id;
      if (a.name && a.name !== h) map.set(h, a.name);
    }
    return map;
  }, [agents, userInfos]);

  // 内容稳定：only swap identity when the handler→name mapping actually changes,
  // so poll churn on agents/userInfos arrays doesn't re-render every <HandlerName>.
  const sig = useMemo(
    () => [...built.entries()].sort().map(([k, v]) => `${k}=${v}`).join("\n"),
    [built],
  );
  const ref = useRef(built);
  const sigRef = useRef(sig);
  if (sig !== sigRef.current) {
    ref.current = built;
    sigRef.current = sig;
  }

  return <DirectoryContext.Provider value={ref.current}>{children}</DirectoryContext.Provider>;
}
```

测试：用 store mock 注入 agents/userInfos，断言 map 内容 + 同内容下 identity 稳定。

- [ ] **Step 3.3 — `<HandlerName>` 组件**

```tsx
import { useDirectory } from "../../hooks/use-display-name-directory";
import { resolveDisplayName } from "../../lib/format-handler-display";
import { cn } from "../../lib/utils";

interface HandlerNameProps {
  handler: string;
  className?: string;
  /** muted @handler 段的额外 class（默认 mono / text-muted / 0.85em）。 */
  handleClassName?: string;
  /** false 时只渲染 display_name（查不到则仍渲染裸 @handler）。默认 true。 */
  showHandle?: boolean;
}

export function HandlerName({ handler, className, handleClassName, showHandle = true }: HandlerNameProps) {
  const directory = useDirectory();
  const name = resolveDisplayName(handler, directory);
  if (!name) return <span className={className}>@{handler}</span>;
  return (
    <span className={className}>
      {name}
      {showHandle && (
        <span className={cn("ml-1 font-mono font-normal text-[0.85em] text-text-muted", handleClassName)}>
          @{handler}
        </span>
      )}
    </span>
  );
}
```

测试（@testing-library/react + Provider wrapper）：查到 → 同时含 name 与 `@handler`；查不到 → 仅 `@handler`；`showHandle={false}` + 查到 → 仅 name。

- [ ] **Step 3.4 — 在 chat 根挂 Provider**：找 chat 层根（`chat-layout.tsx` 或 `app-shell.tsx`），用 `<DisplayNameDirectoryProvider>` 包住聊天子树（含 mobile）。确保 message-list / thread-panel / mobile overlay / sidebar / header / input-area / mention-popup / user-card 都在其下。
- [ ] **Step 3.5 — commit** `feat(web): handler→display_name directory + <HandlerName> shared renderer`

---

## Task 4 — 聊天主面 render-site 改造

每个 site：`@{x}` → `<HandlerName handler={x} .../>`；mention-popup 额外改 filter；user-card 额外修 isAgent。每改一组跑相关组件测试，全部完成后一次 commit（或按文件 commit）。

- [ ] **4.1 message-item.tsx**：`:219` 作者头 → `<HandlerName handler={message.author} className="font-semibold text-sm text-foreground" />`；`:242` 回复引用作者 → `<HandlerName handler={replyTarget.author} .../>`（保留 `: ` 后缀）；`:285-290` recipient 回执 → `<HandlerName handler={recipient} showHandle .../>`（mono pill 已有，让 name 也进 pill）。
- [ ] **4.2 message-body.tsx**：`:107-119` mention case → 用 `<HandlerName handler={fragment.handler} />` 替 `@{fragment.handler}`（保留 onClick / hover 样式在外层 span）；`:149-161` user-profile 保持 `~` 前缀语义 —— 用 `<HandlerName>` 渲染 name 段，前缀仍 `~`（或 `resolveDisplayName` 拼）。
- [ ] **4.3 input-area.tsx**：`:237` 回复预览 `Reply to @{replyTo.author}` → `<HandlerName>`；`:316` recipient → `<HandlerName>`。插入逻辑 `handleMentionSelect`（:216 `<@${handle}>`）**不动**。
- [ ] **4.4 mention-popup.tsx**：props 仍收 `users: string[]`（handler 列表，键盘导航 & onSelect 仍是 handler）；组件内 `useDirectory()`，filter 改为 `(resolveDisplayName(u, dir)?.toLowerCase().includes(f) || u.toLowerCase().includes(f))`（敲 name 或 handler 都命中）；渲染 `<HandlerName handler={u} />`（`:87`）。
- [ ] **4.5 user-card.tsx**：`:15` isAgent 改 `agents.some((a) => a.handler === handler)`（更准）；`:58` 标题 → name 大字 + muted `@handler`（直接用 `resolveDisplayName` + 两行，或 `<HandlerName>` 配 class）。
- [ ] **4.6 thread-panel.tsx**：`:96` parent 作者、`:113` msg 作者 → `<HandlerName>`。
- [ ] **4.7 测试** + **commit** `feat(web): render display_name in message/thread/composer/mention/hover`

---

## Task 5 — DM 标题、sidebar、header 成员、当前用户身份、agent-card、mobile 镜像

- [ ] **5.1 DM 标题（header + sidebar）**
  - 保留 `formatDmDisplayName(name, currentUser): string` 用于 aria/title/edge-case 字符串。
  - 新增 `peerFromDmName`（sidebar 已用，确认其语义）取 1:1 peer handler。
  - header.tsx:116：1:1 DM → `<HandlerName handler={peer} />`，self-dm/pair/malformed → 原 string。
  - sidebar.tsx:1146/1190 `ChannelItem.label`：先查 `ChannelItem.label` 类型是否 `ReactNode`。是 → 1:1 传 `<HandlerName>`，`pinLabel/unpinLabel/archiveLabel` 仍用 `formatDmDisplayName`/`@peer` 字符串。否（string-only）→ 扩 `label?: ReactNode` 或新增 `labelNode?: ReactNode` 优先渲染。
- [ ] **5.2 sidebar DM 搜索**：`:1131` `@{u}` → `<HandlerName handler={u} />`，filter 若按 handler 现状保留（可选：也匹配 display_name，与 mention-popup 对齐）。
- [ ] **5.3 header 频道成员/创建者**：`:169` creator、`:195` member → `<HandlerName>`。
- [ ] **5.4 member-picker.tsx**：`:51`/`:98` `@{handle}` → `<HandlerName handler={handle} />`（filter 可选加 display_name 匹配）。
- [ ] **5.5 当前用户身份徽标**：app-shell.tsx:82、mobile-sidebar-drawer.tsx:154 `@{currentUser}` → `<HandlerName handler={currentUser} />`。
- [ ] **5.6 agent-card.tsx**：`:118-122` 在 `agent.name` 旁/下加 muted mono `@{agent.handler}`（设计文档明确要求；解决同名卡不可区分）。例：name span 后接 `<span className="font-mono text-[0.7rem] text-text-muted">@{agent.handler}</span>`。
- [ ] **5.7 mobile 聊天镜像**：mobile-thread-overlay.tsx:83/96、mobile-action-sheet.tsx:39 → `<HandlerName>`（与桌面 thread-panel 对齐，避免桌/移动不一致）。
- [ ] **5.8 测试** + **commit** `feat(web): display_name in DM titles, sidebar, members, identity, agent-card, mobile`

---

## Task 6 — 视觉 QA + 终审

- [ ] **6.1** `pnpm/npm test`（前端 vitest，scoped 到改动文件）+ `npm run build`（tsc 类型）+ `cargo test -p gitim-core -p gitim-daemon`（scoped）。
- [ ] **6.2** Playwright 起前端，截图核对：消息流作者头、mention、DM 标题、agent 列表卡（同名区分）、@ 弹窗双段过滤、hover 卡。对照 DESIGN.md（mono / text-muted / 小字）。
- [ ] **6.3** 边界：directory 未加载（裸 handler）、departed/历史 handler（裸）、display_name 缺失（裸）、同名（并排 @handler 天然区分）。
- [ ] **6.4** codex review + requesting-code-review。
- [ ] **6.5** finishing-a-development-branch。

---

## Render-site 清单（in / out 决策，No silent cap）

**IN（人在聊天语境 → `<HandlerName>`）**

| 文件:行 | 点 |
|---|---|
| message-item.tsx:219/242/290 | 作者头 / 回复引用 / 回执 |
| message-body.tsx:117/159 | mention / user-profile |
| input-area.tsx:237/316 | 回复预览 / 回执 |
| mention-popup.tsx:87 | @ 弹窗（+ 双段 filter） |
| user-card.tsx:58 | hover 卡（+ isAgent 修正） |
| thread-panel.tsx:96/113 | 线程父/消息作者 |
| header.tsx:116/169/195 | DM 标题 / 创建者 / 成员 |
| sidebar.tsx:1131/1146/1190 | DM 搜索 / DM 列表标题 |
| member-picker.tsx:51/98 | 成员选择 |
| app-shell.tsx:82, mobile-sidebar-drawer.tsx:154 | 当前用户身份 |
| agent-card.tsx | 补 handler（同名区分） |
| mobile-thread-overlay.tsx:83/96, mobile-action-sheet.tsx:39 | mobile 聊天镜像 |

**OUT（handler 作技术 ID，mono 渲染，保持裸 `@handler`；符合 DESIGN.md「mono=技术值」+ 设计文档未含）**

cards：card-create-dialog / card-meta-bar / card-filter-bar / card-kanban-cell / mobile-card-list（assignee / created_by）。boards：boards-view（board.handler）。crons：cron-day-panel / cron-run-viewer / cron-spec-detail（target / author / createdBy）。flows：run-detail / flow-detail（started_by / actor / owner）。management：archived-agent-card / burn-agent-dialog。setup：browser-workspace-form / workspace-switcher（后者是 commit hash，非 handler）。

> 若需 app-wide 一致，cards/crons 等可后续接入 `<HandlerName showHandle={false}>`（compact，仅 name），但 mono ID 场景多数保持裸 handler 更贴合 DESIGN.md。

---

## Spec 覆盖自检

- §1 Directory：Task 3（hook + context）✓ —— 源 agents ∪ user_infos（D2）。
- §2 后端：Task 1（Rust）+ Task 2.1（daemon-web）✓ —— 加性（D1）。
- §3 渲染统一：Task 4 + Task 5 ✓ —— `<HandlerName>` 单点（D3）。
- §4 Composer @ 弹窗双段 filter：Task 4.4 ✓。
- §5 Hover 卡 enrichment：Task 4.5 ✓。
- 边界（未加载 / 查不到 / 缺失 / 同名）：`resolveDisplayName` 回退语义 + Task 6.3 ✓。
- Non-goals（编辑 / 改协议 / agent 感知 / per-viewer）：不触及 ✓。
