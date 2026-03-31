# Channel Validation + CreateChannel Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 堵住 3 个 pre-existing 安全漏洞（path traversal、send 到不存在 channel、DM 到未注册用户），并新增 CreateChannel API 使 channel 生命周期完整。

**Architecture:** 新建 `ChannelName` 类型做 channel name 白名单校验（防 path traversal），`resolve_thread_path` 接入校验保护所有调用者。`handle_send` 对 channel 要求 meta.json 存在，对 DM 要求双方注册。新增 `create_channel` API 处理 channel 创建。

**Tech Stack:** Rust (gitim-core types + gitim-daemon handlers)

---

## 设计决策（已确认）

| 决策 | 结论 |
|------|------|
| Channel name 验证位置 | `resolve_thread_path` 里，channel 分支加 `ChannelName::new` |
| Channel name 规则 | `^[a-z0-9]+(-[a-z0-9]+)*$`，1-32 字符 |
| ChannelName 类型位置 | `gitim-core/src/types/channel.rs`，和 Handler 对称 |
| Send 要求 channel 存在 | 非 DM 时，meta.json 不存在 → 拒绝 |
| DM 参与者必须注册 | 在 `handle_send` 的 DM 分支检查 |
| CreateChannel API | name, display_name?, introduction?, author? |
| Creator 自动 join | members[0] + join event 写入 .thread |
| CreateChannel push 策略 | push-with-retry（复制 register_user 模式） |

---

## Chunk 1: ChannelName 类型 + resolve_thread_path 接入

### Task 1: 新建 `ChannelName` 类型

**Files:**
- Create: `crates/gitim-core/src/types/channel.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`（导出 ChannelName）

- [ ] **Step 1:** 创建 `channel.rs`，定义 `ChannelNameError` 枚举（Empty, TooLong, InvalidChar, HyphenBoundary, ConsecutiveHyphens）和 `ChannelName` newtype struct

- [ ] **Step 2:** 实现 `ChannelName::new(s: &str) -> Result<Self, ChannelNameError>`，规则：
  - 非空，最长 32 字符
  - 只允许 `a-z 0-9 -`
  - 不允许首尾 `-`
  - 不允许连续 `--`

- [ ] **Step 3:** 实现 `as_str(&self) -> &str`、`Display`、`Into<String>`

- [ ] **Step 4:** 在 `types/mod.rs` 中 `pub mod channel;` 并 re-export `ChannelName`

- [ ] **Step 5:** 写单元测试（在 channel.rs 的 `#[cfg(test)]` 模块）：合法名、空名、超长、非法字符（含 `/`、`..`）、首尾连字符、连续连字符

- [ ] **Step 6:** 运行 `cargo test -p gitim-core`，全部通过

- [ ] **Step 7:** Commit: `feat(core): add ChannelName type with validation`

### Task 2: `resolve_thread_path` 接入 ChannelName 校验

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1:** 在 `resolve_thread_path` 的 channel 分支（非 DM，L142-148），对 `channel` 调用 `ChannelName::new(&channel)`，失败则返回 `Response::error`

- [ ] **Step 2:** 新增测试 `test_send_invalid_channel_name_rejected` — 用 `../../etc/passwd` 作为 channel name 发送，断言失败

- [ ] **Step 3:** 新增测试 `test_read_invalid_channel_name_rejected` — 用非法 channel name 读取，断言失败

- [ ] **Step 4:** 运行 `cargo test -p gitim-daemon`，全部通过

- [ ] **Step 5:** Commit: `fix(daemon): validate channel names in resolve_thread_path`

---

## Chunk 2: Send 安全加固

### Task 3: Send 要求 channel meta.json 存在

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1:** 在 `handle_send` 的 channel 分支（非 DM）的 `allowed_senders` 计算逻辑中，将"meta.json 不存在 → 传 `&[]`"改为"meta.json 不存在 → 返回 `Response::error("channel '{}' does not exist")`"

- [ ] **Step 2:** 注意：meta.json 存在但 members 为空仍然传 `&[]`（open channel 语义不变）。只有文件不存在才拒绝。

