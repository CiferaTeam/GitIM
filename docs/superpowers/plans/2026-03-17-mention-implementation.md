# Mention 协议扩展实现计划

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 GitIM 消息格式增加 `<@handler>` 协议级 mention 能力，含解析、写入验证、读取检测。

**Architecture:** 在 `gitim-core` crate 中新增 mention 提取函数，扩展 `Message` 结构体增加 `mentions` 字段，在 parser 组装 body 后调用提取。写入验证和读取检测各自新增 mention 存在性检查。

**Tech Stack:** Rust, regex crate

**Spec:** `docs/superpowers/specs/2026-03-17-gitim-mention-design.md`

**Worktree:** `.worktrees/mention` (branch: `feature/mention`)

---

## 文件变更清单

| 操作 | 文件 | 职责 |
|------|------|------|
| 新建 | `crates/gitim-core/src/mention.rs` | mention 提取函数：从 body 文本中提取 `Vec<Handler>` |
| 新建 | `crates/gitim-core/tests/mention_test.rs` | mention 提取的单元测试 |
| 修改 | `crates/gitim-core/src/lib.rs` | 新增 `pub mod mention;` |
| 修改 | `crates/gitim-core/src/types/message.rs` | `Message` 增加 `mentions: Vec<Handler>` 字段 |
| 修改 | `crates/gitim-core/src/parser.rs` | 组装 body 后调用 mention 提取 |
| 修改 | `crates/gitim-core/tests/parser_test.rs` | 更新现有测试以覆盖 mentions 字段 |
| 修改 | `crates/gitim-core/src/validator/compliance.rs` | 新增 `UnknownMention` 错误和检查逻辑 |
| 修改 | `crates/gitim-core/tests/compliance_test.rs` | 新增 mention 验证测试 |
| 修改 | `crates/gitim-core/src/validator/read_check.rs` | 新增 `UnknownMention` issue 类型和检测 |
| 修改 | `crates/gitim-core/tests/read_check_test.rs` | 新增 mention 读取检测测试 |

---

## Chunk 1: mention 提取模块

### Task 1: mention 提取函数与测试

**Files:**
- Create: `crates/gitim-core/src/mention.rs`
- Create: `crates/gitim-core/tests/mention_test.rs`
- Modify: `crates/gitim-core/src/lib.rs`

- [ ] **Step 1: 创建 mention.rs 提取函数**

```rust
// crates/gitim-core/src/mention.rs
use regex::Regex;
use std::sync::LazyLock;
use crate::types::Handler;

static MENTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<@([a-z0-9]([a-z0-9-]*[a-z0-9])?)>").unwrap()
});

/// 从消息 body 中提取协议级 mention，去重，按首次出现顺序返回。
/// Handler::new() 验证失败的匹配（如连续连字符、保留字）静默忽略。
pub fn extract_mentions(body: &str) -> Vec<Handler> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for caps in MENTION_RE.captures_iter(body) {
        let raw = &caps[1];
        if seen.contains(raw) {
            continue;
        }
        if let Ok(handler) = Handler::new(raw) {
            seen.insert(raw.to_string());
            result.push(handler);
        }
    }
    result
}
```

- [ ] **Step 2: 注册模块**

在 `crates/gitim-core/src/lib.rs` 末尾添加：

```rust
pub mod mention;
```

- [ ] **Step 3: 编写 mention 提取测试**

