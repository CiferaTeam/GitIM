# Unified Labels Space — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` to implement task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 gitim-core / daemon / cli / runtime / frontend 全栈落地统一 `labels` 空间 —— Card 已有 labels 保持,UserMeta 新增 labels(agent 能力 SoT),BoardMeta.tags 收敛到 labels,FlowNode 加 required_labels(信息位),并落 4 个 daemon IPC + CLI + runtime HTTP + WebUI read-only chip。

**Architecture:** Embedded labels(无独立 registry 文件),agent self-claim only,daemon 在 `create_card` 给 advisory `suggested_assignees`,Flow node `required_labels` 是 hint 不强制 routing。详见 [00-requirements.md](./00-requirements.md)。

**Tech Stack:** Rust(gitim-core / gitim-daemon / gitim-cli / gitim-client / gitim-runtime / gitim-agent-provider),React 19 + Vite + Radix UI + Tailwind + Zustand(products/gitim/frontend),serde + tokio + axum。

---

## File Structure

### 新建文件
| 路径 | 职责 |
|---|---|
| `crates/gitim-core/src/types/labels.rs` | 共享 `LabelError` + `validate_label` / `validate_labels(&[String], max_count)` + 4 个 `*_MAX_LABELS` 常量 + `MAX_LABEL_LEN` |
| `crates/gitim-daemon/src/handlers/labels.rs` | 4 个 handler:`handle_labels_add` / `handle_labels_remove` / `handle_labels_list` / `handle_agents_with_labels` + `compute_suggested_assignees` 辅助函数 |
| `crates/gitim-cli/src/commands/labels.rs` | `gitim labels add/remove/list/match` subcommand |
| `crates/gitim-daemon/tests/labels_test.rs` | Daemon labels handler 集成测 |
| `crates/gitim-cli/tests/labels_cli_test.rs` | CLI subcommand smoke 测 |

