# Archive Frontend Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Per memory `feedback_plan_no_code`, this plan lists task boundaries, files, and steps — not Rust/TS code. Implementers own the code.

**Goal:** 把 Card 归档 / Channel 归档两项能力从 CLI+daemon 层延伸到 AI 前端（prompts）和人类前端（Runtime HTTP + WebUI-v2），形成对称、可用的归档 UX。

**Architecture:** 七层增量（Daemon → Client → CLI → Runtime HTTP → WebUI API/Store → WebUI UI → Prompts）。Channel 侧补齐缺失的 `unarchive_channel` 对称操作；Card 侧已有 daemon 实现，仅做上层暴露。所有写操作走 daemon 的 git mv + commit + push-retry 既有范式。

**Tech Stack:** Rust (daemon/client/cli/runtime)、axum (HTTP)、React 19 + Vite + Zustand (WebUI-v2)、vitest/cargo test。

---

## File Structure

### Rust 新增/改动

| 文件 | 作用 | 变更 |
|------|------|------|
| `crates/gitim-daemon/src/api.rs` | IPC 类型 | 新增 `Request::UnarchiveChannel { channel, author }`、`Event::ChannelUnarchived`，补 serde 测试 |
| `crates/gitim-daemon/src/handlers.rs` | 命令分发 + handler | 新增 `handle_unarchive_channel`（mirror `handle_archive_channel` 带 commit-fail rollback），dispatch arm，`is_write` 加 `UnarchiveChannel` |
| `crates/gitim-daemon/tests/*` | handler 测试 | 新增 `unarchive_channel` 场景覆盖（成功、不存在、非 creator、目标 name 冲突） |
| `crates/gitim-client/src/client.rs` | IPC 客户端 | 新增 `unarchive_channel()` 方法 + roundtrip test |
| `crates/gitim-cli/src/main.rs` | CLI | 新增 `UnarchiveChannel { name }` 顶级 command + dispatch |
| `crates/gitim-runtime/src/http.rs` | Runtime HTTP | 新增 6 个 handler（card archive/unarchive/archived + channel archive/unarchive/archived），路由注册 |
| `crates/gitim-runtime/tests/*` | HTTP 集成 | 新增 6 端点 happy-path 测试 |
| `crates/gitim-agent-provider/src/prompts.rs` | AI 工具清单 | 扩展 `default_gitim_api`：加 `### 看板` 段（card 全套）+ 频道段补 archive/unarchive/archived |

### WebUI 新增/改动

| 文件 | 作用 | 变更 |
|------|------|------|
| `webui-v2/src/lib/client.ts` | HTTP fetch 层 | 新增 `archiveCard`, `unarchiveCard`, `listArchivedCards`, `archiveChannel`, `unarchiveChannel`, `listArchivedChannels` |
| `webui-v2/src/hooks/use-card-store.ts` | Zustand store | 新增 archive/unarchive actions、`showArchived` toggle、`archivedCards` 缓存 |
| `webui-v2/src/hooks/use-connection-store.ts` | 连接/频道 store | 新增 archive/unarchive/archivedChannels 字段 + actions |
| `webui-v2/src/components/cards/card-filter-bar.tsx` | 过滤器 | 加 "Show archived" toggle，触发 store 拉取 archived cards |
| `webui-v2/src/components/cards/card-detail.tsx` | Card drawer | archived 状态显示只读 banner + Unarchive 按钮；active 显示 Archive 按钮 |
| `webui-v2/src/components/cards/card-kanban*.tsx` | Kanban 视图 | 接 `showArchived` 展示 archived cards（低调样式） |
| `webui-v2/src/components/layout/*` | Sidebar | 加 "Archived channels" 折叠区，inline 列出 archived channels + Unarchive |
| `webui-v2/src/**/*.test.ts[x]` | 单元测试 | client.ts + store + 关键组件覆盖 |

---

