# Channel Archive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 支持频道归档 — 归档的频道移到 `archive/channels/` 目录，禁止所有写操作（send/join/leave），仍可读取。

**Architecture:** 归档操作通过 `git mv` 将 `.thread` 和 `.meta.yaml` 从 `channels/` 移到 `archive/channels/`，目录即状态。写入拦截在各 handler 中添加 archive 目录检查。CLI 提供 `archive-channel` 和 `archived-channels` 两个新命令。

**Tech Stack:** Rust (daemon), TypeScript (CLI)

---

### Task 1: GitStorage 添加 mv 方法

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`

**变更内容:**
- 在 `GitStorage` 上新增 `mv(&self, from: &str, to: &str) -> Result<(), GitError>` 方法
- 执行 `git mv <from> <to>`，失败时返回 `GitError::CommandFailed`

**验收标准:**
- [ ] `git mv` 封装为 `GitStorage::mv` 方法
- [ ] 错误处理与现有方法（如 `add_and_commit`）风格一致

---

### Task 2: API 层添加 ArchiveChannel 和 ListArchivedChannels 请求

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`

**变更内容:**
- `Request` enum 新增 `ArchiveChannel { channel: String, author: Option<String> }`，method 名 `"archive_channel"`
- `Request` enum 新增 `ListArchivedChannels`，method 名 `"archived_channels"`

**验收标准:**
- [ ] 两个新 variant 可从 JSON 反序列化
- [ ] 字段命名与现有 Request（如 `CreateChannel`、`ListChannels`）风格一致

---

### Task 3: handle_request 路由新请求

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`（`handle_request` 函数）

**变更内容:**
- 在 `handle_request` match 中添加 `ArchiveChannel` 和 `ListArchivedChannels` 的路由
- `ArchiveChannel` 需要 `resolve_author`，参照 `CreateChannel` 的模式

**验收标准:**
- [ ] 新请求路由到对应的 handler 函数
- [ ] author 解析逻辑与其他带 author 的请求一致

---

### Task 4: handle_archive_channel 核心逻辑

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

**变更内容:**
- 新增 `handle_archive_channel` 异步函数
- 校验流程：
  1. 验证 channel name 合法
  2. 验证 author 已注册
  3. 读取 `channels/<name>.meta.yaml`，确认频道存在
  4. 检查 `meta.created_by == author`，否则返回权限错误
  5. 创建 `archive/channels/` 目录（如不存在）
  6. `git mv` 两个文件（.thread + .meta.yaml）到 `archive/channels/`
  7. `git add + commit`，commit message: `archive: #<channel> by @<author>`
  8. push with retry（参照 `handle_create_channel` 的 push 模式）
- 从 thread_cache 中移除该频道的缓存条目

**验收标准:**
- [ ] 仅 `created_by` 可以归档
- [ ] 归档后 `channels/` 下文件消失，`archive/channels/` 下出现
- [ ] git log 中有对应的 archive commit
- [ ] 频道不存在 → 明确错误
- [ ] 已归档的频道再归档 → 明确错误（channels/ 下找不到）

---

### Task 5: handle_list_archived_channels

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

**变更内容:**
- 新增 `handle_list_archived_channels` 异步函数
- 扫描 `archive/channels/*.meta.yaml`，提取频道名和 members
- 返回格式与 `handle_list_channels` 一致，`kind` 字段为 `"archived_channel"`

**验收标准:**
- [ ] 返回所有已归档频道的列表
- [ ] `archive/channels/` 不存在时返回空列表
- [ ] 返回的 JSON 结构与 `handle_list_channels` 的 channel 条目格式一致

---