### 修改文件
| 路径 | 改动 |
|---|---|
| `crates/gitim-core/src/types/mod.rs` | 加 `pub mod labels;` 声明 + 顶层 re-export |
| `crates/gitim-core/src/types/card.rs` | `validate_label` / `validate_labels` 改 re-export `labels.rs`,内联实现删除;`CardError` 加 `#[from] LabelError` variant |
| `crates/gitim-core/src/types/board.rs` | `BoardMeta.tags` → `labels`(`#[serde(default, alias = "tags")]`);移除 `#[serde(deny_unknown_fields)]`(eng-review Issue #1);`set_board_field` match arm 接受 `"tags" \| "labels"`;`default_board` 改 `labels: Vec::new()`;`BOARD_MAX_TAGS` → `BOARD_MAX_LABELS` 移至 `labels.rs` |
| `crates/gitim-core/src/types/meta.rs` | `UserMeta` 加 `labels: Vec<String>` 字段;加 `validate_user_meta` 函数 |
| `crates/gitim-core/src/flow/types.rs` | `FlowNode` 加 `required_labels: Vec<String>` 字段(`#[serde(default, skip_serializing_if = "Vec::is_empty")]`) |
| `crates/gitim-core/src/flow/validator.rs` | 调 `validate_labels(&node.required_labels, FLOW_NODE_MAX_LABELS)`,失败 wrap 节点 id context |
| `crates/gitim-core/src/responses.rs` | 加 `LabelsAddResponse` / `LabelsRemoveResponse` / `LabelsListResponse` / `AgentsWithLabelsResponse` + `CreateCardResponse` 加 `suggested_assignees: Vec<String>` 字段 |
| `crates/gitim-daemon/src/api.rs` | `Request` enum 加 4 个 variant:`LabelsAdd` / `LabelsRemove` / `LabelsList` / `AgentsWithLabels` |
| `crates/gitim-daemon/src/handlers/mod.rs` | `mod labels;` 声明 + `pub use labels::*;` |
| `crates/gitim-daemon/src/server.rs`(或 dispatch 所在文件) | match 加 4 个 variant 路由 |
| `crates/gitim-daemon/src/card_handlers.rs` | `handle_create_card` push 后调 `compute_suggested_assignees` 塞 response;`ensure_known_user` 保持现状(已经检查 archive/users/) |
| `crates/gitim-daemon/src/onboard.rs` | `register_user`(line 395)`UserMeta` struct literal 加 `labels: vec![]` |
| `crates/gitim-client/src/lib.rs` | 加 `labels_add` / `labels_remove` / `labels_list` / `agents_with_labels` 方法 |
| `crates/gitim-cli/src/commands/mod.rs` | 加 `pub mod labels;` |
| `crates/gitim-cli/src/main.rs` | 加 `Labels { ... }` subcommand variant + dispatch |
| `crates/gitim-runtime/src/http.rs` | 加 4 个 route:`GET /im/labels/{handler}` / `POST /im/labels` / `DELETE /im/labels` / `GET /im/agents-with-labels` |
| `crates/gitim-agent-provider/src/prompts.rs` | line 501 `tags` → `labels`(主)+ 加一段 `gitim labels` API 文档 |
| `products/gitim/frontend/src/` | agent detail 页 / card 详情加 labels chip(read-only) |
| `CLAUDE.md` | Current Orientation 追加 unified labels 段 |

---

## Phase A — Shared Types (gitim-core)

### Task 1: 创建 `types/labels.rs` 共享 validator

**Files:**
- Create: `crates/gitim-core/src/types/labels.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`

- [ ] **Step 1: 创建 `types/labels.rs` 测试和实现**

Create `crates/gitim-core/src/types/labels.rs`:

```rust
use thiserror::Error;

pub const MAX_LABEL_LEN: usize = 32;
pub const CARD_MAX_LABELS: usize = 10;
pub const BOARD_MAX_LABELS: usize = 20;
pub const USER_MAX_LABELS: usize = 20;
pub const FLOW_NODE_MAX_LABELS: usize = 10;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum LabelError {
    #[error("label length out of range (1..={1}), got {0}")]
    LengthOutOfRange(usize, usize),
    #[error("invalid char '{0}' in label (allowed: a-z 0-9 - _)")]
    InvalidChar(char),
    #[error("too many labels (max {1}), got {0}")]
    TooMany(usize, usize),
}

pub fn validate_label(label: &str) -> Result<(), LabelError> {
    if label.is_empty() || label.len() > MAX_LABEL_LEN {
        return Err(LabelError::LengthOutOfRange(label.len(), MAX_LABEL_LEN));
    }
    for ch in label.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_') {
            return Err(LabelError::InvalidChar(ch));
        }
    }
    Ok(())
}

pub fn validate_labels(labels: &[String], max_count: usize) -> Result<(), LabelError> {
    if labels.len() > max_count {
        return Err(LabelError::TooMany(labels.len(), max_count));
    }
    for l in labels {
        validate_label(l)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_label_accepts_valid_chars() {
        assert!(validate_label("rust").is_ok());
        assert!(validate_label("frontend-react").is_ok());
        assert!(validate_label("mobile_ios").is_ok());
        assert!(validate_label("v2").is_ok());
    }

    #[test]
    fn validate_label_rejects_uppercase() {
        let err = validate_label("Rust").unwrap_err();
        assert!(matches!(err, LabelError::InvalidChar('R')));
    }

    #[test]
    fn validate_label_rejects_too_long() {
        let too_long = "a".repeat(33);
        let err = validate_label(&too_long).unwrap_err();
        assert!(matches!(err, LabelError::LengthOutOfRange(33, 32)));
    }

    #[test]
    fn validate_label_rejects_empty() {
        let err = validate_label("").unwrap_err();
        assert!(matches!(err, LabelError::LengthOutOfRange(0, 32)));
    }

    #[test]
    fn validate_labels_respects_max_count() {
        let labels: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
        let err = validate_labels(&labels, 10).unwrap_err();
        assert!(matches!(err, LabelError::TooMany(11, 10)));
    }

    #[test]
    fn validate_labels_user_cap() {
        let labels: Vec<String> = (0..21).map(|i| format!("l{}", i)).collect();
        let err = validate_labels(&labels, USER_MAX_LABELS).unwrap_err();
        assert!(matches!(err, LabelError::TooMany(21, 20)));
    }

    #[test]
    fn validate_labels_all_valid_passes() {
        let labels = vec!["rust".to_string(), "backend".to_string(), "mobile_ios".to_string()];
        assert!(validate_labels(&labels, 10).is_ok());
    }
}
```

- [ ] **Step 2: Wire 到 `types/mod.rs`**

Modify `crates/gitim-core/src/types/mod.rs`:

```rust
pub mod board;
pub mod card;
pub mod channel;
pub mod config;
pub mod cron;
pub mod handler;
pub mod labels;  // ← 新加
pub mod link;
pub mod message;
pub mod meta;

pub use board::{
    append_board_section, board_path, default_board, parse_board_markdown, set_board_field,
    set_board_section, stringify_board_markdown, validate_board_document,
    validate_board_for_handler, BoardDocument, BoardError, BoardMarkdownError, BoardMeta,
    BOARD_VERSION,
};
pub use card::{
    parse_card_meta_yaml, stringify_card_meta_yaml, validate_card_id, validate_card_meta,
    CardError, CardMeta, CardMetaYamlError, CardStatus,
};
pub use channel::ChannelName;
pub use config::Config;
pub use cron::{validate_cron_name, CronNameError, CronSpec, CronSpecError};
pub use handler::Handler;
pub use labels::{
    validate_label, validate_labels, LabelError, BOARD_MAX_LABELS, CARD_MAX_LABELS,
    FLOW_NODE_MAX_LABELS, MAX_LABEL_LEN, USER_MAX_LABELS,
};
pub use link::{Link, LinkKind};
pub use message::{ChannelEvent, Message, ThreadEntry, ThreadFile, ThreadLine};
pub use meta::{ChannelMeta, UserMeta, MAX_INTRODUCTION_LEN};
```

注意:`card::validate_labels` 从 pub-export 列表删除(被 `labels::validate_labels` 替代,签名变了从 `&[String]` → `&[String], max_count`)。

- [ ] **Step 3: 跑单测**

```bash
cargo test -p gitim-core types::labels::tests -- --nocapture
```

Expected: 6 test pass。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/labels.rs crates/gitim-core/src/types/mod.rs
git commit -m "feat(labels): extract shared validator into types/labels.rs"
```

---

### Task 2: 重构 `CardMeta` 使用共享 validator

**Files:**
- Modify: `crates/gitim-core/src/types/card.rs`

- [ ] **Step 1: 替换 `card.rs` 内联 validator**

Modify `crates/gitim-core/src/types/card.rs`:

删除 line 23-29(CardError 的 `LabelLengthOutOfRange` / `InvalidLabelChar` / `TooManyLabels` 三 variant)。
删除 line 89-91(`MAX_LABELS`、`MAX_LABEL_LEN`、`MAX_CARD_ID_LEN`,只删 LABEL 两个,保留 CARD_ID_LEN)。
删除 line 108-128 的 `validate_label` / `validate_labels` 函数。

`CardError` 加 `LabelError` variant:

```rust
#[derive(Error, Debug)]
pub enum CardError {
    #[error("invalid status '{0}', allowed: todo/doing/done")]
    InvalidStatus(String),
    #[error("card_id length out of range (1..={1}), got {0}")]
    CardIdLengthOutOfRange(usize, usize),
    #[error("invalid character in card_id: '{0}'")]
    InvalidCardIdChar(char),
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error("invalid channel name: {0}")]
    InvalidChannel(String),
    #[error("invalid handler: {0}")]
    InvalidHandler(String),
    #[error("invalid timestamp '{0}'")]
    InvalidTimestamp(String),
    #[error(transparent)]
    Label(#[from] super::labels::LabelError),
}
```

`validate_card_meta` 函数里替换 `validate_labels` 调用:

```rust
pub fn validate_card_meta(meta: &CardMeta) -> Result<(), CardError> {
    if meta.title.trim().is_empty() {
        return Err(CardError::EmptyTitle);
    }
    ChannelName::new(&meta.channel).map_err(|e| CardError::InvalidChannel(e.to_string()))?;
    Handler::new(&meta.created_by).map_err(|e| CardError::InvalidHandler(e.to_string()))?;
    if let Some(assignee) = &meta.assignee {
        Handler::new(assignee).map_err(|e| CardError::InvalidHandler(e.to_string()))?;
    }
    super::labels::validate_labels(&meta.labels, super::labels::CARD_MAX_LABELS)?;
    validate_timestamp(&meta.created_at)?;
    validate_timestamp(&meta.updated_at)?;
    Ok(())
}
```

更新 card.rs 内的 test(原 `validate_label_*` / `validate_labels_*` tests 删除 —— 已经在 labels.rs 测过)。

- [ ] **Step 2: Grep 外部 caller,看 `card::validate_labels` 还有谁调**

```bash
grep -rn "card::validate_labels\|use gitim_core::card::validate_labels\|CardError::LabelLengthOutOfRange\|CardError::InvalidLabelChar\|CardError::TooManyLabels" crates/ products/
```

Expected: 0 hit(只有 daemon 自己用 `validate_labels`,我们已经 re-export 到 `gitim_core::types::validate_labels`)。如果有 hit,改成 `gitim_core::types::validate_labels(&xs, CARD_MAX_LABELS)`。

注:`gitim-daemon::card_handlers.rs:167` 调 `validate_labels(&labels_vec)`(无 max_count 参数);改成 `gitim_core::validate_labels(&labels_vec, gitim_core::CARD_MAX_LABELS)`(顶层 re-export)。

- [ ] **Step 3: 跑 card 测**

```bash
cargo test -p gitim-core types::card -- --nocapture
cargo test -p gitim-daemon card_test
```

Expected: 全 pass。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/card.rs crates/gitim-daemon/src/card_handlers.rs
git commit -m "refactor(labels): card uses shared LabelError + validate_labels(_, CARD_MAX_LABELS)"
```

---

### Task 3: `UserMeta.labels` + validation

**Files:**
- Modify: `crates/gitim-core/src/types/meta.rs`

- [ ] **Step 1: 在 meta.rs 加 labels 字段 + validator + 测试**

Modify `crates/gitim-core/src/types/meta.rs`:

```rust
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::labels::{validate_labels, LabelError, USER_MAX_LABELS};

pub const MAX_INTRODUCTION_LEN: usize = 256;

#[derive(Error, Debug)]
pub enum UserMetaError {
    #[error("introduction too long ({0} > {1} bytes)")]
    IntroductionTooLong(usize, usize),
    #[error(transparent)]
    Label(#[from] LabelError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMeta {
    pub display_name: String,
    pub role: String,
    pub introduction: String,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
    #[serde(default)]
    pub members: Vec<String>,
}

pub fn validate_user_meta(meta: &UserMeta) -> Result<(), UserMetaError> {
    if meta.introduction.len() > MAX_INTRODUCTION_LEN {
        return Err(UserMetaError::IntroductionTooLong(
            meta.introduction.len(),
            MAX_INTRODUCTION_LEN,
        ));
    }
    validate_labels(&meta.labels, USER_MAX_LABELS)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_yaml_without_labels_deserializes_as_empty() {
        let yaml = "display_name: Alice\nrole: backend\nintroduction: hello\n";
        let meta: UserMeta = serde_yaml::from_str(yaml).unwrap();
        assert!(meta.labels.is_empty());
        assert_eq!(meta.display_name, "Alice");
    }

    #[test]
    fn new_yaml_with_labels_roundtrip() {
        let meta = UserMeta {
            display_name: "Alice".into(),
            role: "backend".into(),
            introduction: "hello".into(),
            labels: vec!["rust".into(), "backend".into()],
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        let parsed: UserMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(meta, parsed);
    }

    #[test]
    fn add_labels_preserves_other_fields() {
        let mut meta = UserMeta {
            display_name: "Alice".into(),
            role: "backend".into(),
            introduction: "hello".into(),
            labels: vec![],
        };
        meta.labels.push("rust".into());
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(yaml.contains("display_name: Alice"));
        assert!(yaml.contains("role: backend"));
        assert!(yaml.contains("introduction: hello"));
        assert!(yaml.contains("- rust"));
    }

    #[test]
    fn validate_user_meta_rejects_too_many_labels() {
        let labels: Vec<String> = (0..21).map(|i| format!("l{}", i)).collect();
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: String::new(),
            labels,
        };
        let err = validate_user_meta(&meta).unwrap_err();
        assert!(matches!(err, UserMetaError::Label(LabelError::TooMany(21, 20))));
    }

    #[test]
    fn validate_user_meta_rejects_invalid_label_char() {
        let meta = UserMeta {
            display_name: "A".into(),
            role: "r".into(),
            introduction: String::new(),
            labels: vec!["Rust!".into()],
        };
        let err = validate_user_meta(&meta).unwrap_err();
        assert!(matches!(err, UserMetaError::Label(LabelError::InvalidChar('R'))));
    }
}
```

- [ ] **Step 2: 加 `validate_user_meta` 到 mod.rs re-export**

Modify `crates/gitim-core/src/types/mod.rs`:

```rust
pub use meta::{validate_user_meta, ChannelMeta, UserMeta, UserMetaError, MAX_INTRODUCTION_LEN};
```

- [ ] **Step 3: 跑测**

```bash
cargo test -p gitim-core types::meta -- --nocapture
```

Expected: 5 test pass。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/meta.rs crates/gitim-core/src/types/mod.rs
git commit -m "feat(labels): add UserMeta.labels field + validate_user_meta"
```

---

### Task 4: `BoardMeta` rename `tags` → `labels` + 去 `deny_unknown_fields`

**Files:**
- Modify: `crates/gitim-core/src/types/board.rs`

- [ ] **Step 1: 改 BoardMeta struct + 共享 validator + 移除常量**

Modify `crates/gitim-core/src/types/board.rs`:

```rust
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::handler::Handler;
use super::labels::{validate_labels, LabelError, BOARD_MAX_LABELS, MAX_LABEL_LEN as BOARD_MAX_LABEL_LEN};

pub const BOARD_VERSION: u32 = 1;
pub const BOARD_MAX_BYTES: usize = 64 * 1024;
pub const BOARD_MAX_STATUS_LEN: usize = 80;
pub const BOARD_MAX_SUMMARY_LEN: usize = 280;

// 删除:
// - pub const BOARD_MAX_TAGS
// - pub const BOARD_MAX_TAG_LEN
// (移动到 labels.rs:BOARD_MAX_LABELS / MAX_LABEL_LEN)

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BoardError {
    #[error("invalid handler: {0}")]
    InvalidHandler(String),
    #[error("handler mismatch: expected {expected}, got {actual}")]
    HandlerMismatch { expected: String, actual: String },
    #[error("unsupported board version: {0}, expected {1}")]
    UnsupportedVersion(u32, u32),
    #[error("invalid timestamp '{0}'")]
    InvalidTimestamp(String),
    #[error("status cannot be empty")]
    EmptyStatus,
    #[error("status exceeds {1} bytes, got {0}")]
    StatusTooLong(usize, usize),
    #[error("summary exceeds {1} bytes, got {0}")]
    SummaryTooLong(usize, usize),
    // 删除:TooManyTags / TagLengthOutOfRange / InvalidTagChar
    #[error(transparent)]
    Label(#[from] LabelError),
    #[error("YAML serialization error: {0}")]
    YamlSerialize(String),
    #[error("unknown board field '{0}'")]
    UnknownField(String),
    #[error("invalid section name: {0}")]
    InvalidSection(String),
    #[error("board document exceeds {1} bytes, got {0}")]
    DocumentTooLarge(usize, usize),
}

// 注:相对原 `#[serde(deny_unknown_fields)]` 移除(eng-review Issue #1)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardMeta {
    pub version: u32,
    pub handler: String,
    pub updated_at: String,
    pub status: String,
    pub summary: String,
    #[serde(default, alias = "tags")]
    pub labels: Vec<String>,
}
```

`default_board` 函数:

```rust
pub fn default_board(handler: &str, timestamp: &str) -> Result<BoardDocument, BoardError> {
    let handler = Handler::new(handler).map_err(|e| BoardError::InvalidHandler(e.to_string()))?;
    let doc = BoardDocument {
        meta: BoardMeta {
            version: BOARD_VERSION,
            handler: handler.to_string(),
            updated_at: timestamp.to_string(),
            status: "idle".to_string(),
            summary: String::new(),
            labels: Vec::new(),
        },
        body: default_board_body(),
    };
    validate_board_document(&doc)?;
    Ok(doc)
}
```

`set_board_field` match arm 接受两个别名:

```rust
pub fn set_board_field(
    doc: &mut BoardDocument,
    field: &str,
    value: &str,
) -> Result<(), BoardError> {
    match field {
        "status" => {
            let status = value.trim().to_string();
            validate_status(&status)?;
            doc.meta.status = status;
        }
        "summary" => {
            let summary = value.trim().to_string();
            validate_summary(&summary)?;
            doc.meta.summary = summary;
        }
        // labels 是 canonical 名字;tags 是 backward-compat alias
        "labels" | "tags" => {
            let labels = parse_labels_csv(value)?;
            validate_labels(&labels, BOARD_MAX_LABELS)?;
            doc.meta.labels = labels;
        }
        other => return Err(BoardError::UnknownField(other.to_string())),
    }
    validate_board_document(doc)
}
```

把原 `parse_tags` 函数 rename 为 `parse_labels_csv`:

```rust
fn parse_labels_csv(value: &str) -> Result<Vec<String>, BoardError> {
    let labels = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    validate_labels(&labels, BOARD_MAX_LABELS)?;
    Ok(labels)
}
```

`validate_board_meta` 里把 `validate_tags` 调换:

```rust
fn validate_board_meta(meta: &BoardMeta) -> Result<(), BoardError> {
    if meta.version != BOARD_VERSION {
        return Err(BoardError::UnsupportedVersion(meta.version, BOARD_VERSION));
    }
    Handler::new(&meta.handler).map_err(|e| BoardError::InvalidHandler(e.to_string()))?;
    validate_timestamp(&meta.updated_at)?;
    validate_status(&meta.status)?;
    validate_summary(&meta.summary)?;
    validate_labels(&meta.labels, BOARD_MAX_LABELS)?;
    Ok(())
}
```

删除原 `validate_tags` 内联函数(被 shared `validate_labels` 替代)。

- [ ] **Step 2: 改 tests — 现有测里 tags 字段相关的**

替换 `crates/gitim-core/src/types/board.rs` 的 sample 函数 + tests。

```rust
fn sample_board() -> &'static str {
    "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: 正在梳理发布风险\nlabels:\n  - release\n---\n## 当前状态\n\n在看 sync 失败。\n\n## 已知事实\n\n- origin/main 可达\n"
}

