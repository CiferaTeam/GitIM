# Mention 协议扩展实现计划

**状态：已完成**

**Goal:** 为 GitIM 消息格式增加 `<@handler>` 协议级 mention 能力，含解析、写入验证、读取检测。

**Architecture:** 在 `gitim-core` crate 中新增 mention 提取函数，扩展 `Message` 结构体增加 `mentions` 字段，在 parser 组装 body 后调用提取。写入验证和读取检测各自新增 mention 存在性检查。

**Tech Stack:** Rust, regex crate

**Spec:** `docs/superpowers/specs/2026-03-17-gitim-mention-design.md`

---

## 文件变更清单

| 操作 | 文件 | 职责 |
|------|------|------|
| 新建 | `crates/gitim-core/src/mention.rs` | mention 提取函数：从 body 文本中提取 `Vec<Handler>` |
| 新建 | `crates/gitim-core/tests/mention_test.rs` | mention 提取的单元测试（14 个） |
| 修改 | `crates/gitim-core/src/lib.rs` | 新增 `pub mod mention;` |
| 修改 | `crates/gitim-core/src/types/message.rs` | `Message` 增加 `mentions: Vec<Handler>` 字段 |
| 修改 | `crates/gitim-core/src/parser.rs` | 组装 body 后调用 mention 提取 |
| 修改 | `crates/gitim-core/tests/parser_test.rs` | 新增 4 个 mention 集成测试 |
| 修改 | `crates/gitim-core/src/validator/compliance.rs` | 新增 `UnknownMention` 错误和检查逻辑 |
| 修改 | `crates/gitim-core/tests/compliance_test.rs` | 新增 5 个 mention 验证测试 |
| 修改 | `crates/gitim-core/src/validator/read_check.rs` | 新增 `UnknownMention` issue 类型和检测 |
| 修改 | `crates/gitim-core/tests/read_check_test.rs` | 新增 3 个 mention 读取检测测试 |

---

## Chunk 1: mention 提取模块

### Task 1: mention 提取函数与测试

**Files:** `mention.rs`, `mention_test.rs`, `lib.rs`

- [x] 创建 `mention.rs`，实现 `extract_mentions(body) -> Vec<Handler>`，使用 `LazyLock<Regex>` 提取 `<@handler>` 模式，去重，`Handler::new()` 验证失败的静默忽略
- [x] 注册模块到 `lib.rs`
- [x] 编写 14 个测试覆盖：单/多 mention、续行、含连字符 handler、去重、裸 @、空 handler、大写、保留字、连续连字符、未闭合、嵌套、无 mention、行边界
- [x] Commit: `feat(core): add mention extraction from message body`

---

## Chunk 2: Message 结构体扩展与 parser 集成

### Task 2: 扩展 Message 并集成到 parser

**Files:** `message.rs`, `parser.rs`, `parser_test.rs`

- [x] `Message` 增加 `mentions: Vec<Handler>` 字段
- [x] `parse_thread` 中 body 赋值完成后调用 `extract_mentions`（两处：循环内和循环后）
- [x] 新增 4 个 parser 层面的 mention 集成测试：body 中提取、续行中提取、无 mention、裸 @ 不提取
- [x] Commit: `feat(core): integrate mention extraction into parser and Message struct`

---

## Chunk 3: 写入验证与读取检测

### Task 3: 写入验证新增 mention 检查

**Files:** `compliance.rs`, `compliance_test.rs`

- [x] `ComplianceError` 新增 `UnknownMention { handler, line_number }` 变体
- [x] `validate_append` 循环中检查每条消息的 mentions 是否在 registered_users 中
- [x] 新增 5 个测试：合法 mention、未知 mention 被拒、裸 @ 不验证、续行中的 mention、多 mention 其一未知
- [x] Commit: `feat(core): add mention validation to write-path compliance check`

### Task 4: 读取检测新增 mention 告警

**Files:** `read_check.rs`, `read_check_test.rs`

- [x] `IntegrityIssue` 新增 `UnknownMention { handler, line_number }` 变体
- [x] `check_thread_integrity` 循环中检查 mentions
- [x] 新增 3 个测试：未知 mention 检出、合法 mention 无 issue、裸 @ 无 issue
- [x] Commit: `feat(core): add mention detection to read-path integrity check`