```rust
// crates/gitim-core/tests/mention_test.rs
use gitim_core::mention::extract_mentions;

#[test]
fn test_single_mention() {
    let mentions = extract_mentions("<@lewis> 请看一下");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "lewis");
}

#[test]
fn test_multiple_mentions() {
    let mentions = extract_mentions("<@lewis> 和 <@nexus> 讨论一下");
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].as_str(), "lewis");
    assert_eq!(mentions[1].as_str(), "nexus");
}

#[test]
fn test_mention_in_continuation() {
    let mentions = extract_mentions("第一行内容\n第二行 <@coder> 看看");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "coder");
}

#[test]
fn test_mention_with_hyphen() {
    let mentions = extract_mentions("请 <@cifera-nexus> 确认");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "cifera-nexus");
}

#[test]
fn test_duplicate_mention_dedup() {
    let mentions = extract_mentions("<@lewis> 和 <@lewis> 重复了");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "lewis");
}

#[test]
fn test_bare_at_ignored() {
    let mentions = extract_mentions("@lewis 不是协议级 mention");
    assert!(mentions.is_empty());
}

#[test]
fn test_empty_handler_ignored() {
    let mentions = extract_mentions("<@> 空的");
    assert!(mentions.is_empty());
}

#[test]
fn test_uppercase_ignored() {
    let mentions = extract_mentions("<@LEWIS> 大写");
    assert!(mentions.is_empty());
}

#[test]
fn test_system_reserved_ignored() {
    let mentions = extract_mentions("<@system> 保留字");
    assert!(mentions.is_empty());
}

#[test]
fn test_consecutive_hyphens_ignored() {
    let mentions = extract_mentions("<@foo--bar> 连续连字符");
    assert!(mentions.is_empty());
}

#[test]
fn test_unclosed_mention_ignored() {
    let mentions = extract_mentions("<@lewis 未闭合");
    assert!(mentions.is_empty());
}

#[test]
fn test_nested_mention() {
    let mentions = extract_mentions("<@<@lewis>> 嵌套");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "lewis");
}

#[test]
fn test_no_mentions() {
    let mentions = extract_mentions("普通消息，没有 mention");
    assert!(mentions.is_empty());
}

#[test]
fn test_mention_at_line_boundaries() {
    let mentions = extract_mentions("<@alice>\n<@bob>");
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].as_str(), "alice");
    assert_eq!(mentions[1].as_str(), "bob");
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/mention && cargo test --test mention_test`
Expected: 14 tests PASS

- [ ] **Step 5: 提交**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/mention
git add crates/gitim-core/src/mention.rs crates/gitim-core/src/lib.rs crates/gitim-core/tests/mention_test.rs
git commit -m "feat(core): add mention extraction from message body"
```

---

## Chunk 2: Message 结构体扩展与 parser 集成

### Task 2: 扩展 Message 并集成到 parser

**Files:**
- Modify: `crates/gitim-core/src/types/message.rs`
- Modify: `crates/gitim-core/src/parser.rs`
- Modify: `crates/gitim-core/tests/parser_test.rs`

- [ ] **Step 1: 扩展 Message 结构体**

在 `crates/gitim-core/src/types/message.rs` 中，给 `Message` 增加 `mentions` 字段：

```rust
use crate::types::handler::Handler;