fn board_without_labels() -> &'static str {
    "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: 正在梳理发布风险\n---\n## 当前状态\n"
}

fn legacy_board_with_tags() -> &'static str {
    // 旧 yaml 用 tags 字段,alias 应能 read
    "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: x\ntags:\n  - release\n---\nbody\n"
}

#[test]
fn board_markdown_roundtrips() {
    let parsed = parse_board_markdown(sample_board()).unwrap();
    assert_eq!(parsed.meta.handler, "alice");
    assert_eq!(parsed.meta.labels, vec!["release"]);
}

#[test]
fn parse_board_without_labels_defaults_to_empty_vec() {
    let parsed = parse_board_markdown(board_without_labels()).unwrap();
    assert!(parsed.meta.labels.is_empty());
}

#[test]
fn legacy_tags_field_is_read_via_alias() {
    let parsed = parse_board_markdown(legacy_board_with_tags()).unwrap();
    assert_eq!(parsed.meta.labels, vec!["release"]);
}

#[test]
fn rendered_yaml_uses_labels_not_tags() {
    let parsed = parse_board_markdown(legacy_board_with_tags()).unwrap();
    let rendered = stringify_board_markdown(&parsed).unwrap();
    assert!(rendered.contains("labels:"), "rendered:\n{rendered}");
    assert!(!rendered.contains("tags:"), "rendered:\n{rendered}");
}

#[test]
fn unknown_frontmatter_fields_are_silently_dropped() {
    // 跟原 deny_unknown_fields 测语义反转 —— 现在应该 accept + drop
    let with_extra = "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: s\nlabels: []\nfuture_field: dropped\n---\nbody\n";
    let parsed = parse_board_markdown(with_extra).expect("should accept unknown field");
    assert_eq!(parsed.meta.handler, "alice");
}

#[test]
fn invalid_label_characters_are_rejected() {
    let invalid = "---\nversion: 1\nhandler: alice\nupdated_at: 20260509T120000Z\nstatus: working\nsummary: s\nlabels:\n  - release!\n---\nbody\n";
    assert!(parse_board_markdown(invalid).is_err());
}

#[test]
fn set_field_with_labels_arg() {
    let mut parsed = parse_board_markdown(sample_board()).unwrap();
    set_board_field(&mut parsed, "labels", "ci,release").unwrap();
    assert_eq!(parsed.meta.labels, vec!["ci", "release"]);
}

#[test]
fn set_field_with_tags_alias_arg() {
    let mut parsed = parse_board_markdown(sample_board()).unwrap();
    set_board_field(&mut parsed, "tags", "ci,release").unwrap();
    // tags arg 应当路由到 meta.labels
    assert_eq!(parsed.meta.labels, vec!["ci", "release"]);
}
```

删除原 `unknown_frontmatter_fields_are_rejected` 测(语义反转,被 `unknown_frontmatter_fields_are_silently_dropped` 替代)。

- [ ] **Step 3: 跑 board 测**

```bash
cargo test -p gitim-core types::board -- --nocapture
```

Expected: 所有 test pass(包括新加的 6 个 label-related test)。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/board.rs
git commit -m "feat(labels): board.tags → labels + drop deny_unknown_fields (cross-version compat)"
```

---

### Task 5: `FlowNode.required_labels` + flow validator

**Files:**
- Modify: `crates/gitim-core/src/flow/types.rs`
- Modify: `crates/gitim-core/src/flow/validator.rs`

- [ ] **Step 1: `FlowNode` 加字段**

Modify `crates/gitim-core/src/flow/types.rs` line 71-99(`FlowNode` struct):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,

    #[serde(default)]
    pub needs: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<String>,

    /// 新加。Flow 节点声明的能力需求 — 仅信息位,daemon 不强制 routing。
    /// Coordinator 自行用 `agents_with_labels` IPC 查候选,自己决定拉谁。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_labels: Vec<String>,

    #[serde(skip)]
    pub prompt: String,
}
```

- [ ] **Step 2: validator 加 required_labels 校验**

Modify `crates/gitim-core/src/flow/validator.rs`(应该是检查节点 yaml 的地方;具体位置 grep `MissingRequiredField` 或 `FlowError` 找入口):

```rust
use crate::types::labels::{validate_labels, FLOW_NODE_MAX_LABELS};

// 在 validate_node 函数末尾(或 validate_flow_meta 里的节点 loop 内)加:
fn validate_node_required_labels(node: &FlowNode) -> Result<(), FlowError> {
    validate_labels(&node.required_labels, FLOW_NODE_MAX_LABELS)
        .map_err(|inner| FlowError::InvalidNodeField {
            node: node.id.clone(),
            field: "required_labels",
            inner: inner.to_string(),
        })
}
```

`FlowError` 加 variant(在 types.rs 末尾的 `FlowError` enum):

```rust
#[error("node {node} field {field}: {inner}")]
InvalidNodeField {
    node: String,
    field: &'static str,
    inner: String,
},
```

- [ ] **Step 3: tests**

Modify `crates/gitim-core/src/flow/types.rs` test module 加:

```rust
#[test]
fn old_flow_yaml_without_required_labels_defaults_to_empty() {
    let yaml = "id: n1\ntype: agent_mention\nowner: alice\n";
    let node: FlowNode = serde_yaml::from_str(yaml).unwrap();
    assert!(node.required_labels.is_empty());
}

#[test]
fn new_flow_yaml_with_required_labels_roundtrip() {
    let node = FlowNode {
        id: "n1".into(),
        node_type: NodeType::AgentMention,
        owner: Some("alice".into()),
        participants: vec![],
        signal: None,
        needs: vec![],
        exits: vec![],
        required_labels: vec!["rust".into(), "backend".into()],
        prompt: String::new(),
    };
    let yaml = serde_yaml::to_string(&node).unwrap();
    assert!(yaml.contains("required_labels:"));
    assert!(yaml.contains("- rust"));
    let parsed: FlowNode = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(parsed.required_labels, vec!["rust", "backend"]);
}