- [ ] **Step 3:** 新增测试 `test_send_nonexistent_channel_rejected` — 不创建 channel，直接 send，断言失败且错误包含 "does not exist"

- [ ] **Step 4:** 检查现有测试是否因此断裂，修复（所有 send 测试的 channel 都应先通过 `create_test_channel` 创建）

- [ ] **Step 5:** 运行 `cargo test -p gitim-daemon`，全部通过

- [ ] **Step 6:** Commit: `fix(daemon): reject send to nonexistent channels`

### Task 4: DM 双方必须注册

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1:** 在 `handle_send` 的 DM 分支，计算 `allowed_senders` 时（从 `dm:a,b` 解析出两个参与者后），检查两个参与者是否都在 `user_list` 中。任一不在 → 返回 `Response::error("DM participant '@{}' is not a registered user")`

- [ ] **Step 2:** 新增测试 `test_send_dm_unregistered_participant_rejected` — alice 发送到 `dm:alice,ghost`（ghost 未注册），断言失败

- [ ] **Step 3:** 确认现有 DM 测试不受影响（现有测试的 DM 参与者都是注册用户）

- [ ] **Step 4:** 运行 `cargo test -p gitim-daemon`，全部通过

- [ ] **Step 5:** Commit: `fix(daemon): require DM participants to be registered`

---

## Chunk 3: CreateChannel API

### Task 5: 添加 CreateChannel Request 类型

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`

- [ ] **Step 1:** 在 `Request` 枚举中新增 `CreateChannel` 变体：
  - `name: String`
  - `display_name: Option<String>`（默认用 name）
  - `introduction: Option<String>`（默认空字符串）
  - `author: Option<String>`（默认 current_user）
  - serde rename: `"create_channel"`

- [ ] **Step 2:** Commit: `feat(api): add CreateChannel request type`

### Task 6: 实现 `handle_create_channel` + dispatch

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1:** 新增 `handle_create_channel` 函数，流程：
  1. resolve author（复用 `resolve_author`）
  2. 验证 author 是注册用户
  3. 验证 channel name（`ChannelName::new`）
  4. 检查 `channels/<name>.meta.json` 不存在（已存在 → 报错 "channel already exists"）
  5. 创建 `channels/` 目录（如果不存在）
  6. 写 `meta.json`：display_name, created_by, created_at, introduction, members=[author]
  7. 写 `.thread`：author 的 join event（L1）
  8. Commit：`channel: create #<name> by @<author>`
  9. Push-with-retry（复制 register_user 的模式：最多 3 次，冲突 → fetch + rebase → retry）
  10. 返回 success

- [ ] **Step 2:** 在 `handle_request` 的 match 中添加 `Request::CreateChannel` 分支，dispatch 到 `handle_create_channel`

- [ ] **Step 3:** 新增测试 `test_create_channel_basic` — 创建 channel，验证 meta.json 和 .thread 文件存在且内容正确

- [ ] **Step 4:** 新增测试 `test_create_channel_already_exists` — 创建同名 channel 两次，第二次应失败

- [ ] **Step 5:** 新增测试 `test_create_channel_invalid_name` — 用非法 channel name 创建，应失败

- [ ] **Step 6:** 新增测试 `test_create_channel_then_send` — 创建 channel 后 send 消息，验证完整流程

- [ ] **Step 7:** 运行 `cargo test -p gitim-daemon`，全部通过

- [ ] **Step 8:** Commit: `feat(daemon): implement create_channel handler`

### Task 7: CLI 添加 create-channel 命令

**Files:**
- Modify: `cli/src/client.ts`（添加 createChannel 方法）
- Modify: `cli/src/commands/`（添加 create-channel 命令）或 `cli/src/index.ts`

- [ ] **Step 1:** 在 `client.ts` 添加 `createChannel(name, displayName?, introduction?)` 方法

- [ ] **Step 2:** 在 CLI 入口添加 `create-channel <name>` 命令，支持 `--display-name` 和 `--introduction` 选项

- [ ] **Step 3:** 手动验证 CLI help 输出包含 create-channel 命令

- [ ] **Step 4:** Commit: `feat(cli): add create-channel command`