/// A parsed message from a .thread file.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub line_number: u64,
    pub point_to: u64,
    pub author: Handler,
    pub timestamp: String,
    pub body: String,
    pub mentions: Vec<Handler>,
}
```

`ThreadLine` 和 `ThreadFile` 不变。

- [ ] **Step 2: 更新 parser.rs 中的 Message 构造**

在 `parser.rs` 中，消息创建时初始化 `mentions: Vec::new()`，body 最终赋值后调用 `extract_mentions`。

修改 `parse_thread` 函数：

1. 在 `messages.push(Message { ... })` 处添加 `mentions: Vec::new()`

2. 在两处 body 赋值完成后提取 mentions。将现有的 body 赋值逻辑改为提取 mentions 的辅助函数：

```rust
use crate::mention::extract_mentions;
```

在 `if let (Some(body), Some(msg)) = (current_body.take(), messages.last_mut())` 这两处（循环内和循环后），body 赋值后追加：

```rust
msg.body = body;
msg.mentions = extract_mentions(&msg.body);
```

完整修改后的 `parse_thread`：

```rust
pub fn parse_thread(input: &str) -> Result<ThreadFile, ParseError> {
    let input = &input.replace("\r\n", "\n");
    if input.is_empty() {
        return Ok(ThreadFile { messages: vec![] });
    }

    let mut messages: Vec<Message> = Vec::new();
    let mut current_body: Option<String> = None;
    let mut first_content_line = true;

    for (file_line_idx, line) in input.lines().enumerate() {
        if let Some(caps) = MSG_RE.captures(line) {
            if let (Some(body), Some(msg)) = (current_body.take(), messages.last_mut()) {
                msg.body = body;
                msg.mentions = extract_mentions(&msg.body);
            }

            let line_number: u64 = caps[1].parse().unwrap();
            let point_to: u64 = caps[2].parse().unwrap();
            let author = Handler::new(&caps[3]).map_err(|e| ParseError::InvalidHandler {
                line: file_line_idx + 1,
                source: e,
            })?;
            let timestamp = caps[4].to_string();
            let body_first_line = caps[5].to_string();

            messages.push(Message {
                line_number,
                point_to,
                author,
                timestamp,
                body: String::new(),
                mentions: Vec::new(),
            });
            current_body = Some(body_first_line);
            first_content_line = false;
        } else {
            if first_content_line {
                return Err(ParseError::FirstLineNotMessage(file_line_idx + 1));
            }
            if let Some(ref mut body) = current_body {
                let content = if line.starts_with(" [L") {
                    &line[1..]
                } else {
                    line
                };
                body.push('\n');
                body.push_str(content);
            }
        }
    }

    if let (Some(body), Some(msg)) = (current_body, messages.last_mut()) {
        msg.body = body;
        msg.mentions = extract_mentions(&msg.body);
    }

    Ok(ThreadFile { messages })
}
```

- [ ] **Step 3: 更新现有 parser 测试**

现有 parser 测试中的断言不会断裂（它们不检查 mentions 字段）。但需要新增 parser 层面的 mention 集成测试，在 `crates/gitim-core/tests/parser_test.rs` 末尾追加：

```rust
#[test]
fn test_parse_extracts_mentions_from_body() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey <@lewis> check this\n";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages[0].mentions.len(), 1);
    assert_eq!(result.messages[0].mentions[0].as_str(), "lewis");
}

#[test]
fn test_parse_extracts_mentions_from_continuation() {
    let input = "\
[L000001][P000000][@nexus][20250316T120000Z] first line
need <@coder> to review
";
    let result = parse_thread(input).unwrap();
    assert_eq!(result.messages[0].mentions.len(), 1);
    assert_eq!(result.messages[0].mentions[0].as_str(), "coder");
}

#[test]
fn test_parse_no_mentions() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] plain message\n";
    let result = parse_thread(input).unwrap();
    assert!(result.messages[0].mentions.is_empty());
}

#[test]
fn test_parse_bare_at_not_extracted() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] cc @lewis 看看\n";
    let result = parse_thread(input).unwrap();
    assert!(result.messages[0].mentions.is_empty());
}
```

- [ ] **Step 4: 运行全部 parser 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/mention && cargo test --test parser_test --test mention_test`
Expected: 所有测试 PASS（原有 8 个 + 新增 4 个 parser + 14 个 mention）

- [ ] **Step 5: 提交**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/mention
git add crates/gitim-core/src/types/message.rs crates/gitim-core/src/parser.rs crates/gitim-core/tests/parser_test.rs
git commit -m "feat(core): integrate mention extraction into parser and Message struct"
```

---

## Chunk 3: 写入验证与读取检测

### Task 3: 写入验证新增 mention 检查

**Files:**
- Modify: `crates/gitim-core/src/validator/compliance.rs`
- Modify: `crates/gitim-core/tests/compliance_test.rs`

- [ ] **Step 1: 新增 ComplianceError 变体和检查逻辑**

在 `crates/gitim-core/src/validator/compliance.rs` 中：

1. 在 `ComplianceError` 枚举中新增：

```rust
    #[error("unknown mention '<@{handler}>' in message L{line_number:06}")]
    UnknownMention { handler: String, line_number: u64 },