## Task 1: Daemon layer — unarchive_channel 对称补齐

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers.rs`
- Create: `crates/gitim-daemon/tests/unarchive_channel.rs`

**Context:** channel archive 已有（`handle_archive_channel` @ handlers.rs:1271）。本 task 把其反向镜像实现，作为后续所有上层（HTTP/WebUI/prompts）调用的基础。权限模型保持「only channel creator」与 archive 对称。commit-fail 要 rollback（参考 card-archive v1 给 archive_channel 遗留的旧 bug，本次**不修** archive_channel，但新增的 unarchive_channel 必须 rollback 完整）。

- [ ] **Step 1: 在 api.rs 定义 Request::UnarchiveChannel + Event::ChannelUnarchived**，补 serde roundtrip 测试（raw JSON，因为 Request 只 Deserialize）。
- [ ] **Step 2: 运行 `cargo test -p gitim-daemon api::`，确认新测试 pass**。
- [ ] **Step 3: 在 handlers.rs 写 handle_unarchive_channel**（mirror archive_channel 的 8 步：validate name / registered author / 读 archive/channels/{}.meta.yaml / creator 权限 / 确认 channels/{}.meta.yaml 不存在（拒 name_conflict）/ 建 channels/ 目录 / 两个文件 git mv 反向 / add+commit，**commit fail 时 reverse mv 完整回滚两文件** / push-retry）。
- [ ] **Step 4: dispatch 加 UnarchiveChannel arm 调 handle_unarchive_channel**。
- [ ] **Step 5: `is_write` match 加 UnarchiveChannel**（guest-mode denylist，关键防护）。
- [ ] **Step 6: 写 tests/unarchive_channel.rs，覆盖:**
  - happy path: archive 完 → unarchive → 文件回到 channels/
  - archive 源不存在 → error
  - 非 creator 调用 → error
  - 目标 name 已存在（channels/{}.meta.yaml 存在） → error + 无副作用
  - commit fail（装 pre-commit hook exit 1）→ reverse mv 回滚，两文件都不动
- [ ] **Step 7: 跑 `cargo test -p gitim-daemon`，全绿**。
- [ ] **Step 8: Commit**：`feat(daemon): add unarchive_channel handler with commit-fail rollback`。

## Task 2: Client IPC + CLI

**Files:**
- Modify: `crates/gitim-client/src/client.rs`
- Modify: `crates/gitim-cli/src/main.rs`

**Context:** 上一步暴露了 daemon IPC；这一步让 Rust 客户端和 CLI 可以调用。对称 `archive_channel`/`ArchiveChannel` 现有代码。

- [ ] **Step 1: client.rs 加 async fn unarchive_channel(channel)，参照 archive_channel 的 request 结构**，加 mock IPC test（若已有 daemon helper）。
- [ ] **Step 2: `cargo test -p gitim-client`，pass**。
- [ ] **Step 3: main.rs 加 Commands::UnarchiveChannel { name } 枚举变体 + doc + dispatch 调 client.unarchive_channel()**。
- [ ] **Step 4: 手动 smoke:** 
  - `gitim archive-channel test-ch` → ok
  - `gitim unarchive-channel test-ch` → ok
  - `gitim channels | grep test-ch` → 出现
- [ ] **Step 5: Commit**：`feat(client, cli): expose unarchive_channel`。

## Task 3: Runtime HTTP endpoints (6)

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`
- Create: `crates/gitim-runtime/tests/http_archive.rs`（或扩展现有 HTTP 集成测试文件）

**Context:** Runtime 是 WebUI 唯一网关。现有 card HTTP handler 都是 `api_response_to_json(client.xxx().await)` 的 thin wrapper（参考 `im_create_card` @ http.rs:846-865）。保持该风格。路径采 subresource 风格（Q2 决策）。