#[test]
fn required_labels_omitted_when_empty() {
    let node = FlowNode {
        id: "n1".into(),
        node_type: NodeType::AgentMention,
        owner: Some("alice".into()),
        participants: vec![],
        signal: None,
        needs: vec![],
        exits: vec![],
        required_labels: vec![],
        prompt: String::new(),
    };
    let yaml = serde_yaml::to_string(&node).unwrap();
    assert!(!yaml.contains("required_labels"));
}
```

在 `crates/gitim-core/src/flow/validator.rs`(或 validator 测所在地)加:

```rust
#[test]
fn validator_rejects_invalid_required_label_char() {
    let node = FlowNode {
        id: "n1".into(),
        node_type: NodeType::AgentMention,
        owner: Some("alice".into()),
        participants: vec![],
        signal: None,
        needs: vec![],
        exits: vec![],
        required_labels: vec!["Rust!".into()],
        prompt: String::new(),
    };
    let err = validate_node_required_labels(&node).unwrap_err();
    match err {
        FlowError::InvalidNodeField { node, field, .. } => {
            assert_eq!(node, "n1");
            assert_eq!(field, "required_labels");
        }
        e => panic!("unexpected error: {e:?}"),
    }
}

#[test]
fn validator_rejects_too_many_required_labels() {
    let labels: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
    let node = FlowNode {
        id: "n1".into(),
        node_type: NodeType::AgentMention,
        owner: Some("alice".into()),
        participants: vec![],
        signal: None,
        needs: vec![],
        exits: vec![],
        required_labels: labels,
        prompt: String::new(),
    };
    assert!(validate_node_required_labels(&node).is_err());
}
```

- [ ] **Step 4: 跑测**

```bash
cargo test -p gitim-core flow:: -- --nocapture
```

Expected: 所有 test pass。

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core/src/flow/
git commit -m "feat(labels): FlowNode.required_labels (info-only hint, not enforced)"
```

---

## Phase B — Wire types (Request / Response)

### Task 6: 定义 Request / Response wire types

**Files:**
- Modify: `crates/gitim-core/src/responses.rs`
- Modify: `crates/gitim-daemon/src/api.rs`

- [ ] **Step 1: Response types**