```

2. 在 `validate_append` 函数的 `for msg in &new_file.messages` 循环中，在 `known_lines.insert` 之前添加：

```rust
        for mention in &msg.mentions {
            if !user_set.contains(mention.as_str()) {
                return Err(ComplianceError::UnknownMention {
                    handler: mention.to_string(),
                    line_number: msg.line_number,
                });
            }
        }
```

- [ ] **Step 2: 编写 mention 验证测试**

在 `crates/gitim-core/tests/compliance_test.rs` 末尾追加：

```rust
#[test]
fn test_append_with_valid_mention() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] hey <@lewis> check this\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_with_unknown_mention_rejected() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] hey <@ghost> check this\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("ghost"));
}

#[test]
fn test_append_bare_at_not_validated() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] cc @ghost 不验证\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_ok());
}

#[test]
fn test_append_mention_in_continuation() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] first line\ncc <@unknown> 看看\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
}

#[test]
fn test_append_multiple_mentions_one_unknown() {
    let existing = make_existing();
    let new_lines = "[L000003][P000001][@nexus][20250316T121000Z] hey <@lewis> and <@ghost>\n";
    let users = vec!["nexus", "lewis"];
    let result = validate_append(existing, new_lines, &users);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("ghost"));
}
```

- [ ] **Step 3: 运行测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/mention && cargo test --test compliance_test`
Expected: 所有测试 PASS（原有 6 个 + 新增 4 个）

- [ ] **Step 4: 提交**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/mention
git add crates/gitim-core/src/validator/compliance.rs crates/gitim-core/tests/compliance_test.rs
git commit -m "feat(core): add mention validation to write-path compliance check"
```

### Task 4: 读取检测新增 mention 告警

**Files:**
- Modify: `crates/gitim-core/src/validator/read_check.rs`
- Modify: `crates/gitim-core/tests/read_check_test.rs`

- [ ] **Step 1: 新增 IntegrityIssue 变体和检测逻辑**

在 `crates/gitim-core/src/validator/read_check.rs` 中：

1. 在 `IntegrityIssue` 枚举中新增：

```rust
    UnknownMention { handler: String, line_number: u64 },
```

2. 在 `check_thread_integrity` 函数的 `for msg in &file.messages` 循环中，在循环末尾（`expected_next` 更新之前）添加：

```rust
        for mention in &msg.mentions {
            if !user_set.contains(mention.as_str()) {
                issues.push(IntegrityIssue::UnknownMention {
                    handler: mention.to_string(),
                    line_number: msg.line_number,
                });
            }
        }
```

- [ ] **Step 2: 编写读取检测测试**

在 `crates/gitim-core/tests/read_check_test.rs` 末尾追加：

```rust
#[test]
fn test_detect_unknown_mention() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey <@ghost>\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.iter().any(|i| matches!(i, IntegrityIssue::UnknownMention { handler, .. } if handler == "ghost")));
}

#[test]
fn test_valid_mention_no_issue() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey <@lewis>\n";
    let users = vec!["nexus", "lewis"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.is_empty());
}

#[test]
fn test_bare_at_no_issue() {
    let input = "[L000001][P000000][@nexus][20250316T120000Z] hey @ghost\n";
    let users = vec!["nexus"];
    let issues = check_thread_integrity(input, &users);
    assert!(issues.is_empty());
}
```

- [ ] **Step 3: 运行测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/mention && cargo test --test read_check_test`
Expected: 所有测试 PASS（原有 4 个 + 新增 3 个）

- [ ] **Step 4: 运行全量测试确认无回归**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/mention && cargo test`
Expected: 所有 crate 全部测试 PASS

- [ ] **Step 5: 提交**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/mention
git add crates/gitim-core/src/validator/read_check.rs crates/gitim-core/tests/read_check_test.rs
git commit -m "feat(core): add mention detection to read-path integrity check"
```