- [ ] **Step 1: 定义 `im_card_archive`, `im_card_unarchive`, `im_list_archived_cards` 三个 handler**，签名按 axum Path/Query 习惯 — archive/unarchive 从 path 取 channel+card_id，list 从 query 取可选 channel。
- [ ] **Step 2: 定义 `im_channel_archive`, `im_channel_unarchive`, `im_list_archived_channels` 三个 handler**，从 path 取 name。
- [ ] **Step 3: 路由表加 6 条:**
  ```
  POST /im/cards/{channel}/{card_id}/archive    → im_card_archive
  POST /im/cards/{channel}/{card_id}/unarchive  → im_card_unarchive
  GET  /im/cards/archived                       → im_list_archived_cards
  POST /im/channels/{name}/archive              → im_channel_archive
  POST /im/channels/{name}/unarchive            → im_channel_unarchive
  GET  /im/channels/archived                    → im_list_archived_channels
  ```
  注意：`/im/cards/archived` 必须注册在 `/im/cards/{channel}/{card_id}` 之前或 axum 会误匹配参数。
- [ ] **Step 4: 集成测试**：启真实 daemon + runtime，6 个端点各一条 happy path。至少确认：
  - POST archive 后 GET archived 能看到
  - POST unarchive 后 GET archived 看不到
  - archive 404 / 权限拒的透传是 daemon error JSON
- [ ] **Step 5: `cargo test -p gitim-runtime`，pass**。
- [ ] **Step 6: Commit**：`feat(runtime): expose archive/unarchive HTTP endpoints for cards and channels`。

## Task 4: WebUI lib/client.ts + Zustand store

**Files:**
- Modify: `webui-v2/src/lib/client.ts`
- Modify: `webui-v2/src/hooks/use-card-store.ts`
- Modify: `webui-v2/src/hooks/use-connection-store.ts`
- Create/modify: `webui-v2/src/**/*.test.ts` 对应测试

**Context:** client.ts 已有 createCard/listCards/readCard/updateCard 6 个函数的 fetch 模式（@ client.ts:157-240）。对称加 6 个函数。store 目前维护 cards by channel，加 `archivedCards` Map + `showArchived: boolean` + 一组 actions。

- [ ] **Step 1: client.ts 新增 6 个 export async function，URL 按 Task 3 路由表，错误透传**。HTTP method: archive/unarchive 是 POST（无 body），list 是 GET（query 参数）。
- [ ] **Step 2: vitest 覆盖 6 函数的 URL 构造和错误处理**，fetch mock。
- [ ] **Step 3: use-card-store.ts 扩展 state**: `archivedCards: Map<string, Card[]>`, `showArchived: boolean`；actions `archiveCard(channel, id)`, `unarchiveCard(channel, id)`, `loadArchivedCards(channel?)`, `toggleShowArchived()`. archive 成功后把 card 从 active list 移除，并把 updated card 放进 archivedCards。
- [ ] **Step 4: use-connection-store.ts 扩展 state**: `archivedChannels: Channel[]`；actions `archiveChannel(name)`, `unarchiveChannel(name)`, `loadArchivedChannels()`. archive 后把 channel 从 channels 列表移除并入 archivedChannels。
- [ ] **Step 5: store 单元测试**，mock client 函数，验证 state 迁移正确（archive → 两个 list 变动）。
- [ ] **Step 6: `cd webui-v2 && pnpm test`，全绿**。
- [ ] **Step 7: Commit**：`feat(webui): archive/unarchive client and store`。

## Task 5: WebUI UI 入口

**Files:**
- Modify: `webui-v2/src/components/cards/card-filter-bar.tsx`
- Modify: `webui-v2/src/components/cards/card-detail.tsx`
- Modify: `webui-v2/src/components/cards/card-kanban.tsx`（或相关 column/cell）
- Modify: `webui-v2/src/components/layout/*`（sidebar）

**Context:** WebUI-v2 已有完整 kanban UI。本 task 加"最小可用"入口：过滤 bar toggle + detail drawer 按钮 + sidebar archived section。遵循 `DESIGN.md`（项目 CLAUDE.md 强制）。