### Task 6: 写入拦截 — handle_send 中的 archive 检查

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`（`handle_send` 函数）

**变更内容:**
- 在 `handle_send` 的 channel 分支中（非 DM），当 `channels/<name>.meta.yaml` 不存在时，额外检查 `archive/channels/<name>.meta.yaml` 是否存在
- 如果存在，返回 `"channel '<name>' is archived"` 错误（而非当前的 `"does not exist"`）

**验收标准:**
- [ ] 向已归档频道发消息 → 返回 "is archived" 而非 "does not exist"
- [ ] 向不存在的频道发消息 → 行为不变（"does not exist"）
- [ ] DM 逻辑不受影响

---

### Task 7: 写入拦截 — write_channel_event 中的 archive 检查

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`（`write_channel_event` 函数）

**变更内容:**
- 在 `write_channel_event` 读取 `channels/<name>.meta.yaml` 的分支中，当文件不存在时检查 archive 目录
- 存在则返回 `"channel '<name>' is archived"` 错误

**验收标准:**
- [ ] join 已归档频道 → "is archived"
- [ ] leave 已归档频道 → "is archived"

---

### Task 8: 读取支持 — handle_read 兼容归档频道

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`（`resolve_thread_path` 函数 或 `handle_read` 函数）

**变更内容:**
- 当 `channels/<name>.thread` 不存在时，尝试 `archive/channels/<name>.thread`
- 返回结果中附加 `"archived": true` 标记

**验收标准:**
- [ ] `read` 已归档频道 → 正常返回消息，附带 `archived: true`
- [ ] `read` 普通频道 → 行为不变
- [ ] `read` 不存在的频道 → 行为不变

---

### Task 9: CLI — client 添加 archiveChannel 和 listArchivedChannels 方法

**Files:**
- Modify: `cli/src/client.ts`

**变更内容:**
- `GitimClient` 新增 `archiveChannel(channel: string): Promise<ApiResponse>` 方法
- `GitimClient` 新增 `listArchivedChannels(): Promise<ApiResponse>` 方法
- 参照现有方法（`createChannel`、`listChannels`）的模式

**验收标准:**
- [ ] 两个方法发送正确的 JSON 请求到 daemon socket

---

### Task 10: CLI — archive-channel 命令

**Files:**
- Create: `cli/src/commands/archive-channel.ts`

**变更内容:**
- 新增 `archiveChannelCommand(name: string)` 函数
- 模式参照 `create-channel.ts`：findRepoRoot → ensureDaemon → client.archiveChannel → 输出结果

**验收标准:**
- [ ] `gitim archive-channel <name>` 调用 daemon 的 archive_channel 方法
- [ ] 成功时输出确认信息，失败时输出错误并 exit(1)

---

### Task 11: CLI — archived-channels 命令

**Files:**
- Create: `cli/src/commands/archived-channels.ts`

**变更内容:**
- 新增 `archivedChannelsCommand()` 函数
- 参照 `channels.ts` 的模式

**验收标准:**
- [ ] `gitim archived-channels` 列出所有已归档频道
- [ ] 无归档频道时输出空列表或提示

---

### Task 12: CLI — 注册新命令

**Files:**
- Modify: `cli/src/index.ts`

**变更内容:**
- import 两个新 command
- 注册 `archive-channel <name>` 命令
- 注册 `archived-channels` 命令

**验收标准:**
- [ ] `gitim --help` 显示两个新命令
- [ ] 命令可正常调用

---

## 依赖关系

```
Task 1 (git mv) ──┐
Task 2 (API)   ───┤
                   ├─→ Task 3 (路由) ─→ Task 4 (archive handler) ─→ Task 6 (send 拦截)
                   │                  ─→ Task 5 (list archived)     Task 7 (event 拦截)
                   │                                                 Task 8 (read 支持)
                   │
Task 9 (client) ───┼─→ Task 10 (archive cmd)
                   ├─→ Task 11 (archived cmd)
                   └─→ Task 12 (register)
```

Task 1/2/9 可并行。Task 4 依赖 1+2+3。Task 6/7/8 依赖 4。Task 10/11/12 依赖 9。