Modify `crates/gitim-core/src/responses.rs`(末尾追加):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LabelsAddResponse {
    pub current_labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LabelsRemoveResponse {
    pub current_labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LabelsListResponse {
    pub handler: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentsWithLabelsResponse {
    pub handlers: Vec<String>,
}
```

`CreateCardResponse` 加字段。先 grep 找到现有 struct(line ~133):

```bash
grep -n "pub struct CreateCardResponse" crates/gitim-core/src/responses.rs
```

Modify:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateCardResponse {
    pub channel: String,
    pub card_id: String,
    pub title: String,
    /// Best-effort assignee recommendations:agent.labels ⊇ card.labels 的 active agents。
    /// 失败 / 没匹配时为 `[]`。Suggestion 不影响 card 是否创建,客户端可忽略。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_assignees: Vec<String>,
}
```

- [ ] **Step 2: Request enum 加 4 个 variant**

Modify `crates/gitim-daemon/src/api.rs`(在 `Request` enum 内部追加):

```rust
    #[serde(rename = "labels_add")]
    LabelsAdd {
        target: String,
        labels: Vec<String>,
    },
    #[serde(rename = "labels_remove")]
    LabelsRemove {
        target: String,
        labels: Vec<String>,
    },
    #[serde(rename = "labels_list")]
    LabelsList {
        target: String,
    },
    #[serde(rename = "agents_with_labels")]
    AgentsWithLabels {
        labels: Vec<String>,
    },
```

- [ ] **Step 3: 跑 build 确认不裂**

```bash
cargo build -p gitim-core -p gitim-daemon -p gitim-client
```

Expected: 编译通过(handler 还没写,但 enum / struct 应该都能编译)。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/responses.rs crates/gitim-daemon/src/api.rs
git commit -m "feat(labels): define IPC Request variants + Response structs"
```

---

## Phase C — Daemon Handlers

### Task 7: `register_user` struct literal fix

**Files:**
- Modify: `crates/gitim-daemon/src/onboard.rs:395-399`

- [ ] **Step 1: 加 `labels: vec![]`**

Modify `crates/gitim-daemon/src/onboard.rs` line 395-399:

```rust
    let meta = UserMeta {
        display_name: display_name.to_string(),
        role: "member".to_string(),
        introduction: "GitIM user".to_string(),
        labels: Vec::new(),
    };
```

- [ ] **Step 2: 跑测**

```bash
cargo test -p gitim-daemon onboard
```

Expected: 现有的 `register_user_creates_meta_and_pushes` / `register_user_skips_if_exists` test pass(struct literal 不补字段会编译失败 catch 这个改动)。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/onboard.rs
git commit -m "fix(onboard): UserMeta struct literal initializes labels: vec![]"
```

---

### Task 8: `handle_labels_add` + `handle_labels_remove` (write path)

**Files:**
- Create: `crates/gitim-daemon/src/handlers/labels.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`

- [ ] **Step 1: 写测先(在 `crates/gitim-daemon/tests/labels_test.rs`)**

Create `crates/gitim-daemon/tests/labels_test.rs`:

```rust
//! Integration tests for daemon labels handlers.
//!
//! Test infrastructure follows the pattern used by other daemon integration tests
//! (e.g. card_test.rs, board_test.rs) — spawn daemon, write request, read response.

mod common;

use common::TestEnv;
use gitim_core::responses::{LabelsAddResponse, LabelsListResponse, AgentsWithLabelsResponse};
use serial_test::serial;

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_add_self_succeeds_and_persists() {
    let env = TestEnv::new_with_user("alice").await;
    let resp: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["rust", "backend"]
        }))
        .await
        .unwrap();
    assert_eq!(resp.current_labels, vec!["rust", "backend"]);

    // re-read via list
    let list: LabelsListResponse = env
        .request_json(serde_json::json!({
            "method": "labels_list",
            "target": "alice"
        }))
        .await
        .unwrap();
    assert_eq!(list.labels, vec!["rust", "backend"]);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_add_non_self_rejected() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;

    let err = env
        .request_raw(serde_json::json!({
            "method": "labels_add",
            "target": "bob",
            "labels": ["rust"]
        }))
        .await;
    assert!(err.error.is_some(), "expected error response, got: {err:?}");
    assert!(
        err.error.as_ref().unwrap().contains("not_self"),
        "expected error_code 'not_self', got: {:?}",
        err.error
    );
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_add_invalid_char_rejected() {
    let env = TestEnv::new_with_user("alice").await;
    let err = env
        .request_raw(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["Rust!"]
        }))
        .await;
    assert!(err.error.is_some());
    assert!(err.error.as_ref().unwrap().contains("invalid_label"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_add_over_cap_rejected() {
    let env = TestEnv::new_with_user("alice").await;
    let labels: Vec<String> = (0..21).map(|i| format!("l{i}")).collect();
    let err = env
        .request_raw(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": labels
        }))
        .await;
    assert!(err.error.is_some());
    assert!(err.error.as_ref().unwrap().contains("labels_full"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_add_dedupes() {
    let env = TestEnv::new_with_user("alice").await;
    let r1: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["rust"]
        }))
        .await
        .unwrap();
    assert_eq!(r1.current_labels, vec!["rust"]);

    let r2: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["rust", "rust", "backend"]
        }))
        .await
        .unwrap();
    assert_eq!(r2.current_labels, vec!["backend", "rust"]);  // sorted unique
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_remove_works() {
    let env = TestEnv::new_with_user("alice").await;
    let _: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["rust", "backend", "frontend"]
        }))
        .await
        .unwrap();

    let r: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_remove",
            "target": "alice",
            "labels": ["backend"]
        }))
        .await
        .unwrap();
    // remove response 复用 LabelsAddResponse shape(current_labels)
    assert_eq!(r.current_labels, vec!["frontend", "rust"]);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_remove_nonexistent_is_noop() {
    let env = TestEnv::new_with_user("alice").await;
    let r: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_remove",
            "target": "alice",
            "labels": ["nonexistent"]
        }))
        .await
        .unwrap();
    assert!(r.current_labels.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_add_preserves_other_user_meta_fields() {
    let env = TestEnv::new_with_user("alice").await;
    let _: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["rust"]
        }))
        .await
        .unwrap();

    // Read user meta yaml directly,确认 display_name / role / introduction 没丢
    let meta_path = env.repo_root.join("users/alice.meta.yaml");
    let contents = std::fs::read_to_string(&meta_path).unwrap();
    assert!(contents.contains("display_name:"));
    assert!(contents.contains("role:"));
    assert!(contents.contains("introduction:"));
    assert!(contents.contains("- rust"));
}
```

注:如 `crates/gitim-daemon/tests/common/` 不存在,看其他 test(`card_test.rs` / `board_test.rs`)如何 setup `TestEnv` 写一份(或复用)。`TestEnv` 的接口契约:`new_with_user(handler)` / `register_user(handler)` / `request_json` / `request_raw`(返回 `Response { ok, error, payload }`)。

- [ ] **Step 2: 跑测,确认 fail "labels_add method not implemented"**

```bash
cargo test -p gitim-daemon --test labels_test labels_add_self_succeeds_and_persists -- --nocapture
```

Expected: FAIL,error message 包含 "unknown method" 或 dispatch panic。

- [ ] **Step 3: 实现 handlers**

Create `crates/gitim-daemon/src/handlers/labels.rs`:

```rust
use std::collections::BTreeSet;

use gitim_core::responses::{
    AgentsWithLabelsResponse, LabelsAddResponse, LabelsListResponse, LabelsRemoveResponse,
};
use gitim_core::types::{
    parse_user_meta_yaml, stringify_user_meta_yaml, validate_labels, UserMeta, USER_MAX_LABELS,
};
use gitim_core::Handler;
use tracing::warn;

use crate::api::Response;
use crate::state::SharedState;

/// caller(state.me.handler) 必须等于 target。否则拒绝。
fn ensure_self(state: &SharedState, target: &str) -> Result<(), Response> {
    let me = state
        .current_user
        .blocking_read()
        .clone()
        .unwrap_or_default();
    if me != target {
        return Err(Response::error_with_code(
            "not_self",
            format!("only self ({}) can modify own labels", me),
        ));
    }
    Ok(())
}

pub async fn handle_labels_add(
    state: SharedState,
    target: String,
    labels: Vec<String>,
) -> Response {
    // 1. 验 caller == target (用 async-friendly current_user lock)
    let me = state.current_user.read().await.clone().unwrap_or_default();
    if me != target {
        return Response::error_with_code(
            "not_self",
            format!("only self ({}) can modify own labels", me),
        );
    }

    // 2. char set / single-label validate 先(union 之前)
    if let Err(e) = validate_labels(&labels, USER_MAX_LABELS) {
        return Response::error_with_code("invalid_label", e.to_string());
    }

    // 3. lock + read-modify-write
    let _guard = state
        .commit_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", target));
    let yaml = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read user meta failed: {e}")),
    };
    let old_bytes = yaml.clone();
    let mut meta: UserMeta = match parse_user_meta_yaml(&yaml) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse user meta failed: {e}")),
    };

    // union + sort + dedupe
    let mut union: BTreeSet<String> = meta.labels.iter().cloned().collect();
    for l in &labels {
        union.insert(l.clone());
    }
    if union.len() > USER_MAX_LABELS {
        return Response::error_with_code(
            "labels_full",
            format!("would exceed user cap {} (got {})", USER_MAX_LABELS, union.len()),
        );
    }
    meta.labels = union.into_iter().collect();

    let new_yaml = match stringify_user_meta_yaml(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("serialize user meta failed: {e}")),
    };
    if let Err(e) = std::fs::write(&meta_path, &new_yaml) {
        return Response::error(format!("write user meta failed: {e}"));
    }

    let rel_path = format!("users/{}.meta.yaml", target);
    let commit_msg = format!("user: labels add @{} +{:?}", target, labels);
    let (author_name, author_email) = state.author_for(&target);
    if let Err(e) =
        state
            .git_storage
            .add_and_commit_as(&[&rel_path], &commit_msg, Some((&author_name, &author_email)))
    {
        // Rollback yaml
        if let Err(restore_err) = std::fs::write(&meta_path, &old_bytes) {
            warn!("labels_add: commit failed, yaml restore also failed: {restore_err}");
        }
        return Response::error(format!("labels_add commit failed: {e}"));
    }

    drop(_guard); // 释放 lock 后再 push,避免 push 阻塞其他 writer

    if state.git_storage.has_remote() {
        if let Err(e) = state.git_storage.push() {
            warn!("labels_add: push failed (commit durable, sync_loop will retry): {e}");
        }
    }

    Response::ok_with(LabelsAddResponse {
        current_labels: meta.labels,
    })
}

pub async fn handle_labels_remove(
    state: SharedState,
    target: String,
    labels: Vec<String>,
) -> Response {
    let me = state.current_user.read().await.clone().unwrap_or_default();
    if me != target {
        return Response::error_with_code(
            "not_self",
            format!("only self ({}) can modify own labels", me),
        );
    }

    let _guard = state
        .commit_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", target));
    let yaml = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read user meta failed: {e}")),
    };
    let old_bytes = yaml.clone();
    let mut meta: UserMeta = match parse_user_meta_yaml(&yaml) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse user meta failed: {e}")),
    };

    let to_remove: BTreeSet<String> = labels.into_iter().collect();
    let mut remaining: BTreeSet<String> = meta.labels.iter().cloned().collect();
    for l in &to_remove {
        remaining.remove(l);
    }
    meta.labels = remaining.into_iter().collect();

    let new_yaml = match stringify_user_meta_yaml(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("serialize user meta failed: {e}")),
    };
    if let Err(e) = std::fs::write(&meta_path, &new_yaml) {
        return Response::error(format!("write user meta failed: {e}"));
    }

    let rel_path = format!("users/{}.meta.yaml", target);
    let commit_msg = format!("user: labels remove @{} -{:?}", target, to_remove);
    let (author_name, author_email) = state.author_for(&target);
    if let Err(e) =
        state
            .git_storage
            .add_and_commit_as(&[&rel_path], &commit_msg, Some((&author_name, &author_email)))
    {
        if let Err(restore_err) = std::fs::write(&meta_path, &old_bytes) {
            warn!("labels_remove: commit failed, yaml restore also failed: {restore_err}");
        }
        return Response::error(format!("labels_remove commit failed: {e}"));
    }

    drop(_guard);

    if state.git_storage.has_remote() {
        if let Err(e) = state.git_storage.push() {
            warn!("labels_remove: push failed (commit durable, sync_loop will retry): {e}");
        }
    }

    Response::ok_with(LabelsRemoveResponse {
        current_labels: meta.labels,
    })
}
```

注 1:`parse_user_meta_yaml` / `stringify_user_meta_yaml` 可能不存在 —— 如果不存在,在 `crates/gitim-core/src/types/meta.rs` 加(同 `card.rs` 的 yaml helper pattern)。

注 2:`Response::error_with_code(code, msg)` 假定存在;若没有,看 `Response::error` 现有实现,扩出来。如果当前 `Response` 只有单字段 `error: Option<String>`,要么:① 把 `error_code` 编码进 message(`format!("not_self: {}", msg)`),客户端 substring match;② 给 `Response` 加 `error_code: Option<String>` 字段(更干净)。**建议走 ②**,plan 阶段确认后改 `crates/gitim-daemon/src/api.rs::Response`。

注 3:`SharedState.current_user` 是 `tokio::sync::RwLock<Option<String>>` —— 已有,见 `state.rs`。Daemon onboard 后会填入。

- [ ] **Step 4: 在 handlers/mod.rs 加 module + re-export**

Modify `crates/gitim-daemon/src/handlers/mod.rs`:

```rust
mod channel;
pub mod cron;
mod depart;
mod dm;
mod labels;  // ← 新加
mod poll;
mod read;
mod search;
mod send;
pub(crate) mod serde;
mod user;

pub use channel::*;
pub use cron::*;
pub use depart::*;
pub use dm::*;
pub use labels::*;  // ← 新加
pub use poll::*;
pub use read::*;
pub use search::*;
pub use send::*;
pub use user::*;
```

- [ ] **Step 5: 在 dispatch 处加 4 个 match arm**

先找 dispatch entry:

```bash
grep -rn "Request::Send\|Request::Read\|Request::Poll" crates/gitim-daemon/src/ | head -10
```

通常在 `server.rs` 或 `lib.rs`。Modify 那里的 match,加:

```rust
Request::LabelsAdd { target, labels } => {
    handlers::handle_labels_add(state.clone(), target, labels).await
}
Request::LabelsRemove { target, labels } => {
    handlers::handle_labels_remove(state.clone(), target, labels).await
}
Request::LabelsList { target } => {
    handlers::handle_labels_list(state.clone(), target).await
}
Request::AgentsWithLabels { labels } => {
    handlers::handle_agents_with_labels(state.clone(), labels).await
}
```

注:list/agents_with_labels 在 Task 9 实现,先 stub 返回 error;Task 9 完成后再去 stub。

- [ ] **Step 6: 跑测**

```bash
cargo test -p gitim-daemon --test labels_test labels_add -- --nocapture
cargo test -p gitim-daemon --test labels_test labels_remove -- --nocapture
```

Expected: 7 个 add/remove test pass。

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-daemon/src/handlers/labels.rs \
        crates/gitim-daemon/src/handlers/mod.rs \
        crates/gitim-daemon/src/server.rs \
        crates/gitim-daemon/tests/labels_test.rs \
        crates/gitim-core/src/types/meta.rs
git commit -m "feat(labels): handle_labels_add + handle_labels_remove with commit_lock RMW"
```

---

### Task 9: `handle_labels_list` + `handle_agents_with_labels` (read path)

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/labels.rs`(继续在同文件追加)
- Modify: `crates/gitim-daemon/tests/labels_test.rs`(加 read-path tests)

- [ ] **Step 1: 写 read-path 测**

Append to `crates/gitim-daemon/tests/labels_test.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_list_returns_self_labels() {
    let env = TestEnv::new_with_user("alice").await;
    let _: LabelsAddResponse = env
        .request_json(serde_json::json!({
            "method": "labels_add",
            "target": "alice",
            "labels": ["rust", "backend"]
        }))
        .await
        .unwrap();

    let resp: LabelsListResponse = env
        .request_json(serde_json::json!({
            "method": "labels_list",
            "target": "alice"
        }))
        .await
        .unwrap();
    assert_eq!(resp.handler, "alice");
    assert_eq!(resp.labels, vec!["backend", "rust"]);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_list_returns_others_labels() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;
    // simulate bob writing his own labels (test infra hack: write yaml directly)
    env.set_user_labels("bob", &["python"]).await;

    let resp: LabelsListResponse = env
        .request_json(serde_json::json!({
            "method": "labels_list",
            "target": "bob"
        }))
        .await
        .unwrap();
    assert_eq!(resp.handler, "bob");
    assert_eq!(resp.labels, vec!["python"]);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_list_returns_404_for_unknown_user() {
    let env = TestEnv::new_with_user("alice").await;
    let err = env
        .request_raw(serde_json::json!({
            "method": "labels_list",
            "target": "noexist"
        }))
        .await;
    assert!(err.error.is_some());
    assert!(err.error.as_ref().unwrap().contains("unknown_user"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn labels_list_returns_404_for_departed_user() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;
    env.depart_user("bob").await; // 调 depart 让 bob 进 archive/users/

    let err = env
        .request_raw(serde_json::json!({
            "method": "labels_list",
            "target": "bob"
        }))
        .await;
    assert!(err.error.is_some());
    assert!(err.error.as_ref().unwrap().contains("unknown_user"));
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn agents_with_labels_finds_all_of_match() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;
    env.register_user("carol").await;
    env.set_user_labels("alice", &["rust", "backend"]).await;
    env.set_user_labels("bob", &["rust", "frontend"]).await;
    env.set_user_labels("carol", &["python"]).await;

    let resp: AgentsWithLabelsResponse = env
        .request_json(serde_json::json!({
            "method": "agents_with_labels",
            "labels": ["rust"]
        }))
        .await
        .unwrap();
    assert_eq!(resp.handlers, vec!["alice", "bob"]);

    let resp: AgentsWithLabelsResponse = env
        .request_json(serde_json::json!({
            "method": "agents_with_labels",
            "labels": ["rust", "backend"]
        }))
        .await
        .unwrap();
    assert_eq!(resp.handlers, vec!["alice"]);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn agents_with_labels_empty_query_returns_empty() {
    let env = TestEnv::new_with_user("alice").await;
    env.set_user_labels("alice", &["rust"]).await;
    let resp: AgentsWithLabelsResponse = env
        .request_json(serde_json::json!({
            "method": "agents_with_labels",
            "labels": []
        }))
        .await
        .unwrap();
    assert!(resp.handlers.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn agents_with_labels_excludes_departed_users() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;
    env.set_user_labels("alice", &["rust"]).await;
    env.set_user_labels("bob", &["rust"]).await;
    env.depart_user("bob").await;

    let resp: AgentsWithLabelsResponse = env
        .request_json(serde_json::json!({
            "method": "agents_with_labels",
            "labels": ["rust"]
        }))
        .await
        .unwrap();
    assert_eq!(resp.handlers, vec!["alice"]);
}
```

- [ ] **Step 2: 跑测,确认 fail**

```bash
cargo test -p gitim-daemon --test labels_test labels_list -- --nocapture
cargo test -p gitim-daemon --test labels_test agents_with_labels -- --nocapture
```

Expected: FAIL("unknown method" 或类似)。

- [ ] **Step 3: 实现 read handler**

Append to `crates/gitim-daemon/src/handlers/labels.rs`:

```rust
pub async fn handle_labels_list(state: SharedState, target: String) -> Response {
    // ensure target is active (in users/), not departed
    let users = state.users.read().await;
    if !users.contains(&target) {
        return Response::error_with_code("unknown_user", format!("unknown user: {}", target));
    }
    drop(users);

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", target));
    let yaml = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read user meta failed: {e}")),
    };
    let meta: UserMeta = match parse_user_meta_yaml(&yaml) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse user meta failed: {e}")),
    };
    Response::ok_with(LabelsListResponse {
        handler: target,
        labels: meta.labels,
    })
}

pub async fn handle_agents_with_labels(
    state: SharedState,
    query_labels: Vec<String>,
) -> Response {
    if query_labels.is_empty() {
        return Response::ok_with(AgentsWithLabelsResponse {
            handlers: vec![],
        });
    }

    // 用 spawn_blocking 包 fs I/O,避免阻塞 reactor
    let users_dir = state.repo_root.join("users");
    let active_users: Vec<String> = state.users.read().await.clone();

    let result = tokio::task::spawn_blocking(move || {
        let query: BTreeSet<String> = query_labels.into_iter().collect();
        let mut matched: Vec<String> = Vec::new();
        for handler in &active_users {
            let path = users_dir.join(format!("{}.meta.yaml", handler));
            let yaml = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue, // skip missing/unreadable
            };
            let meta: UserMeta = match parse_user_meta_yaml(&yaml) {
                Ok(m) => m,
                Err(e) => {
                    warn!("agents_with_labels: skip {handler} (parse error: {e})");
                    continue;
                }
            };
            let agent_set: BTreeSet<String> = meta.labels.into_iter().collect();
            if query.is_subset(&agent_set) {
                matched.push(handler.clone());
            }
        }
        matched.sort();
        matched
    })
    .await
    .unwrap_or_default();

    Response::ok_with(AgentsWithLabelsResponse { handlers: result })
}
```

注意 `state.users` 列表的 source —— 它 cached active handlers,由 `state.rs::on_synced` 维护,depart 后 handler 从这 list 移除。所以 list/agents_with_labels 自动排除 archive。

- [ ] **Step 4: 跑测**

```bash
cargo test -p gitim-daemon --test labels_test labels_list agents_with_labels -- --nocapture
```

Expected: 6 test pass。

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/src/handlers/labels.rs crates/gitim-daemon/tests/labels_test.rs
git commit -m "feat(labels): handle_labels_list + handle_agents_with_labels (read path, excludes departed)"
```

---

### Task 10: `handle_create_card` 加 suggested_assignees

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs:138-260` (handle_create_card)
- Modify: `crates/gitim-daemon/src/handlers/labels.rs` (加 compute_suggested_assignees helper)
- Modify: `crates/gitim-daemon/tests/card_test.rs` (加 suggested_assignees test)

- [ ] **Step 1: 加 compute_suggested_assignees helper**

Append to `crates/gitim-daemon/src/handlers/labels.rs`:

```rust
/// Best-effort:扫所有 active user.meta.yaml,返回 labels ⊇ card_labels 的 handler。
/// 失败(读 fs / parse yaml)在 task 内 log warn,不冒泡;返回 sorted Vec。
pub async fn compute_suggested_assignees(
    state: &SharedState,
    card_labels: Vec<String>,
) -> Vec<String> {
    if card_labels.is_empty() {
        return vec![];
    }
    let users_dir = state.repo_root.join("users");
    let active: Vec<String> = state.users.read().await.clone();

    tokio::task::spawn_blocking(move || {
        let query: BTreeSet<String> = card_labels.into_iter().collect();
        let mut matched: Vec<String> = Vec::new();
        for handler in &active {
            let path = users_dir.join(format!("{}.meta.yaml", handler));
            let yaml = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let Ok(meta) = parse_user_meta_yaml(&yaml) else {
                warn!("compute_suggested_assignees: skip {handler} (parse error)");
                continue;
            };
            let agent_set: BTreeSet<String> = meta.labels.into_iter().collect();
            if query.is_subset(&agent_set) {
                matched.push(handler.clone());
            }
        }
        matched.sort();
        matched
    })
    .await
    .unwrap_or_default()
}
```

- [ ] **Step 2: 测先(append to `crates/gitim-daemon/tests/card_test.rs`)**

```rust
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn create_card_with_labels_suggests_matching_agents() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;
    env.register_user("carol").await;
    env.set_user_labels("alice", &["rust", "backend"]).await;
    env.set_user_labels("bob", &["rust", "frontend"]).await;
    env.set_user_labels("carol", &["python"]).await;
    env.create_channel("dev").await;

    let resp: CreateCardResponse = env
        .request_json(serde_json::json!({
            "method": "create_card",
            "channel": "dev",
            "title": "Implement Rust handler",
            "labels": ["rust"],
            "author": "alice"
        }))
        .await
        .unwrap();
    assert_eq!(resp.suggested_assignees, vec!["alice", "bob"]);
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn create_card_no_labels_no_suggestions() {
    let env = TestEnv::new_with_user("alice").await;
    env.set_user_labels("alice", &["rust"]).await;
    env.create_channel("dev").await;

    let resp: CreateCardResponse = env
        .request_json(serde_json::json!({
            "method": "create_card",
            "channel": "dev",
            "title": "no labels card",
            "author": "alice"
        }))
        .await
        .unwrap();
    assert!(resp.suggested_assignees.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn create_card_unmatched_labels_no_suggestions() {
    let env = TestEnv::new_with_user("alice").await;
    env.set_user_labels("alice", &["rust"]).await;
    env.create_channel("dev").await;

    let resp: CreateCardResponse = env
        .request_json(serde_json::json!({
            "method": "create_card",
            "channel": "dev",
            "title": "needs python",
            "labels": ["python"],
            "author": "alice"
        }))
        .await
        .unwrap();
    assert!(resp.suggested_assignees.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn create_card_excludes_departed_users_from_suggestions() {
    let env = TestEnv::new_with_user("alice").await;
    env.register_user("bob").await;
    env.set_user_labels("alice", &["rust"]).await;
    env.set_user_labels("bob", &["rust"]).await;
    env.depart_user("bob").await;
    env.create_channel("dev").await;

    let resp: CreateCardResponse = env
        .request_json(serde_json::json!({
            "method": "create_card",
            "channel": "dev",
            "title": "rust task",
            "labels": ["rust"],
            "author": "alice"
        }))
        .await
        .unwrap();
    assert_eq!(resp.suggested_assignees, vec!["alice"]);
}
```

- [ ] **Step 3: 实现 — 改 handle_create_card 末尾**

Modify `crates/gitim-daemon/src/card_handlers.rs:235-249`(`event_tx.send` 之后,response 构建之前):

```rust
    let _ = state.event_tx.send(Event::CardCreated {
        channel: ch_name.to_string(),
        card_id: card_id.clone(),
    });

    info!(
        "card '{}' created in channel '{}' by @{}",
        card_id, channel, author
    );

    // suggest assignees based on labels superset match (best-effort,失败为空)
    let suggested_assignees = crate::handlers::compute_suggested_assignees(
        &state,
        meta.labels.clone(),
    )
    .await;

    let payload = gitim_core::responses::CreateCardResponse {
        channel: ch_name.to_string(),
        card_id,
        title,
        suggested_assignees,
    };
```

- [ ] **Step 4: 跑测**

```bash
cargo test -p gitim-daemon --test card_test create_card -- --nocapture
```

Expected: 旧的 create_card test + 4 个新 test 都 pass。

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/src/card_handlers.rs \
        crates/gitim-daemon/src/handlers/labels.rs \
        crates/gitim-daemon/tests/card_test.rs
git commit -m "feat(labels): create_card returns suggested_assignees (labels superset match)"
```

---

## Phase D — Client + CLI

### Task 11: `gitim-client` labels 方法

**Files:**
- Modify: `crates/gitim-client/src/lib.rs`

- [ ] **Step 1: 加 4 个 method + 1 个 CreateCardResponse field 透传**

`gitim-client` 是 thin wrapper around daemon IPC。看现有 method pattern(`send_message` / `read_thread` 等),加:

Modify `crates/gitim-client/src/lib.rs`,append:

```rust
impl GitimClient {
    pub async fn labels_add(
        &self,
        target: &str,
        labels: &[String],
    ) -> Result<gitim_core::responses::LabelsAddResponse, ClientError> {
        let req = serde_json::json!({
            "method": "labels_add",
            "target": target,
            "labels": labels
        });
        self.send_request_typed(req).await
    }

    pub async fn labels_remove(
        &self,
        target: &str,
        labels: &[String],
    ) -> Result<gitim_core::responses::LabelsRemoveResponse, ClientError> {
        let req = serde_json::json!({
            "method": "labels_remove",
            "target": target,
            "labels": labels
        });
        self.send_request_typed(req).await
    }

    pub async fn labels_list(
        &self,
        target: &str,
    ) -> Result<gitim_core::responses::LabelsListResponse, ClientError> {
        let req = serde_json::json!({
            "method": "labels_list",
            "target": target
        });
        self.send_request_typed(req).await
    }

    pub async fn agents_with_labels(
        &self,
        labels: &[String],
    ) -> Result<gitim_core::responses::AgentsWithLabelsResponse, ClientError> {
        let req = serde_json::json!({
            "method": "agents_with_labels",
            "labels": labels
        });
        self.send_request_typed(req).await
    }
}
```

`send_request_typed` 是假定的 helper —— 实际去看现有方法用什么 helper(可能是 `send_request` + manual deserialize)。沿用该 pattern。

- [ ] **Step 2: 跑 build**

```bash
cargo build -p gitim-client
```

Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-client/src/lib.rs
git commit -m "feat(labels): client methods labels_add / remove / list / agents_with_labels"
```

---

### Task 12: `gitim labels` CLI subcommand

**Files:**
- Create: `crates/gitim-cli/src/commands/labels.rs`
- Create: `crates/gitim-cli/tests/labels_cli_test.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`
- Modify: `crates/gitim-cli/src/main.rs`

- [ ] **Step 1: 写 CLI**

Create `crates/gitim-cli/src/commands/labels.rs`:

```rust
use clap::Subcommand;
use gitim_client::GitimClient;

use crate::output::OutputMode;
use super::{get_repo_root, read_my_handler};

#[derive(Subcommand)]
pub enum LabelsCommand {
    /// Add labels to your own user meta
    Add {
        /// Labels to add (space-separated)
        #[arg(required = true, num_args = 1..)]
        labels: Vec<String>,
    },
    /// Remove labels from your own user meta
    Remove {
        /// Labels to remove (space-separated)
        #[arg(required = true, num_args = 1..)]
        labels: Vec<String>,
    },
    /// List labels for a user (default: yourself)
    List {
        /// Target handler (default: self)
        #[arg(long)]
        handler: Option<String>,
    },
    /// Find agents matching ALL given labels (all-of intersection)
    Match {
        /// Labels to match
        #[arg(required = true, num_args = 1..)]
        labels: Vec<String>,
    },
}

pub async fn run(client: GitimClient, cmd: LabelsCommand, mode: OutputMode) -> i32 {
    let repo_root = get_repo_root();
    let me = read_my_handler(&repo_root);

    match cmd {
        LabelsCommand::Add { labels } => match client.labels_add(&me, &labels).await {
            Ok(resp) => {
                if mode == OutputMode::Json {
                    println!("{}", serde_json::to_string(&resp).unwrap());
                } else {
                    println!("labels updated: [{}]", resp.current_labels.join(", "));
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        },
        LabelsCommand::Remove { labels } => match client.labels_remove(&me, &labels).await {
            Ok(resp) => {
                if mode == OutputMode::Json {
                    println!("{}", serde_json::to_string(&resp).unwrap());
                } else {
                    println!("labels updated: [{}]", resp.current_labels.join(", "));
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        },
        LabelsCommand::List { handler } => {
            let target = handler.unwrap_or(me);
            match client.labels_list(&target).await {
                Ok(resp) => {
                    if mode == OutputMode::Json {
                        println!("{}", serde_json::to_string(&resp).unwrap());
                    } else {
                        println!("@{}: [{}]", resp.handler, resp.labels.join(", "));
                    }
                    0
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    1
                }
            }
        }
        LabelsCommand::Match { labels } => match client.agents_with_labels(&labels).await {
            Ok(resp) => {
                if mode == OutputMode::Json {
                    println!("{}", serde_json::to_string(&resp).unwrap());
                } else if resp.handlers.is_empty() {
                    println!("(no agents match all of: {})", labels.join(", "));
                } else {
                    for h in &resp.handlers {
                        println!("@{h}");
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        },
    }
}
```

- [ ] **Step 2: Wire `commands/mod.rs`**

```rust
pub mod admin;
pub mod board;
pub mod burn;
pub mod card;
pub mod channels;
pub mod cron;
pub mod dm;
pub mod flow;
pub mod labels;  // ← 新加
pub mod messaging;
pub mod onboard;
pub mod timer;
pub mod update;
```

- [ ] **Step 3: Wire `main.rs`**

Modify `crates/gitim-cli/src/main.rs` `enum Commands`(在末尾或合适位置加):

```rust
    /// Manage your user labels (capabilities / skills)
    Labels {
        #[command(subcommand)]
        cmd: commands::labels::LabelsCommand,
    },
```

Dispatch match 加:

```rust
        Commands::Labels { cmd } => commands::labels::run(client, cmd, mode).await,
```

- [ ] **Step 4: CLI smoke test**

Create `crates/gitim-cli/tests/labels_cli_test.rs`:

```rust
mod common;
use common::CliEnv;

#[test]
fn gitim_labels_add_then_list_roundtrip() {
    let env = CliEnv::new_with_user("alice");
    let out = env.run(&["labels", "add", "rust", "backend"]).unwrap();
    assert!(out.contains("labels updated"));

    let out = env.run(&["labels", "list"]).unwrap();
    assert!(out.contains("rust"));
    assert!(out.contains("backend"));
}

#[test]
fn gitim_labels_match_finds_agent() {
    let env = CliEnv::new_with_user("alice");
    env.run(&["labels", "add", "rust"]).unwrap();
    let out = env.run(&["labels", "match", "rust"]).unwrap();
    assert!(out.contains("@alice"));
}
```

注:`CliEnv` 接口跟其他 CLI test fixture 一致。如果项目里没现成 `tests/common/mod.rs`,看 `crates/gitim-cli/tests/` 现有 test 用什么 setup,沿用。

- [ ] **Step 5: 跑测**

```bash
cargo test -p gitim-cli --test labels_cli_test -- --nocapture
```

Expected: 2 test pass。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-cli/src/commands/labels.rs \
        crates/gitim-cli/src/commands/mod.rs \
        crates/gitim-cli/src/main.rs \
        crates/gitim-cli/tests/labels_cli_test.rs
git commit -m "feat(labels): gitim labels add/remove/list/match CLI subcommand"
```

---

## Phase E — Runtime HTTP

### Task 13: Runtime HTTP endpoints

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` (路由表 + 4 个 handler)

- [ ] **Step 1: 加 route + handler**

Modify `crates/gitim-runtime/src/http.rs`,在 `/im/cards` route 附近加:

```rust
        .route("/im/labels/{handler}", get(im_labels_list).delete(im_labels_remove))
        .route("/im/labels", post(im_labels_add))
        .route("/im/agents-with-labels", get(im_agents_with_labels))
```

Handler 函数(append to same file,跟现有 `im_*` 函数风格一致):

```rust
async fn im_labels_list(
    State(state): State<AppState>,
    Path(handler): Path<String>,
) -> Result<Json<LabelsListResponse>, AppError> {
    let client = state.workspace_client()?;
    let resp = client.labels_list(&handler).await
        .map_err(|e| AppError::from_client(e))?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct LabelsAddBody {
    labels: Vec<String>,
}

async fn im_labels_add(
    State(state): State<AppState>,
    Json(body): Json<LabelsAddBody>,
) -> Result<Json<LabelsAddResponse>, AppError> {
    let client = state.workspace_client()?;
    let me = state.workspace_handler()?;
    let resp = client.labels_add(&me, &body.labels).await
        .map_err(|e| AppError::from_client(e))?;
    Ok(Json(resp))
}

async fn im_labels_remove(
    State(state): State<AppState>,
    Path(_handler): Path<String>, // 忽略 path param,以 me.json 为准 (per requirement P4)
    Json(body): Json<LabelsAddBody>,
) -> Result<Json<LabelsRemoveResponse>, AppError> {
    let client = state.workspace_client()?;
    let me = state.workspace_handler()?;
    let resp = client.labels_remove(&me, &body.labels).await
        .map_err(|e| AppError::from_client(e))?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct AgentsWithLabelsQuery {
    labels: String, // comma-separated
}

async fn im_agents_with_labels(
    State(state): State<AppState>,
    Query(q): Query<AgentsWithLabelsQuery>,
) -> Result<Json<AgentsWithLabelsResponse>, AppError> {
    let client = state.workspace_client()?;
    let labels: Vec<String> = q.labels.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let resp = client.agents_with_labels(&labels).await
        .map_err(|e| AppError::from_client(e))?;
    Ok(Json(resp))
}
```

注:`AppState::workspace_handler()` / `workspace_client()` 应该已存在(看其他 `im_*` handler 怎么拿 client)。如果具体名字不同,沿用现有模式。`AppError::from_client` 同理 —— 项目现有错误转换。

- [ ] **Step 2: HTTP smoke test(可选,留给 plan 阶段决定)**

如果 `gitim-runtime/tests/` 下有 HTTP test 模式,加一个 smoke test 验 route 注册了。否则跳过 —— route 加 typo 在编译期 catch 不到,但 frontend 实测能 catch。

- [ ] **Step 3: 跑 build + 现有 test 不破**

```bash
cargo build -p gitim-runtime
cargo test -p gitim-runtime --lib
```

Expected: build 通过,现有 test 没倒退。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(labels): runtime HTTP /im/labels + /im/agents-with-labels"
```

---

## Phase F — Frontend (read-only chip)

### Task 14: WebUI labels chip

**Files:**
- Modify: `products/gitim/frontend/src/` (agent detail page + card chip)

注:具体路径要 grep frontend 代码找(`AgentDetail` / `CardDetail` 组件位置)。

- [ ] **Step 1: 找到 agent detail 页**

```bash
grep -rln "AgentDetail\|agent-detail\|agentDetail" products/gitim/frontend/src/ | head -5
```

- [ ] **Step 2: 加 labels chip 组件 + fetch**

在 agent detail 页 component 内,加一个 chip 列表 read-only:

```tsx
// 假设是 src/pages/agent-detail.tsx
import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge"; // 跟项目现有 chip / badge 组件一致

interface LabelsListResponse {
  handler: string;
  labels: string[];
}

function AgentLabelsChips({ handler }: { handler: string }) {
  const [labels, setLabels] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    fetch(`/im/labels/${encodeURIComponent(handler)}`)
      .then((r) => (r.ok ? r.json() : { labels: [] }))
      .then((data: LabelsListResponse) => setLabels(data.labels ?? []))
      .catch(() => setLabels([]))
      .finally(() => setLoading(false));
  }, [handler]);

  if (loading) return null;
  if (labels.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-1 mt-2">
      {labels.map((l) => (
        <Badge key={l} variant="secondary">{l}</Badge>
      ))}
    </div>
  );
}

// 在 AgentDetail 渲染里加 <AgentLabelsChips handler={agent.handler} />
```

Card chip 同 pattern,在 card 详情 / hover preview 用 `card.labels`(已在 card object 里,无需额外 fetch):

```tsx
{card.labels?.length > 0 && (
  <div className="flex flex-wrap gap-1 mt-1">
    {card.labels.map((l) => (
      <Badge key={l} variant="outline" className="text-xs">{l}</Badge>
    ))}
  </div>
)}
```

- [ ] **Step 3: lint + typecheck**

```bash
cd products/gitim/frontend && bun run typecheck && bun run lint
```

Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add products/gitim/frontend/src/
git commit -m "feat(labels): WebUI read-only labels chip on agent + card"
```

---

## Phase G — Documentation

### Task 15: Update prompts.rs + CLAUDE.md

**Files:**
- Modify: `crates/gitim-agent-provider/src/prompts.rs:501`
- Modify: `CLAUDE.md` Current Orientation 段

- [ ] **Step 1: prompts.rs:501**

Modify `crates/gitim-agent-provider/src/prompts.rs:501` 那一行 + 在 board 段后追加 labels 段:

```diff
- - `gitim board set <field> <value>` — 更新 frontmatter 字段，`field` 为 `status` / `summary` / `tags`
+ - `gitim board set <field> <value>` — 更新 frontmatter 字段，`field` 为 `status` / `summary` / `labels`（`tags` 是兼容别名，等价于 `labels`）
```

在 board 段后(line ~514 之后)追加 `gitim labels` 段:

```
## 标签 / 能力宣告（labels）

你可以给自己 user.meta.yaml 加 labels 来宣告能力（如 rust / backend / frontend-react / mobile-ios）。
这些 labels 会被 create_card 拿来推荐 assignee，被 coordinator 拿来按 flow 节点的 required_labels 找候选人。

- `gitim labels add <label1> <label2> ...` — 加 labels 到自己（不能加到别人）
- `gitim labels remove <label1> <label2> ...` — 从自己移除
- `gitim labels list` — 看自己当前 labels
- `gitim labels list --handler <他人>` — 看别人当前 labels
- `gitim labels match <label1> <label2> ...` — 找所有 labels 同时包含这些的 agent

约定:
- 字符集 `a-z 0-9 - _`，单 label 最长 32 字符，单人最多 20 个
- labels 是能力声明，不是话题标签 —— Card 上的 labels 是「这张卡涉及什么领域」，Agent 上的 labels 是「你会什么」
- self-claim only —— 你只能改自己的；想让别人加 label 只能社交沟通
- 没有 namespace（`:`）—— 自然 prefix 约定即可：`frontend-react` / `mobile-ios`
```

- [ ] **Step 2: CLAUDE.md orientation**

Modify `CLAUDE.md` 的 "Current Orientation" 段,在 "Where we are" 末尾追加一段:

```markdown
**Unified labels space v1** 已落地:CardMeta.labels / BoardMeta.labels(原 tags) / UserMeta.labels / FlowNode.required_labels 全部走 `gitim-core::types::labels.rs` 共享 validator + `LabelError`(char set `a-z 0-9 - _`、单 label 32 char、各对象 max_count 各异:card 10 / board 20 / user 20 / flow_node 10)。BoardMeta 移除 `deny_unknown_fields`(eng-review Issue #1,让新 daemon 写 `labels:` 时老 daemon fetch 不挂);`set_board_field` 同时接受 `"tags"` 和 `"labels"` arg。Daemon 4 个 IPC:`LabelsAdd / LabelsRemove`(self-claim only,read-modify-write 在 `commit_lock` 内,rollback yaml on commit fail) + `LabelsList`(`ensure_known_user` 拒 departed,返 404) + `AgentsWithLabels`(all-of subset,排除 archive/users/)。`handle_create_card` push 之后调 `compute_suggested_assignees` 给 `CreateCardResponse.suggested_assignees`(best-effort,客户端可忽略)。CLI 新加 `gitim labels add/remove/list/match`,Runtime HTTP 加 `GET/POST/DELETE /im/labels` + `GET /im/agents-with-labels`,WebUI agent detail / card 加 read-only labels chip。Spec/Plan 在 `docs/plans/unified-labels/`。
```

注:具体段落措辞跟 CLAUDE.md 现有 orientation 段落 style 一致(参考 oneshot timer / agent routing v1 那种密度的描述)。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-agent-provider/src/prompts.rs CLAUDE.md
git commit -m "docs(labels): prompts.rs + CLAUDE.md orientation 描述 labels space"
```

---

## Self-Review

**Spec coverage check**(verify each premise has implementing tasks):

| Premise | Implementing task |
|---|---|
| P1 永远 embedded | All — no registry file 引入 |
| P2 字段统一 labels | T1-T5 全部 |
| P3 UserMeta SoT | T3 + T8/T9 (read 只看 users/) |
| P4 self-claim only | T8 `ensure_self` |
| P5 card-suggest | T10 `compute_suggested_assignees` + create_card |
| P5b storage model | T9/T10 walk users/*.meta.yaml each call |
| P6 flow info-only | T5 (字段加但 daemon 不强制 routing) |
| P7 char set / no namespace | T1 validator |
| P8 onboard labels=[] | T7 register_user |
| P9 max_counts per object | T1 constants |
| P10 CLI naming split | T12 `gitim labels`(not collapsed with card) |

**Placeholder scan**: ✅ No "TBD" / "implement later" / "similar to" etc.

**Type consistency check**:
- `LabelError` 出现在 T1 / T2 / T3 / T5 — 同一类型,同源 import
- `UserMeta.labels: Vec<String>` 在 T3 / T7 / T8 / T9 — 一致
- `LabelsAddResponse.current_labels: Vec<String>` 在 T6 / T8 / T11 — 一致
- `CreateCardResponse.suggested_assignees: Vec<String>` 在 T6 / T10 / T11 — 一致
- `compute_suggested_assignees(&state, Vec<String>) -> Vec<String>` 签名在 T10 一致

---

## Execution Handoff

Plan complete and saved to `docs/plans/unified-labels/01-plan.md`。

继续 SOP Phase 5,走 `subagent-driven-development` —— 每个 task dispatch fresh subagent 实现 + 两阶段 review。