- [ ] **Step 1: card-filter-bar 加 "Show archived" toggle**（使用 ui/ 下已有 Switch/Checkbox 组件），绑定 `useCardStore.showArchived`。toggle 打开时 trigger `loadArchivedCards(currentChannel)`。
- [ ] **Step 2: card-kanban 展示逻辑**：`showArchived = true` 时把 archivedCards 追加到末尾（或每一列尾部），样式低调（opacity 50% + "Archived" 角标）。
- [ ] **Step 3: card-detail drawer 按 archived 状态切换操作**:
  - active → 显示 "Archive" 按钮（调 `archiveCard`）
  - archived → 顶部显示只读 banner "This card is archived." + 显示 "Unarchive" 按钮（调 `unarchiveCard`）
  - archived 状态下禁用其它编辑交互（status 切换、label 编辑等）— 读接口 `readCard` 已返回 `archived: bool`，依赖该字段。
- [ ] **Step 4: sidebar 加 "Archived channels" 折叠 section**：从 `useConnectionStore.archivedChannels` 取数据，inline 列出，每项末尾一个 "Unarchive" icon-button 触发 `unarchiveChannel`。初始折叠，点开时 `loadArchivedChannels`。
- [ ] **Step 5: 组件测试**（vitest + testing-library）覆盖：toggle 触发 load、按钮切换、Archive 按钮点击后 card 从 active 消失。
- [ ] **Step 6: `pnpm test && pnpm build`，全绿**。
- [ ] **Step 7: 手动 smoke:**（需要 daemon 和 runtime 在跑）
  - 打开 kanban → Archive 一个 card → 从 kanban 消失
  - toggle Show archived → 看到低调显示
  - 打开 detail → Unarchive → 回到 active
  - sidebar Archived channels 折叠区 Unarchive → 频道回到 sidebar
- [ ] **Step 8: Commit**：`feat(webui): archive UI entry points for cards and channels`。

## Task 6: Prompts default_gitim_api 扩展

**Files:**
- Modify: `crates/gitim-agent-provider/src/prompts.rs`
- Modify: `crates/gitim-agent-provider/src/prompts.rs` 单元测试（若有）

**Context:** 当前 `default_gitim_api` (@ prompts.rs:250-294) 只列 send/read/dm/channels/search，**无 card 系列**。本次全量补齐 card 操作（Q4 决策）。严格按 CLI 现有命令写命令形态（注意 channel 的 archive/unarchive 是顶级命令 `gitim archive-channel <name>`，而 card 是子命令 `gitim card archive <channel> <id>`，**不对称，原样写**）。

- [ ] **Step 1: prompts.rs 在"频道"段末尾加 archive/unarchive 子项**:
  - `gitim archive-channel <name>` — 归档频道（仅 creator）
  - `gitim unarchive-channel <name>` — 取消归档
  - `gitim archived-channels` — 列出归档频道
- [ ] **Step 2: 在"搜索"前插入"### 看板 (Cards)"新段**，覆盖:
  - `gitim card create --channel <ch> --title "..."` [--label, --assignee, --status]
  - `gitim card list` [--channel, --label, --status, --assignee]
  - `gitim card read <channel> <card_id>` [--limit, --since]
  - `gitim card message <channel> <card_id> "<body>"` [--reply-to]
  - `gitim card update <channel> <card_id>` [--status, --label, --assignee]
  - `gitim card archive <channel> <card_id>`（仅 creator 或 assignee）
  - `gitim card unarchive <channel> <card_id>`
  - `gitim card archived` [--channel]
  写清：archived 的 card 无法发消息/编辑，需先 unarchive；archived channel 下 unarchive card 会被拒绝。
