# Send Membership Validation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Send 操作验证发送者是否为目标频道/DM 的成员，堵住"非成员可写入"的安全漏洞。

**Architecture:** 在 `validate_append()` 增加 `allowed_senders` 参数，handler 层根据 channel/DM 类型计算允许的发送者列表传入。空列表 = open channel 不检查。

**Tech Stack:** Rust (gitim-core validator + gitim-daemon handlers)

---

## 设计决策（已确认）

| 决策 | 结论 |
|------|------|
| 检查位置 | `validate_append()` 改签名，加 `allowed_senders: &[&str]` |
| 空列表语义 | `&[]` = open channel，不检查 membership |
| Channel/DM 判断 | `channel.starts_with("dm:")` = DM，否则 = channel |
| Channel 成员来源 | 读 `channels/<name>.meta.json` → `meta.members` |
| DM 参与者来源 | 从 channel 参数 `dm:handler1,handler2` 解析两个参与者 |
| 错误类型 | `ComplianceError::NotMember(String)` |
| 测试覆盖 | 非成员发送失败 + 成员发送成功两个场景都覆盖 |

---

## Chunk 1: Core Validator 改造

### Task 1: `ComplianceError` 新增 `NotMember` 变体 + `validate_append` 签名改造

**Files:**
- Modify: `crates/gitim-core/src/validator/compliance.rs`

- [ ] **Step 1:** 在 `ComplianceError` 枚举中新增 `NotMember(String)` 变体，错误消息格式：`"author '@{0}' is not a member of this channel"`

- [ ] **Step 2:** `validate_append` 函数签名新增第 4 个参数 `allowed_senders: &[&str]`

- [ ] **Step 3:** 在 author 注册检查（`UnknownAuthor`）之后，加入 membership 检查：如果 `allowed_senders` 非空且 author 不在列表中，返回 `NotMember` 错误

- [ ] **Step 4:** 运行 `cargo test -p gitim-core`，预期编译失败（现有测试调用缺少第 4 参数）

- [ ] **Step 5:** Commit: `feat(core): add allowed_senders param to validate_append`

### Task 2: 更新现有 compliance 测试 + 新增 membership 测试

**Files:**
- Modify: `crates/gitim-core/tests/compliance_test.rs`

- [ ] **Step 1:** 所有现有的 `validate_append` 调用增加第 4 参数 `&[]`（open channel 语义，保持原有行为不变）

- [ ] **Step 2:** 运行 `cargo test -p gitim-core`，所有现有测试应通过

- [ ] **Step 3:** 新增测试 `test_append_non_member_rejected` — author 不在 `allowed_senders` 列表中，send 被拒绝，错误消息包含 author 名

- [ ] **Step 4:** 新增测试 `test_append_member_allowed` — author 在 `allowed_senders` 列表中，send 成功

- [ ] **Step 5:** 新增测试 `test_append_open_channel_allowed` — `allowed_senders` 为空，任何已注册用户都能发送（和现有行为一致，但显式测试）

- [ ] **Step 6:** 运行 `cargo test -p gitim-core`，全部通过

- [ ] **Step 7:** Commit: `test(core): add membership validation tests for validate_append`

---

## Chunk 2: Handler 层接入

### Task 3: `handle_send` 计算 `allowed_senders` 并传入 `validate_append`

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1:** 在 `handle_send` 中，`resolve_thread_path` 之后、`validate_append` 调用之前，加入 `allowed_senders` 计算逻辑：
  - 如果 `channel.starts_with("dm:")` → 从 channel 参数解析出两个参与者（`channel[3..].split(',')` 得到两个 handler），作为 `allowed_senders`
  - 否则（普通 channel）→ 读 `channels/<channel>.meta.json`，解析为 `ChannelMeta`，取 `meta.members` 作为 `allowed_senders`。如果文件不存在或 members 为空，传 `&[]`

- [ ] **Step 2:** 更新 `validate_append` 调用，传入计算好的 `allowed_senders`

- [ ] **Step 3:** 运行 `cargo test -p gitim-daemon`，预期部分测试失败（`test_poll_filters_non_member_channels` 中 alice 发 random 现在会被拒绝）

- [ ] **Step 4:** Commit: `feat(daemon): enforce membership check in handle_send`

### Task 4: 修复 + 扩展 handler 测试

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`（tests 模块）

- [ ] **Step 1:** 修改 `test_poll_filters_non_member_channels` — Alice 发送到非成员频道 random 应该**失败**，断言 `send_random.ok == false` 且错误消息包含 "not a member"

- [ ] **Step 2:** 新增测试 `test_send_member_channel_succeeds` — 用户先 join channel，再 send，验证 send 成功

- [ ] **Step 3:** 新增测试 `test_send_non_member_channel_rejected` — 用户未 join channel（channel 有其他成员，非 open），send 被拒绝

- [ ] **Step 4:** 新增测试 `test_send_open_channel_succeeds` — channel members 为空（open channel），任何注册用户 send 成功

- [ ] **Step 5:** 新增测试 `test_send_dm_participant_succeeds` — DM 参与者可以发消息

- [ ] **Step 6:** 新增测试 `test_send_dm_non_participant_rejected` — 非参与者向别人的 DM 发消息被拒绝

- [ ] **Step 7:** 运行 `cargo test -p gitim-daemon`，全部通过

- [ ] **Step 8:** Commit: `test(daemon): add send membership + DM validation tests`