- [ ] **Step 3: 添加 prompts 内容断言测试**：`default_gitim_api(&ctx)` 字符串包含 "card archive" / "unarchive-channel" / "archived-channels" 关键字。防止回归。
- [ ] **Step 4: `cargo test -p gitim-agent-provider`，pass**。
- [ ] **Step 5: Commit**：`feat(prompts): expose cards and archive commands in default_gitim_api`。

## Task 7: Final verification

**Files:** none（验证 only）

- [ ] **Step 1: 全量回归**:
  ```
  cargo test --workspace
  cd webui-v2 && pnpm test && pnpm build
  ```
- [ ] **Step 2: 验证 archive-channel 现有旧 bug 仍然存在**（本 plan 明确不修），在 plan 末尾 follow-ups 记录。
- [ ] **Step 3: 验证所有新增端点/命令在手工 smoke 下闭环**:
  - Rust: `gitim archive-channel / unarchive-channel / archived-channels`
  - HTTP: `curl -X POST http://localhost:<port>/im/channels/<n>/archive` 等 6 端点
  - WebUI: Task 5 的 smoke 步骤
- [ ] **Step 4: grep 验证**:
  - `grep -n "card archive" crates/gitim-agent-provider/src/prompts.rs` 有命中
  - `grep -rn "unarchiveChannel" webui-v2/src/lib/client.ts` 有命中
  - `grep -n "POST.*cards.*archive\|POST.*channels.*archive" crates/gitim-runtime/src/http.rs` 有命中
- [ ] **Step 5: 进入 Phase 6 (Claude + Codex review)**。

---

## Scope out / Known follow-ups

- **`handle_archive_channel` 旧 commit-fail rollback 不完整**（handlers.rs:1322-1328 只回滚 meta 不回滚 thread）— 存在已久，本次不动，留 follow-up。新增的 `handle_unarchive_channel` 本身必须 rollback 完整。
- **Channel 归档不级联 cards**：channel 归档后其下活跃 card 仍在 `channels/<ch>/cards/*`（孤儿）— card-archive v1 记录的 "channel-cascade-cards bug"，未修。
- **WebUI UX 抛光**：empty state、确认 dialog、toast 反馈、动画、排序策略、keyboard shortcut — 本次（Q3 最小 UI）不做。
- **gitim-index 的 `include_archived` flag** — 搜索目前不会命中 archived 文件，待 index 层单独处理。
- **Board 其它操作**（label 详解、assignee 详解、状态转换规则）在 prompts 里一笔带过即可，不展开教程。

---

## Test strategy

- **Daemon**：handler 集成测试覆盖 happy + 4 类失败路径（Task 1 Step 6）。
- **Client/CLI**：client IPC roundtrip + CLI smoke。
- **Runtime HTTP**：6 端点 happy path 集成（真实 daemon + runtime），error 透传靠 daemon 测试覆盖。
- **WebUI**：lib/client fetch mock + store actions + 关键组件 toggle 行为。
- **Prompts**：字符串关键词 assertion 测试。
- **E2E 手工**：Task 5 smoke 步骤（无自动化 E2E，人工验证）。

## Self-review checklist

- [x] 所有 7 task 都对应 Q1-Q6 的决策
- [x] 每个新增 API / 函数都有测试步
- [x] 无占位符 "TBD" / "实现细节略"
- [x] 文件路径全是绝对/相对精确
- [x] TDD 节奏体现（写测试 → 跑失败 → 实现 → 跑通）
- [x] 每任务都 commit
- [x] 命名一致：archive/unarchive/archived × card/channel — 类型/函数/HTTP/CLI 对应关系清晰
- [x] 权限/rollback/push-retry 等关键不变量在 Task 1 明确列出

---

## Execution handoff

按 SOP 流程使用 **Subagent-Driven Development**：dispatcher 按 Task 1→7 顺序 dispatch implementer subagent，每个 task 两轮 review（spec → code quality），完成后 Phase 6 跑 Claude + Codex 双评审。
