# Team Flows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 GitIM 仓库引入 `flows/<slug>/index.md` 模板系统 —— 团队 SOP 流程库，git-tracked markdown 模板 + DAG 渲染 + coordinator 通过 IM 显式调用,**不强制执行,只作为参考材料**。

**Architecture:** flow 模板存 `<repo-root>/flows/<slug>/index.md`,frontmatter 描述节点 + needs[] 依赖,body 用 `## <node-id>` section 给每个节点的 prompt。`gitim-core::flow` 模块负责类型、解析、验证;`gitim-daemon::flow_handlers` 负责 5 个 IPC handler(list/show/create/remove/validate),走 `state.commit_lock` + `git_storage.add_and_commit_only_as` 单文件 commit;`gitim-sync::watcher` 加 `FlowModified(slug)` 变体,daemon 接到事件后 re-validate + commit & sync;`gitim-cli` 加 `flow` 子命令;`products/gitim/frontend` 加 Flows tab(mermaid.js DAG + react-markdown prompt body,lazy-import);各 provider 的 `default_gitim_api()` 增量加 flows 段介绍。

**Tech Stack:** Rust(`gitim-core`/`gitim-daemon`/`gitim-sync`/`gitim-client`/`gitim-cli`/`gitim-agent-provider`)、TypeScript(React 19 + Vite + Radix UI + Tailwind + Zustand)、mermaid.js 11.x、react-markdown 9.x、`serde_yaml`(frontmatter 已有依赖)、`thiserror`(错误类型已有依赖)。

**Spec:** [docs/design/team-flows-design.md](../../design/team-flows-design.md) — 已 approved,2026-05-17 收口的 3 个 open question 在文末"实现决策"段。

---

## File Structure

### gitim-core (新增)

| 文件 | 责任 |
|---|---|
| `crates/gitim-core/src/flow/mod.rs` | 模块声明 + re-export |
| `crates/gitim-core/src/flow/types.rs` | `FlowSlug` newtype、`NodeType` enum、`FlowNode`、`FlowMeta`、`FlowDocument`、`FlowError`、`FlowWarning`、`flow_path()` helper |
| `crates/gitim-core/src/flow/parser.rs` | `parse_flow_markdown(content)` 把 frontmatter YAML + body section 解析成 `FlowDocument`;`stringify_flow_markdown(doc)` 反向 |
| `crates/gitim-core/src/flow/validator.rs` | `validate_flow_document(doc)` 检查 id 唯一、`needs` 引用合法、无环;`validate_flow_for_storage(doc)` 加 size/count warn |

### gitim-core (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-core/src/lib.rs` | `pub mod flow;` |
| `crates/gitim-core/src/responses.rs` | 加 `ListFlowsResponse`、`FlowSummary`、`ShowFlowResponse`、`WriteFlowResponse`、`ValidateFlowResponse`、`FlowValidationItem` |

### gitim-daemon (新增)

| 文件 | 责任 |
|---|---|
| `crates/gitim-daemon/src/flow_handlers.rs` | 5 handler: `handle_flow_list`、`handle_flow_show`、`handle_flow_create`、`handle_flow_remove`、`handle_flow_validate`;`commit_flow_document_locked` helper |
| `crates/gitim-daemon/tests/flow_handlers.rs` | tempdir repo 集成测试: e2e create → list → show → validate → remove |

### gitim-daemon (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-daemon/src/api.rs` | `Request` enum 加 `FlowList` / `FlowShow` / `FlowCreate` / `FlowRemove` / `FlowValidate` 变体;`Event` enum 加 `FlowChanged { slug }` |
| `crates/gitim-daemon/src/handlers/mod.rs` | dispatch 5 个新 Request 到 `flow_handlers::*` |
| `crates/gitim-daemon/src/lib.rs` | `pub mod flow_handlers;` |
| `crates/gitim-daemon/src/main.rs` | watcher consumer 加 `FlowModified` 分支:re-validate + commit & sync + 发 `FlowChanged` event |

### gitim-sync (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-sync/src/watcher.rs` | `FileEvent` 加 `FlowModified(String)` 变体(slug);watch `flows/` 用 `RecursiveMode::Recursive`;path 形如 `flows/<slug>/index.md` 提 slug |

### gitim-client (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-client/src/client.rs` | 加 `flow_list`、`flow_show`、`flow_create`、`flow_remove`、`flow_validate` 5 个 method |

### gitim-cli (新增)

| 文件 | 责任 |
|---|---|
| `crates/gitim-cli/src/commands/flow.rs` | `cmd_flow_list`、`cmd_flow_show`、`cmd_flow_create`、`cmd_flow_remove`、`cmd_flow_validate` |
| `crates/gitim-cli/tests/flow_commands.rs` | e2e 测试: create stub → list → show → validate → remove |

### gitim-cli (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-cli/src/main.rs` | clap `Commands::Flow { command: FlowCommands }` enum + `FlowCommands` (List/Show/Create/Remove/Validate) + dispatch |
| `crates/gitim-cli/src/commands/mod.rs` | `pub mod flow;` |

### gitim-agent-provider (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-agent-provider/src/prompts.rs` | `default_gitim_api()` 在 "### 状态板 (Boards)" 段后插入 "### 流程模板 (Flows)" 段 |

### frontend (新增 + 修改)

| 文件 | 责任 |
|---|---|
| `products/gitim/frontend/package.json` | 加 `mermaid` ^11、`react-markdown` ^9 依赖 |
| `products/gitim/frontend/src/lib/types.ts` | `FlowSummary`、`FlowDocument`、`FlowNode`、`NodeType` TS 类型 |
| `products/gitim/frontend/src/lib/client.ts` | `listFlows`、`getFlow`、`createFlow`、`removeFlow`、`validateFlow` |
| `products/gitim/frontend/src/hooks/use-flow-store.ts` | zustand store(`flows`、`selectedSlug`、`loadFlows`、`loadFlow`) |
| `products/gitim/frontend/src/components/flows/flows-view.tsx` | 列表 + 详情两栏布局 |
| `products/gitim/frontend/src/components/flows/flow-dag.tsx` | mermaid lazy 渲染组件 |
| `products/gitim/frontend/src/components/flows/flow-detail.tsx` | 详情面板(DAG + 节点 prompt + Run this flow 按钮) |
| `products/gitim/frontend/src/components/<nav-root>.tsx` | 加 "Flows" tab |

---

## Task 1: gitim-core flow module skeleton + FlowSlug newtype

**Files:**
- Create: `crates/gitim-core/src/flow/mod.rs`
- Create: `crates/gitim-core/src/flow/types.rs`
- Modify: `crates/gitim-core/src/lib.rs`

- [ ] **Step 1: 在 `crates/gitim-core/src/lib.rs` 加 `pub mod flow;`**

找到现有 `pub mod` 行(如 `pub mod types;`),在其后面加:

```rust
pub mod flow;
```

- [ ] **Step 2: 创建 `crates/gitim-core/src/flow/mod.rs`**

```rust
pub mod parser;
pub mod types;
pub mod validator;

pub use parser::{parse_flow_markdown, stringify_flow_markdown};
pub use types::{
    flow_path, FlowDocument, FlowError, FlowMeta, FlowNode, FlowSlug, FlowSlugError, FlowWarning,
    NodeType,
};
pub use validator::{validate_flow_document, validate_flow_for_storage};
```

- [ ] **Step 3: 写 `FlowSlug` 失败测试**

创建 `crates/gitim-core/src/flow/types.rs`,先写 test 模块(测试用 `super::*` 但 `FlowSlug` 还没实现):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_slugs() {
        for name in &["release", "kickoff", "weekly-retro", "a", "a1b2"] {
            assert!(FlowSlug::new(name).is_ok(), "expected '{}' to be valid", name);
        }
    }

    #[test]
    fn test_empty_slug() {
        let err = FlowSlug::new("").unwrap_err();
        assert!(matches!(err, FlowSlugError::Empty));
    }

    #[test]
    fn test_too_long() {
        let name = "a".repeat(40);
        let err = FlowSlug::new(&name).unwrap_err();
        assert!(matches!(err, FlowSlugError::TooLong));
    }

    #[test]
    fn test_invalid_chars() {
        for name in &["UPPER", "under_score", "space name", "../etc", "x/y"] {
            let err = FlowSlug::new(name).unwrap_err();
            assert!(matches!(err, FlowSlugError::InvalidChar(_)), "for '{}', got {:?}", name, err);
        }
    }

    #[test]
    fn test_hyphen_boundary() {
        for name in &["-start", "end-"] {
            let err = FlowSlug::new(name).unwrap_err();
            assert!(matches!(err, FlowSlugError::HyphenBoundary));
        }
    }

    #[test]
    fn test_consecutive_hyphens() {
        let err = FlowSlug::new("a--b").unwrap_err();
        assert!(matches!(err, FlowSlugError::ConsecutiveHyphens));
    }

    #[test]
    fn test_flow_path() {
        let slug = FlowSlug::new("release").unwrap();
        assert_eq!(flow_path(&slug), std::path::PathBuf::from("flows/release/index.md"));
    }
}
```

- [ ] **Step 4: 实现 FlowSlug + flow_path 让测试通过**

把以下 code 加在 types.rs 顶部(tests 模块之前):

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FlowSlugError {
    #[error("flow slug is empty")]
    Empty,
    #[error("flow slug exceeds 39 characters")]
    TooLong,
    #[error("flow slug contains invalid character: {0}")]
    InvalidChar(char),
    #[error("flow slug must not start or end with hyphen")]
    HyphenBoundary,
    #[error("flow slug must not contain consecutive hyphens")]
    ConsecutiveHyphens,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowSlug(String);

impl FlowSlug {
    pub fn new(s: &str) -> Result<Self, FlowSlugError> {
        if s.is_empty() {
            return Err(FlowSlugError::Empty);
        }
        if s.len() > 39 {
            return Err(FlowSlugError::TooLong);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(FlowSlugError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(FlowSlugError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(FlowSlugError::ConsecutiveHyphens);
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for FlowSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn flow_path(slug: &FlowSlug) -> std::path::PathBuf {
    std::path::PathBuf::from("flows").join(slug.as_str()).join("index.md")
}
```

- [ ] **Step 5: 跑测试,确认 FlowSlug 通过**

Run:
```bash
cargo test -p gitim-core flow::types::tests
```

Expected: 7 passed, 0 failed.

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-core/src/lib.rs crates/gitim-core/src/flow/
git commit -m "feat(core): add FlowSlug newtype + flow_path helper

Mirrors ChannelName pattern. Slug rules: lowercase a-z 0-9 hyphen,
1-39 chars, no leading/trailing hyphen, no consecutive hyphens.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: NodeType + FlowNode + FlowMeta + FlowDocument types

**Files:**
- Modify: `crates/gitim-core/src/flow/types.rs`

- [ ] **Step 1: 加 NodeType / FlowNode / FlowMeta / FlowDocument / FlowError / FlowWarning 类型**

在 `types.rs` 的 `flow_path` 之后、`#[cfg(test)]` 之前插入:

```rust
use serde::{Deserialize, Serialize};

/// 节点类型。v1 落地 agent_mention + channel_thread;human_review / wait_for_signal 留位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    AgentMention,
    ChannelThread,
    HumanReview,
    WaitForSignal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,

    /// agent_mention 必填:派给谁
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    /// channel_thread 必填:参与者
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,

    /// wait_for_signal 必填:信号名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,

    /// 上游依赖。空数组 = 入口节点。
    #[serde(default)]
    pub needs: Vec<String>,

    /// v2 conditional 留位:节点可能的退出 label。v1 解析但不读。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<String>,

    /// 节点 prompt body(由 body section parser 注入,frontmatter 里不读)。
    #[serde(skip)]
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowMeta {
    pub schema_version: u32,
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_by: String,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub nodes: Vec<FlowNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowDocument {
    pub meta: FlowMeta,
}

#[derive(Error, Debug)]
pub enum FlowError {
    #[error("invalid slug: {0}")]
    InvalidSlug(#[from] FlowSlugError),
    #[error("missing frontmatter delimiter")]
    MissingFrontmatter,
    #[error("frontmatter yaml: {0}")]
    YamlParse(String),
    #[error("schema mismatch: expected schema_version 1, got {0}")]
    SchemaVersion(u32),
    #[error("slug in frontmatter ({frontmatter}) != path slug ({path})")]
    SlugMismatch { frontmatter: String, path: String },
    #[error("duplicate node id: {0}")]
    DuplicateNodeId(String),
    #[error("node {node} references unknown id in needs: {missing}")]
    UnknownNeed { node: String, missing: String },
    #[error("cycle detected in flow DAG")]
    Cycle,
    #[error("node {0} type {1:?} missing required field: {2}")]
    MissingRequiredField(String, NodeType, &'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowWarning {
    /// frontmatter 有 id 但 body 缺 `## id` section
    BodySectionMissing(String),
    /// body 有 `## id` 但 frontmatter 没声明
    OrphanBodySection(String),
    /// 文件 size 超过 256KB
    OversizedFile { actual: usize, limit: usize },
    /// 节点数超过 50
    TooManyNodes { count: usize, limit: usize },
}
```

- [ ] **Step 2: 加 type 基础 tests**

在 `tests` 模块底部加:

```rust
    #[test]
    fn test_node_type_serialize_snake_case() {
        let json = serde_json::to_string(&NodeType::AgentMention).unwrap();
        assert_eq!(json, "\"agent_mention\"");
        let json2 = serde_json::to_string(&NodeType::ChannelThread).unwrap();
        assert_eq!(json2, "\"channel_thread\"");
    }

    #[test]
    fn test_flow_node_default_fields_omitted() {
        let node = FlowNode {
            id: "n1".into(),
            node_type: NodeType::AgentMention,
            owner: Some("alice".into()),
            participants: vec![],
            signal: None,
            needs: vec![],
            exits: vec![],
            prompt: String::new(),
        };
        let yaml = serde_yaml::to_string(&node).unwrap();
        assert!(yaml.contains("id: n1"), "yaml={yaml}");
        assert!(yaml.contains("owner: alice"), "yaml={yaml}");
        assert!(!yaml.contains("participants"), "yaml={yaml}");
        assert!(!yaml.contains("signal"), "yaml={yaml}");
        assert!(!yaml.contains("exits"), "yaml={yaml}");
    }
```

- [ ] **Step 3: 跑测试**

Run:
```bash
cargo test -p gitim-core flow::types::tests
```

Expected: 9 passed.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/flow/types.rs
git commit -m "feat(core): add FlowNode/FlowMeta/FlowDocument types

NodeType is serde snake_case enum (agent_mention, channel_thread,
human_review, wait_for_signal). v1 落地前两种,后两种 schema 占位。
exits[] 字段为 v2 conditional 留位,v1 解析但不消费。

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: parse_flow_markdown(frontmatter + body sections)

**Files:**
- Create: `crates/gitim-core/src/flow/parser.rs`

- [ ] **Step 1: 写失败测试**

```rust
use crate::flow::types::{
    FlowDocument, FlowError, FlowMeta, FlowNode, FlowSlug, FlowWarning, NodeType,
};

pub fn parse_flow_markdown(content: &str) -> Result<FlowDocument, FlowError> {
    todo!("implement in step 2")
}

pub fn parse_flow_markdown_with_warnings(
    content: &str,
) -> Result<(FlowDocument, Vec<FlowWarning>), FlowError> {
    todo!("implement in step 2")
}

pub fn stringify_flow_markdown(_doc: &FlowDocument) -> Result<String, FlowError> {
    todo!("implement in Task 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"---
schema_version: 1
slug: release
name: Release Flow
description: 用于一次正式版本发布
created_by: lewis
created_at: 2026-05-12T10:00:00Z
nodes:
  - id: changelog
    type: agent_mention
    owner: alice
    needs: []
  - id: e2e
    type: agent_mention
    owner: bob
    needs: [changelog]
---

## changelog

请基于 `git log v0.7..HEAD` 生成 changelog。

## e2e

跑 `cargo test --workspace`。
"#;

    #[test]
    fn test_parse_happy_path() {
        let doc = parse_flow_markdown(SAMPLE).unwrap();
        assert_eq!(doc.meta.slug, "release");
        assert_eq!(doc.meta.schema_version, 1);
        assert_eq!(doc.meta.nodes.len(), 2);
        assert_eq!(doc.meta.nodes[0].id, "changelog");
        assert_eq!(doc.meta.nodes[0].node_type, NodeType::AgentMention);
        assert_eq!(doc.meta.nodes[0].owner.as_deref(), Some("alice"));
        assert!(doc.meta.nodes[0].prompt.contains("changelog"));
        assert_eq!(doc.meta.nodes[1].needs, vec!["changelog"]);
        assert!(doc.meta.nodes[1].prompt.contains("cargo test"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let err = parse_flow_markdown("## changelog\n\nfoo\n").unwrap_err();
        assert!(matches!(err, FlowError::MissingFrontmatter));
    }

    #[test]
    fn test_parse_schema_version_mismatch() {
        let bad = SAMPLE.replace("schema_version: 1", "schema_version: 2");
        let err = parse_flow_markdown(&bad).unwrap_err();
        assert!(matches!(err, FlowError::SchemaVersion(2)));
    }

    #[test]
    fn test_parse_body_section_missing_warning() {
        let body_stripped = r#"---
schema_version: 1
slug: r
name: r
created_by: lewis
created_at: 2026-05-12T10:00:00Z
nodes:
  - id: a
    type: agent_mention
    owner: alice
    needs: []
---
"#;
        let (_doc, warnings) = parse_flow_markdown_with_warnings(body_stripped).unwrap();
        assert!(
            warnings.iter().any(|w| matches!(w, FlowWarning::BodySectionMissing(s) if s == "a")),
            "warnings={warnings:?}",
        );
    }

    #[test]
    fn test_parse_orphan_body_section_warning() {
        let with_orphan = format!(
            "{}\n## extra\n\nthis section has no frontmatter id\n",
            SAMPLE
        );
        let (_doc, warnings) = parse_flow_markdown_with_warnings(&with_orphan).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| matches!(w, FlowWarning::OrphanBodySection(s) if s == "extra")),
            "warnings={warnings:?}",
        );
    }
}
```

- [ ] **Step 2: 实现 parser**

Replace the `todo!()` bodies with real logic. 注意 frontmatter 分隔符 `---`、YAML deserialize 用 `serde_yaml`、body section 用正则 `^## (\w[\w-]*)$` 切分。

```rust
use crate::flow::types::{
    FlowDocument, FlowError, FlowMeta, FlowWarning,
};

const FRONTMATTER_DELIM: &str = "---";

pub fn parse_flow_markdown(content: &str) -> Result<FlowDocument, FlowError> {
    let (doc, _warnings) = parse_flow_markdown_with_warnings(content)?;
    Ok(doc)
}

pub fn parse_flow_markdown_with_warnings(
    content: &str,
) -> Result<(FlowDocument, Vec<FlowWarning>), FlowError> {
    let mut warnings = Vec::new();

    let trimmed = content.trim_start_matches('\u{FEFF}');
    if !trimmed.starts_with(FRONTMATTER_DELIM) {
        return Err(FlowError::MissingFrontmatter);
    }
    let after_open = &trimmed[FRONTMATTER_DELIM.len()..];
    let end = after_open
        .find(&format!("\n{}", FRONTMATTER_DELIM))
        .ok_or(FlowError::MissingFrontmatter)?;
    let yaml_body = &after_open[..end];
    let body_after = &after_open[end + FRONTMATTER_DELIM.len() + 1..];

    let mut meta: FlowMeta =
        serde_yaml::from_str(yaml_body.trim()).map_err(|e| FlowError::YamlParse(e.to_string()))?;
    if meta.schema_version != 1 {
        return Err(FlowError::SchemaVersion(meta.schema_version));
    }

    let section_map = split_body_sections(body_after);

    let frontmatter_ids: std::collections::HashSet<&str> =
        meta.nodes.iter().map(|n| n.id.as_str()).collect();
    for node in meta.nodes.iter_mut() {
        match section_map.get(node.id.as_str()) {
            Some(text) => node.prompt = text.clone(),
            None => warnings.push(FlowWarning::BodySectionMissing(node.id.clone())),
        }
    }
    for section_id in section_map.keys() {
        if !frontmatter_ids.contains(section_id.as_str()) {
            warnings.push(FlowWarning::OrphanBodySection(section_id.clone()));
        }
    }

    Ok((FlowDocument { meta }, warnings))
}

fn split_body_sections(body: &str) -> std::collections::BTreeMap<String, String> {
    let mut sections = std::collections::BTreeMap::new();
    let mut current_id: Option<String> = None;
    let mut buf = String::new();

    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(id) = current_id.take() {
                sections.insert(id, buf.trim().to_string());
                buf.clear();
            }
            current_id = Some(rest.trim().to_string());
        } else if current_id.is_some() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if let Some(id) = current_id.take() {
        sections.insert(id, buf.trim().to_string());
    }
    sections
}

pub fn stringify_flow_markdown(doc: &FlowDocument) -> Result<String, FlowError> {
    let _ = doc;
    todo!("implemented in Task 4")
}
```

注意 `serde_yaml` 已经在 `gitim-core` 的 `Cargo.toml` 里(board/channel meta 已用),如果不在则需要 `cargo add -p gitim-core serde_yaml`。

- [ ] **Step 3: 跑测试**

Run:
```bash
cargo test -p gitim-core flow::parser::tests
```

Expected: 5 passed, 0 failed.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/flow/parser.rs
git commit -m "feat(core): add flow markdown parser with body section warnings

Parses frontmatter YAML + ## <node-id> body sections. Frontmatter is
source of truth; body sections feed node.prompt. Missing/orphan
section pairs produce warnings (non-fatal).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: stringify_flow_markdown(serialize back)

**Files:**
- Modify: `crates/gitim-core/src/flow/parser.rs`

- [ ] **Step 1: 写 round-trip 测试**

在 parser.rs 的 tests 模块底部加:

```rust
    #[test]
    fn test_stringify_round_trip() {
        let doc = parse_flow_markdown(SAMPLE).unwrap();
        let rendered = stringify_flow_markdown(&doc).unwrap();
        let parsed_back = parse_flow_markdown(&rendered).unwrap();
        assert_eq!(parsed_back.meta.slug, doc.meta.slug);
        assert_eq!(parsed_back.meta.nodes.len(), doc.meta.nodes.len());
        for (a, b) in parsed_back.meta.nodes.iter().zip(doc.meta.nodes.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.node_type, b.node_type);
            assert_eq!(a.owner, b.owner);
            assert_eq!(a.needs, b.needs);
            assert_eq!(a.prompt.trim(), b.prompt.trim());
        }
    }

    #[test]
    fn test_stringify_excludes_prompt_from_frontmatter() {
        let doc = parse_flow_markdown(SAMPLE).unwrap();
        let rendered = stringify_flow_markdown(&doc).unwrap();
        let (frontmatter_block, _body) = rendered
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap();
        assert!(
            !frontmatter_block.contains("prompt:"),
            "frontmatter should not contain prompt field\n{frontmatter_block}",
        );
    }
```

- [ ] **Step 2: 实现 stringify**

Replace `stringify_flow_markdown` 的 `todo!()` body:

```rust
pub fn stringify_flow_markdown(doc: &FlowDocument) -> Result<String, FlowError> {
    let frontmatter =
        serde_yaml::to_string(&doc.meta).map_err(|e| FlowError::YamlParse(e.to_string()))?;
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(frontmatter.trim_end());
    out.push_str("\n---\n");
    for node in &doc.meta.nodes {
        out.push_str("\n## ");
        out.push_str(&node.id);
        out.push_str("\n\n");
        if !node.prompt.is_empty() {
            out.push_str(node.prompt.trim());
            out.push('\n');
        }
    }
    Ok(out)
}
```

- [ ] **Step 3: 跑测试**

Run:
```bash
cargo test -p gitim-core flow::parser::tests
```

Expected: 7 passed.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/flow/parser.rs
git commit -m "feat(core): add stringify_flow_markdown with round-trip test

prompt 字段 #[serde(skip)] 不进 frontmatter,只通过 ## <id> body
section 还原。

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: validator(id 唯一 / needs 引用 / 无环 / size warn)

**Files:**
- Create: `crates/gitim-core/src/flow/validator.rs`

- [ ] **Step 1: 写失败测试**

```rust
use crate::flow::types::{FlowDocument, FlowError, FlowMeta, FlowNode, FlowWarning, NodeType};

pub fn validate_flow_document(_doc: &FlowDocument, _slug_in_path: &str) -> Result<(), FlowError> {
    todo!("implement in step 2")
}

pub fn validate_flow_for_storage(_doc: &FlowDocument, _file_size: usize) -> Vec<FlowWarning> {
    todo!("implement in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_nodes(nodes: Vec<FlowNode>) -> FlowDocument {
        FlowDocument {
            meta: FlowMeta {
                schema_version: 1,
                slug: "test".into(),
                name: "Test".into(),
                description: String::new(),
                created_by: "lewis".into(),
                created_at: "2026-05-12T10:00:00Z".into(),
                updated_at: None,
                nodes,
            },
        }
    }

    fn node(id: &str, needs: &[&str]) -> FlowNode {
        FlowNode {
            id: id.into(),
            node_type: NodeType::AgentMention,
            owner: Some("alice".into()),
            participants: vec![],
            signal: None,
            needs: needs.iter().map(|s| s.to_string()).collect(),
            exits: vec![],
            prompt: String::new(),
        }
    }

    #[test]
    fn test_validate_happy_path() {
        let d = doc_with_nodes(vec![node("a", &[]), node("b", &["a"])]);
        assert!(validate_flow_document(&d, "test").is_ok());
    }

    #[test]
    fn test_slug_mismatch() {
        let d = doc_with_nodes(vec![node("a", &[])]);
        let err = validate_flow_document(&d, "different-slug").unwrap_err();
        assert!(matches!(err, FlowError::SlugMismatch { .. }));
    }

    #[test]
    fn test_duplicate_node_id() {
        let d = doc_with_nodes(vec![node("a", &[]), node("a", &[])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::DuplicateNodeId(id) if id == "a"));
    }

    #[test]
    fn test_unknown_need() {
        let d = doc_with_nodes(vec![node("a", &["ghost"])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::UnknownNeed { .. }));
    }

    #[test]
    fn test_cycle_detection() {
        let d = doc_with_nodes(vec![node("a", &["b"]), node("b", &["a"])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::Cycle));
    }

    #[test]
    fn test_self_cycle() {
        let d = doc_with_nodes(vec![node("a", &["a"])]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::Cycle));
    }

    #[test]
    fn test_agent_mention_missing_owner() {
        let mut bad = node("a", &[]);
        bad.owner = None;
        let d = doc_with_nodes(vec![bad]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(err, FlowError::MissingRequiredField(_, _, "owner")));
    }

    #[test]
    fn test_channel_thread_missing_participants() {
        let mut n = node("a", &[]);
        n.node_type = NodeType::ChannelThread;
        n.owner = None;
        n.participants = vec![];
        let d = doc_with_nodes(vec![n]);
        let err = validate_flow_document(&d, "test").unwrap_err();
        assert!(matches!(
            err,
            FlowError::MissingRequiredField(_, _, "participants")
        ));
    }

    #[test]
    fn test_oversized_warning() {
        let d = doc_with_nodes(vec![node("a", &[])]);
        let warnings = validate_flow_for_storage(&d, 300_000);
        assert!(warnings
            .iter()
            .any(|w| matches!(w, FlowWarning::OversizedFile { .. })));
    }

    #[test]
    fn test_too_many_nodes_warning() {
        let nodes = (0..51).map(|i| node(&format!("n{i}"), &[])).collect();
        let d = doc_with_nodes(nodes);
        let warnings = validate_flow_for_storage(&d, 1000);
        assert!(warnings
            .iter()
            .any(|w| matches!(w, FlowWarning::TooManyNodes { .. })));
    }
}
```

- [ ] **Step 2: 实现 validate_flow_document**

```rust
use crate::flow::types::FlowSlug;

const MAX_FILE_SIZE: usize = 256 * 1024;
const MAX_NODE_COUNT: usize = 50;

pub fn validate_flow_document(doc: &FlowDocument, slug_in_path: &str) -> Result<(), FlowError> {
    FlowSlug::new(&doc.meta.slug).map_err(FlowError::InvalidSlug)?;
    FlowSlug::new(slug_in_path).map_err(FlowError::InvalidSlug)?;

    if doc.meta.slug != slug_in_path {
        return Err(FlowError::SlugMismatch {
            frontmatter: doc.meta.slug.clone(),
            path: slug_in_path.to_string(),
        });
    }

    let mut seen = std::collections::HashSet::new();
    for n in &doc.meta.nodes {
        if !seen.insert(n.id.clone()) {
            return Err(FlowError::DuplicateNodeId(n.id.clone()));
        }
    }

    for n in &doc.meta.nodes {
        for need in &n.needs {
            if !seen.contains(need) {
                return Err(FlowError::UnknownNeed {
                    node: n.id.clone(),
                    missing: need.clone(),
                });
            }
        }
    }

    for n in &doc.meta.nodes {
        match n.node_type {
            NodeType::AgentMention => {
                if n.owner.is_none() {
                    return Err(FlowError::MissingRequiredField(
                        n.id.clone(),
                        n.node_type.clone(),
                        "owner",
                    ));
                }
            }
            NodeType::ChannelThread => {
                if n.participants.is_empty() {
                    return Err(FlowError::MissingRequiredField(
                        n.id.clone(),
                        n.node_type.clone(),
                        "participants",
                    ));
                }
            }
            NodeType::HumanReview => {}
            NodeType::WaitForSignal => {
                if n.signal.is_none() {
                    return Err(FlowError::MissingRequiredField(
                        n.id.clone(),
                        n.node_type.clone(),
                        "signal",
                    ));
                }
            }
        }
    }

    if has_cycle(&doc.meta.nodes) {
        return Err(FlowError::Cycle);
    }

    Ok(())
}

fn has_cycle(nodes: &[FlowNode]) -> bool {
    use std::collections::{HashMap, HashSet};
    let adj: HashMap<&str, Vec<&str>> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.needs.iter().map(String::as_str).collect()))
        .collect();

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }
    let mut marks: HashMap<&str, Mark> = nodes.iter().map(|n| (n.id.as_str(), Mark::White)).collect();

    fn dfs<'a>(
        id: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        marks: &mut HashMap<&'a str, Mark>,
    ) -> bool {
        match marks.get(id).copied().unwrap_or(Mark::Black) {
            Mark::Gray => return true,
            Mark::Black => return false,
            Mark::White => {}
        }
        marks.insert(id, Mark::Gray);
        if let Some(needs) = adj.get(id) {
            for n in needs {
                if dfs(n, adj, marks) {
                    return true;
                }
            }
        }
        marks.insert(id, Mark::Black);
        false
    }

    for n in nodes {
        if dfs(n.id.as_str(), &adj, &mut marks) {
            return true;
        }
    }
    false
}
```

注意 `NodeType` 要 derive `Clone`(Task 2 已经 derive 了)。

- [ ] **Step 3: 实现 validate_flow_for_storage**

```rust
pub fn validate_flow_for_storage(doc: &FlowDocument, file_size: usize) -> Vec<FlowWarning> {
    let mut w = Vec::new();
    if file_size > MAX_FILE_SIZE {
        w.push(FlowWarning::OversizedFile {
            actual: file_size,
            limit: MAX_FILE_SIZE,
        });
    }
    if doc.meta.nodes.len() > MAX_NODE_COUNT {
        w.push(FlowWarning::TooManyNodes {
            count: doc.meta.nodes.len(),
            limit: MAX_NODE_COUNT,
        });
    }
    w
}
```

- [ ] **Step 4: 跑测试**

Run:
```bash
cargo test -p gitim-core flow::validator::tests
```

Expected: 10 passed.

- [ ] **Step 5: 跑整个 gitim-core 测试,确认没破坏其他**

Run:
```bash
cargo test -p gitim-core
```

Expected: 全绿(原 gitim-core 测试 + 22 个新增 flow 测试)。

- [ ] **Step 6: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/flow/validator.rs
git commit -m "feat(core): add flow validator (id unique / needs refs / cycle / warnings)

validate_flow_document hard-fails on schema violations;
validate_flow_for_storage emits non-fatal size + node count warnings.
Cycle detection via white/gray/black DFS.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: gitim-core responses(typed wire payloads)

**Files:**
- Modify: `crates/gitim-core/src/responses.rs`

- [ ] **Step 1: 加 flow response 类型**

在 `responses.rs` 末尾加(以 BoardSummary 等为参考样式):

```rust
use crate::flow::{FlowNode, NodeType};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowSummary {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub node_count: usize,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ListFlowsResponse {
    pub flows: Vec<FlowSummary>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowNodeSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    pub prompt: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShowFlowResponse {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub nodes: Vec<FlowNodeSummary>,
    pub raw_markdown: String,
}

impl From<&FlowNode> for FlowNodeSummary {
    fn from(n: &FlowNode) -> Self {
        Self {
            id: n.id.clone(),
            node_type: n.node_type.clone(),
            owner: n.owner.clone(),
            participants: n.participants.clone(),
            needs: n.needs.clone(),
            prompt: n.prompt.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WriteFlowResponse {
    pub slug: String,
    pub path: String,
    pub status: String,
    pub commit_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowValidationItem {
    pub kind: String, // "error" | "warning"
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidateFlowResponse {
    pub slug: String,
    pub ok: bool,
    pub items: Vec<FlowValidationItem>,
}
```

- [ ] **Step 2: 确认 compile**

Run:
```bash
cargo check -p gitim-core
```

Expected: 0 errors。

- [ ] **Step 3: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/responses.rs
git commit -m "feat(core): add flow wire response types

ListFlows / ShowFlow / WriteFlow / ValidateFlow + summary structs.
ShowFlow 同时返回 typed nodes 和 raw_markdown,前者给前端,后者
给 agent 自己解。

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: daemon api.rs Request + Event variants

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`

- [ ] **Step 1: 读现有 Request enum 风格**

Run:
```bash
sed -n '360,385p' crates/gitim-daemon/src/api.rs
```

记住 BoardShow / BoardList / BoardInit 的 serde tag 风格(单元 vs 带字段),follow same style。

- [ ] **Step 2: 在 Request enum 加 5 个 flow variant**

定位 Request enum 里 BoardSectionAppend 之后(可用 grep `BoardSectionAppend`),加:

```rust
    FlowList,
    FlowShow {
        slug: String,
    },
    FlowCreate {
        slug: String,
        name: String,
        #[serde(default)]
        description: String,
        author: Option<String>,
    },
    FlowRemove {
        slug: String,
        author: Option<String>,
    },
    FlowValidate {
        slug: String,
    },
```

- [ ] **Step 3: Event enum 加 FlowChanged**

定位 `pub enum Event {` 块,加 variant:

```rust
    FlowChanged {
        slug: String,
    },
```

- [ ] **Step 4: 确认 compile**

Run:
```bash
cargo check -p gitim-daemon
```

预期会有未匹配的 Request 分支报错(因为 handlers/mod.rs 的 match 不全)。这是 Task 9 才解决,这里只确认 api.rs 单独编译过。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/api.rs
git commit -m "feat(daemon): add Flow* Request variants + FlowChanged event

Dispatch wiring deferred to next commit (will break build until then).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: daemon flow_handlers (5 handlers)

**Files:**
- Create: `crates/gitim-daemon/src/flow_handlers.rs`
- Modify: `crates/gitim-daemon/src/lib.rs`

- [ ] **Step 1: lib.rs 加 module**

修改 `crates/gitim-daemon/src/lib.rs`,在 `pub mod board_handlers;` 一类的行下面加:

```rust
pub mod flow_handlers;
```

- [ ] **Step 2: 创建 flow_handlers.rs 骨架**

```rust
use std::io::ErrorKind;

use crate::api::{Event, Response};
use crate::state::SharedState;
use gitim_core::flow::{
    flow_path, parse_flow_markdown, parse_flow_markdown_with_warnings, stringify_flow_markdown,
    validate_flow_document, validate_flow_for_storage, FlowDocument, FlowError, FlowMeta,
    FlowSlug, FlowWarning, NodeType,
};
use gitim_core::responses::{
    FlowNodeSummary, FlowSummary, FlowValidationItem, ListFlowsResponse, ShowFlowResponse,
    ValidateFlowResponse, WriteFlowResponse,
};

struct CommittedFlow {
    slug: String,
    path: String,
    commit_id: String,
}

pub async fn handle_flow_list(state: SharedState) -> Response {
    let root = state.repo_root.join("flows");
    let mut flows = Vec::new();

    if root.exists() {
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => return Response::error(format!("failed to list flows: {}", e)),
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(slug) = FlowSlug::new(&name) else {
                continue;
            };
            let rel = flow_path(&slug);
            let abs = state.repo_root.join(&rel);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let Ok(doc) = parse_flow_markdown(&content) else {
                continue;
            };
            if validate_flow_document(&doc, slug.as_str()).is_err() {
                continue;
            }
            flows.push(FlowSummary {
                slug: slug.to_string(),
                name: doc.meta.name,
                description: doc.meta.description,
                node_count: doc.meta.nodes.len(),
                updated_at: doc.meta.updated_at,
            });
        }
    }
    flows.sort_by(|a, b| a.slug.cmp(&b.slug));
    Response::success(serde_json::to_value(ListFlowsResponse { flows }).unwrap())
}

pub async fn handle_flow_show(state: SharedState, slug: String) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    let content = match std::fs::read_to_string(&abs) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Response::error(format!("flow not found: {}", slug));
        }
        Err(e) => return Response::error(format!("failed to read flow: {}", e)),
    };
    let doc = match parse_flow_markdown(&content) {
        Ok(d) => d,
        Err(e) => return Response::error(format!("invalid flow: {}", e)),
    };
    let payload = ShowFlowResponse {
        slug: doc.meta.slug.clone(),
        name: doc.meta.name.clone(),
        description: doc.meta.description.clone(),
        created_by: doc.meta.created_by.clone(),
        created_at: doc.meta.created_at.clone(),
        updated_at: doc.meta.updated_at.clone(),
        nodes: doc.meta.nodes.iter().map(FlowNodeSummary::from).collect(),
        raw_markdown: content,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_flow_create(
    state: SharedState,
    slug: String,
    name: String,
    description: String,
    author: String,
) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    if abs.exists() {
        return Response::error(format!("flow already exists: {}", slug));
    }

    let stub = FlowDocument {
        meta: FlowMeta {
            schema_version: 1,
            slug: slug.to_string(),
            name,
            description,
            created_by: author.clone(),
            created_at: current_timestamp(),
            updated_at: None,
            nodes: vec![],
        },
    };

    match commit_flow_document_locked(&state, &slug, stub, "flow: create", &author) {
        Ok(c) => flow_write_success(&state, c),
        Err(resp) => resp,
    }
}

pub async fn handle_flow_remove(state: SharedState, slug: String, author: String) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    let trash_rel = std::path::PathBuf::from(".trash")
        .join("flows")
        .join(slug.as_str())
        .join("index.md");
    let trash_abs = state.repo_root.join(&trash_rel);

    if !abs.exists() {
        return Response::error(format!("flow not found: {}", slug));
    }
    if let Some(parent) = trash_abs.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Response::error(format!("failed to create trash dir: {}", e));
        }
    }
    if let Err(e) = std::fs::rename(&abs, &trash_abs) {
        return Response::error(format!("failed to move to trash: {}", e));
    }
    let _ = std::fs::remove_dir(state.repo_root.join("flows").join(slug.as_str()));

    let from = rel.to_string_lossy().to_string();
    let to = trash_rel.to_string_lossy().to_string();
    let (author_name, author_email) = state.author_for(&author);
    let commit_id = match state.git_storage.add_paths_and_commit_as(
        &[from.as_str(), to.as_str()],
        &format!("flow: remove {} @{}", slug, author),
        Some((&author_name, &author_email)),
    ) {
        Ok(c) => c,
        Err(e) => return Response::error(format!("flow remove commit failed: {}", e)),
    };

    let _ = state.event_tx.send(Event::FlowChanged {
        slug: slug.to_string(),
    });
    state.push_notify.notify_one();

    Response::success(serde_json::json!({
        "slug": slug.to_string(),
        "status": "removed",
        "commit_id": commit_id,
    }))
}

pub async fn handle_flow_validate(state: SharedState, slug: String) -> Response {
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => {
            return Response::success(serde_json::to_value(ValidateFlowResponse {
                slug,
                ok: false,
                items: vec![FlowValidationItem {
                    kind: "error".into(),
                    message: format!("invalid slug: {}", e),
                }],
            }).unwrap());
        }
    };
    let rel = flow_path(&slug);
    let abs = state.repo_root.join(&rel);
    let content = match std::fs::read_to_string(&abs) {
        Ok(c) => c,
        Err(_) => {
            return Response::success(serde_json::to_value(ValidateFlowResponse {
                slug: slug.to_string(),
                ok: false,
                items: vec![FlowValidationItem {
                    kind: "error".into(),
                    message: format!("flow not found: {}", slug),
                }],
            }).unwrap());
        }
    };
    let file_size = content.len();
    let mut items = Vec::new();
    let parse_result = parse_flow_markdown_with_warnings(&content);
    match parse_result {
        Ok((doc, warnings)) => {
            for w in warnings {
                items.push(FlowValidationItem {
                    kind: "warning".into(),
                    message: format_warning(&w),
                });
            }
            if let Err(e) = validate_flow_document(&doc, slug.as_str()) {
                items.push(FlowValidationItem {
                    kind: "error".into(),
                    message: format!("{}", e),
                });
                return Response::success(serde_json::to_value(ValidateFlowResponse {
                    slug: slug.to_string(),
                    ok: false,
                    items,
                }).unwrap());
            }
            for w in validate_flow_for_storage(&doc, file_size) {
                items.push(FlowValidationItem {
                    kind: "warning".into(),
                    message: format_warning(&w),
                });
            }
            Response::success(serde_json::to_value(ValidateFlowResponse {
                slug: slug.to_string(),
                ok: true,
                items,
            }).unwrap())
        }
        Err(e) => {
            items.push(FlowValidationItem {
                kind: "error".into(),
                message: format!("{}", e),
            });
            Response::success(serde_json::to_value(ValidateFlowResponse {
                slug: slug.to_string(),
                ok: false,
                items,
            }).unwrap())
        }
    }
}

fn commit_flow_document_locked(
    state: &SharedState,
    slug: &FlowSlug,
    mut doc: FlowDocument,
    message_prefix: &str,
    author: &str,
) -> Result<CommittedFlow, Response> {
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    doc.meta.updated_at = Some(current_timestamp());

    validate_flow_document(&doc, slug.as_str()).map_err(|e| Response::error(format!("{}", e)))?;
    let rel = flow_path(slug);
    let rendered = stringify_flow_markdown(&doc).map_err(|e| Response::error(format!("{}", e)))?;
    let abs = state.repo_root.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Response::error(format!("failed to create flow dir: {}", e)))?;
    }
    std::fs::write(&abs, rendered)
        .map_err(|e| Response::error(format!("failed to write flow: {}", e)))?;

    let path = rel.to_string_lossy().to_string();
    let message = format!("{} {} @{}", message_prefix, slug, author);
    let (author_name, author_email) = state.author_for(author);
    let commit_id = state
        .git_storage
        .add_and_commit_only_as(&path, &message, Some((&author_name, &author_email)))
        .map_err(|e| Response::error(format!("flow commit failed: {}", e)))?;

    Ok(CommittedFlow {
        slug: slug.to_string(),
        path,
        commit_id,
    })
}

fn flow_write_success(state: &SharedState, committed: CommittedFlow) -> Response {
    let _ = state.event_tx.send(Event::FlowChanged {
        slug: committed.slug.clone(),
    });
    state.push_notify.notify_one();
    let payload = WriteFlowResponse {
        slug: committed.slug,
        path: committed.path,
        status: "committed".to_string(),
        commit_id: committed.commit_id,
    };
    Response::success(serde_json::to_value(payload).unwrap())
}

fn format_warning(w: &FlowWarning) -> String {
    match w {
        FlowWarning::BodySectionMissing(id) => format!("body section missing for node: {}", id),
        FlowWarning::OrphanBodySection(id) => format!("orphan body section: {}", id),
        FlowWarning::OversizedFile { actual, limit } => {
            format!("file size {} exceeds limit {}", actual, limit)
        }
        FlowWarning::TooManyNodes { count, limit } => {
            format!("node count {} exceeds limit {}", count, limit)
        }
    }
}

fn current_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
```

- [ ] **Step 3: 看 git_storage 是否有 `add_paths_and_commit_as`**

Run:
```bash
grep -n "add_paths_and_commit_as\|add_and_commit_only_as" crates/gitim-sync/src/git.rs
```

如果不存在 `add_paths_and_commit_as`(只接受单文件的 `add_and_commit_only_as` 存在),把 `handle_flow_remove` 里的 `add_paths_and_commit_as(&[from, to], ...)` 改成两次单文件 commit 或用一个 helper。

**Fallback 实现**(如 git_storage 仅暴露单文件 commit):在 handle_flow_remove 里改用现有 helper 加两次 `git add` 然后单次 commit。具体:

```rust
// 替换上面 add_paths_and_commit_as 的调用为:
state.git_storage.stage_path(&from).map_err(|e| ...)?;
state.git_storage.stage_path(&to).map_err(|e| ...)?;
let commit_id = state.git_storage.commit_as(&format!("flow: remove {} @{}", slug, author), Some((&author_name, &author_email)))?;
```

如果连 `stage_path` / `commit_as` 都不存在,则直接调 `state.git_storage.add_and_commit_only_as(&from, ...)` 把 `from` rename 先变成"删除并 commit",然后第二次 `add_and_commit_only_as(&to, ...)` commit trash —— 双 commit。这种 fallback 是 acceptable 的 v1 trade-off。

**实施时**:打开 `crates/gitim-sync/src/git.rs`,查 `GitStorage` 的 public method 列表,选最匹配的写法。先选单 commit 路径,fallback 是双 commit。

- [ ] **Step 4: 确认 compile**

Run:
```bash
cargo check -p gitim-daemon
```

预期还会有 dispatch 缺失的错误(Task 9 才补),但 flow_handlers.rs 本身要 compile 通过。如果 git_storage API 名字对不上,根据 Step 3 调整。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/flow_handlers.rs crates/gitim-daemon/src/lib.rs
git commit -m "feat(daemon): add flow_handlers (list/show/create/remove/validate)

Follows board_handlers pattern:state.commit_lock + git_storage
single-file commit, FlowChanged event on success. Remove uses soft
delete to .trash/. Validate is non-mutating, returns structured items.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: daemon dispatch + integration test

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
- Create: `crates/gitim-daemon/tests/flow_handlers.rs`

- [ ] **Step 1: 在 handlers/mod.rs 加 5 个 match arm**

定位 `Request::BoardSectionAppend { ... } => { ... }` 之后,加:

```rust
        Request::FlowList => crate::flow_handlers::handle_flow_list(state).await,
        Request::FlowShow { slug } => crate::flow_handlers::handle_flow_show(state, slug).await,
        Request::FlowCreate {
            slug,
            name,
            description,
            author,
        } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_handlers::handle_flow_create(state, slug, name, description, resolved_author).await
        }
        Request::FlowRemove { slug, author } => {
            let resolved_author = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_handlers::handle_flow_remove(state, slug, resolved_author).await
        }
        Request::FlowValidate { slug } => crate::flow_handlers::handle_flow_validate(state, slug).await,
```

注意 resolve_author 函数名可能跟 board_handlers 用的不同(如 `resolve_board_author`)。Read `crates/gitim-daemon/src/handlers/mod.rs` 第 430-460 行附近,看 board 用的是哪个 resolver,优先用 generic `resolve_author`;如果只有 board-specialized 的,定义一个 `resolve_flow_author` 简单复用即可。

- [ ] **Step 2: 确认 daemon compile**

Run:
```bash
cargo check -p gitim-daemon
```

Expected: 0 errors.

- [ ] **Step 3: 写 daemon 集成测试**

参考现有 `crates/gitim-daemon/tests/board_handlers.rs`(或类似 test target)的 setup 模板,创建 `crates/gitim-daemon/tests/flow_handlers.rs`:

```rust
//! Flow handler 集成测试。Tempdir repo + in-process daemon state。
//! 复用 board_handlers tests 的 setup helper 模式。

use std::sync::Arc;

use gitim_core::flow::{parse_flow_markdown, FlowSlug};

// ---- 引入 daemon 私有辅助函数:可能需要从 daemon crate 的 test helpers
// 暴露,或本测试自行 setup ----

mod common;

#[tokio::test]
async fn flow_create_then_list_then_show_then_validate_then_remove() {
    let state = common::make_state().await;

    // 1. list flows on empty repo → []
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowList,
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["flows"].as_array().unwrap().len(), 0);

    // 2. create stub flow
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowCreate {
            slug: "release".into(),
            name: "Release Flow".into(),
            description: "test".into(),
            author: Some("lewis".into()),
        },
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["slug"], "release");

    // 3. list now contains it
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowList,
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["flows"][0]["slug"], "release");

    // 4. show returns raw_markdown + 0 nodes
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowShow {
            slug: "release".into(),
        },
    )
    .await;
    let body = r.expect_ok();
    assert!(body["raw_markdown"].as_str().unwrap().starts_with("---\n"));
    assert_eq!(body["nodes"].as_array().unwrap().len(), 0);

    // 5. validate → ok with no errors
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowValidate {
            slug: "release".into(),
        },
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["ok"], true);

    // 6. remove
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowRemove {
            slug: "release".into(),
            author: Some("lewis".into()),
        },
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["status"], "removed");

    // 7. list again → empty
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowList,
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["flows"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn flow_validate_reports_orphan_section_warning() {
    let state = common::make_state().await;

    // Manually drop a flow with orphan body section
    let flow_dir = state.repo_root.join("flows").join("test");
    std::fs::create_dir_all(&flow_dir).unwrap();
    let md = r#"---
schema_version: 1
slug: test
name: Test
created_by: lewis
created_at: 2026-05-12T10:00:00Z
nodes:
  - id: a
    type: agent_mention
    owner: alice
    needs: []
---

## a

prompt for a

## ghost

unrelated section
"#;
    std::fs::write(flow_dir.join("index.md"), md).unwrap();

    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowValidate {
            slug: "test".into(),
        },
    )
    .await;
    let body = r.expect_ok();
    assert_eq!(body["ok"], true);
    let items = body["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["kind"] == "warning" && i["message"].as_str().unwrap().contains("orphan")));
}

#[tokio::test]
async fn flow_create_invalid_slug_rejected() {
    let state = common::make_state().await;
    let r = gitim_daemon::handlers::route(
        state.clone(),
        gitim_daemon::api::Request::FlowCreate {
            slug: "INVALID_UPPER".into(),
            name: "x".into(),
            description: "".into(),
            author: Some("lewis".into()),
        },
    )
    .await;
    assert!(r.is_err());
}
```

注意:`tests/common/mod.rs` setup helper 在现有 board / channel 测试里已经存在;follow same module path。如果现有 helper 函数签名跟这里不一致,根据 `crates/gitim-daemon/tests/` 现有测试 import 调整 `common::make_state()` 调用。

`Response::expect_ok()` / `Response::is_err()` 也可能在测试 helpers 里;如果没有,本测试文件加 inline:

```rust
trait ResponseExt {
    fn expect_ok(&self) -> &serde_json::Value;
    fn is_err(&self) -> bool;
}

impl ResponseExt for gitim_daemon::api::Response {
    fn expect_ok(&self) -> &serde_json::Value {
        // 根据 Response 实际 shape 调整。Response 大概是
        // { ok: bool, data: Option<Value>, error: Option<String> }
        assert!(self.ok, "response error: {:?}", self.error);
        self.data.as_ref().expect("response data missing")
    }
    fn is_err(&self) -> bool {
        !self.ok
    }
}
```

如 `handlers::route` 不是 public,改成调具体 handler 函数:`flow_handlers::handle_flow_list(state).await`,bypass dispatch 层。两种都 acceptable。

- [ ] **Step 4: 跑 daemon 测试**

Run:
```bash
cargo test -p gitim-daemon --test flow_handlers
```

Expected: 3 passed.

如有 helper 路径问题,调整 import 至能 compile + 测试通过为止。

- [ ] **Step 5: 跑 daemon 全测试 confirm 没破坏其他**

Run:
```bash
cargo test -p gitim-daemon
```

Expected: 全绿。

- [ ] **Step 6: Commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/handlers/mod.rs crates/gitim-daemon/tests/flow_handlers.rs
git commit -m "feat(daemon): wire flow dispatch + add integration tests

5 flow Request variants now route to flow_handlers. Integration test
covers create → list → show → validate → remove, orphan-section
warning, invalid-slug rejection.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: watcher integration(FlowModified + flows/ recursive)

**Files:**
- Modify: `crates/gitim-sync/src/watcher.rs`
- Modify: `crates/gitim-daemon/src/main.rs`

- [ ] **Step 1: 加 FlowModified variant + flows watch**

Read `crates/gitim-sync/src/watcher.rs` 现状(本 plan 顶部已经引用),把 FileEvent enum + watch_repo body 改成:

```rust
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::mpsc;
use tracing::info;

pub enum FileEvent {
    ThreadModified(String),
    MetaModified(String),
    FlowModified(String),
}

pub async fn watch_repo(
    repo_root: &Path,
    tx: mpsc::Sender<FileEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(100);

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let _ = notify_tx.blocking_send(event);
            }
        }
    })?;

    let channels_dir = repo_root.join("channels");
    let dm_dir = repo_root.join("dm");
    let flows_dir = repo_root.join("flows");

    if channels_dir.exists() {
        watcher.watch(&channels_dir, RecursiveMode::NonRecursive)?;
    }
    if dm_dir.exists() {
        watcher.watch(&dm_dir, RecursiveMode::NonRecursive)?;
    }
    if flows_dir.exists() {
        watcher.watch(&flows_dir, RecursiveMode::Recursive)?;
    }

    info!("file watcher started");

    let repo_root = repo_root.to_path_buf();
    tokio::spawn(async move {
        let _watcher = watcher;
        while let Some(event) = notify_rx.recv().await {
            for path in event.paths {
                let rel = match path.strip_prefix(&repo_root) {
                    Ok(r) => r.to_path_buf(),
                    Err(_) => continue,
                };
                let comps: Vec<_> = rel.components().collect();

                if comps.first().and_then(|c| c.as_os_str().to_str()) == Some("flows") {
                    if let Some(slug_comp) = comps.get(1) {
                        if let Some(slug) = slug_comp.as_os_str().to_str() {
                            let _ = tx.send(FileEvent::FlowModified(slug.to_string())).await;
                            continue;
                        }
                    }
                    continue;
                }

                let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                if filename.ends_with(".thread") {
                    let name = filename.trim_end_matches(".thread").to_string();
                    let _ = tx.send(FileEvent::ThreadModified(name)).await;
                } else if filename.ends_with(".meta.yaml") {
                    let name = filename.trim_end_matches(".meta.yaml").to_string();
                    let _ = tx.send(FileEvent::MetaModified(name)).await;
                }
            }
        }
    });

    Ok(())
}
```

注意:重构了 closure 让它 capture `repo_root` 的 owned copy,以便 strip_prefix。

- [ ] **Step 2: daemon main.rs 加 FlowModified 消费分支**

Read `crates/gitim-daemon/src/main.rs:166-190`,在 MetaModified 分支之后加:

```rust
                gitim_sync::watcher::FileEvent::FlowModified(slug) => {
                    tracing::debug!("flow modified: {}", slug);
                    // 重新读 + validate + commit & sync(尽量幂等:文件已在 git 里,
                    // git_storage.add 看 diff 为空时是 no-op)
                    let flow_root = watcher_state.repo_root.join("flows").join(&slug);
                    let index_md = flow_root.join("index.md");
                    if !index_md.exists() {
                        continue;
                    }
                    match std::fs::read_to_string(&index_md) {
                        Ok(content) => {
                            let rel_path = format!("flows/{}/index.md", slug);
                            // Validate but only log warnings — don't reject human edits
                            if let Ok(doc) =
                                gitim_core::flow::parse_flow_markdown(&content)
                            {
                                if let Err(e) =
                                    gitim_core::flow::validate_flow_document(&doc, &slug)
                                {
                                    tracing::warn!(
                                        "flow {} validation failed: {} — committing anyway",
                                        slug, e
                                    );
                                }
                            }
                            // commit (no-op if no diff)
                            let (name, email) = watcher_state.author_for("system");
                            let _ = watcher_state.git_storage.add_and_commit_only_as(
                                &rel_path,
                                &format!("flow: edit {} @system", slug),
                                Some((&name, &email)),
                            );
                        }
                        Err(e) => {
                            tracing::warn!("flow {} read failed: {}", slug, e);
                        }
                    }
                    let _ = watcher_state
                        .event_tx
                        .send(gitim_daemon::api::Event::FlowChanged { slug });
                    watcher_state.push_notify.notify_one();
                }
```

注意:**self-write loop 防护**。daemon 自己 commit 文件后 watcher 会再触发一次 FlowModified,但 `git_storage.add_and_commit_only_as` 在 diff 为空时返回 no-op(已是 board 当前模式)。如果实际跑出来发现 loop,加 dedup 缓存(最后修改时间或 hash)。**这一步先按上面写,如出现 loop 在 verification task 里补**。

- [ ] **Step 3: 跑 sync + daemon 全测试**

Run:
```bash
cargo test -p gitim-sync && cargo test -p gitim-daemon
```

Expected: 全绿(可能有 sync 测试的 setup 需要确认 strip_prefix 行为,如失败定位到 watcher 测试调整 closure 的 repo_root capture)。

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-sync -p gitim-daemon
git add crates/gitim-sync/src/watcher.rs crates/gitim-daemon/src/main.rs
git commit -m "feat(daemon): wire flow file watcher

watcher.rs: FlowModified(slug) variant + flows/ recursive watch +
path-to-slug extraction.

daemon main: validate (log-only) + commit-on-edit (no-op when diff
empty, relying on git_storage idempotency) + emit FlowChanged event.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: gitim-client methods

**Files:**
- Modify: `crates/gitim-client/src/client.rs`

- [ ] **Step 1: 加 5 个 method**

定位 board_section_append 结尾(`crates/gitim-client/src/client.rs:711` 附近的 `}` 收尾),在 impl 内追加:

```rust
    pub async fn flow_list(&self) -> Result<ApiResponse, ClientError> {
        self.request("flow_list", json!({})).await
    }

    pub async fn flow_show(&self, slug: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_show", json!({ "slug": slug })).await
    }

    pub async fn flow_create(
        &self,
        slug: &str,
        name: &str,
        description: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "flow_create",
            json!({
                "slug": slug,
                "name": name,
                "description": description,
            }),
        )
        .await
    }

    pub async fn flow_remove(&self, slug: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_remove", json!({ "slug": slug })).await
    }

    pub async fn flow_validate(&self, slug: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_validate", json!({ "slug": slug })).await
    }
```

注意 method-name string `"flow_list"` 等需要跟 daemon 的 Request enum 序列化形式一致。Daemon api.rs 用 `#[serde(tag = "...")]` —— 看 BoardShow / BoardList 等是 `board_show` / `board_list` 这种 snake_case 命名(参考 client board_show 行的字符串值),Request enum 的 serde rename 已经在 api.rs 配过。如果 Request enum 是 `rename_all = "snake_case"`,则 `FlowList` → `flow_list`,无需手动 rename。

- [ ] **Step 2: 确认 compile**

Run:
```bash
cargo check -p gitim-client
```

Expected: 0 errors.

- [ ] **Step 3: Commit**

```bash
cargo fmt -p gitim-client
git add crates/gitim-client/src/client.rs
git commit -m "feat(client): add flow_* daemon IPC methods

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: gitim-cli clap + commands/flow.rs

**Files:**
- Create: `crates/gitim-cli/src/commands/flow.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`
- Modify: `crates/gitim-cli/src/main.rs`

- [ ] **Step 1: 加 module 注册**

修改 `crates/gitim-cli/src/commands/mod.rs`,在 `pub mod board;` 类似行下加:

```rust
pub mod flow;
```

- [ ] **Step 2: 在 main.rs 加 clap 子命令**

定位 `enum Commands` 里 `Board { ... }` variant,在其旁加:

```rust
    /// Flow template commands
    Flow {
        #[command(subcommand)]
        command: FlowCommands,
    },
```

然后在 `enum BoardCommands { ... }` 末尾后加:

```rust
#[derive(Subcommand)]
enum FlowCommands {
    /// List all flow templates
    List,
    /// Show a flow template (markdown + ascii DAG)
    Show {
        slug: String,
    },
    /// Create a stub flow template
    Create {
        slug: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Soft-delete a flow template (move to .trash/)
    Rm {
        slug: String,
    },
    /// Validate a flow template (schema + double-source alignment)
    Validate {
        slug: String,
    },
}
```

最后在 main.rs 的 match dispatcher 里加分支(对照 `Commands::Board { command }` 写法):

```rust
        Commands::Flow { command } => match command {
            FlowCommands::List => commands::flow::cmd_flow_list(&client, mode).await,
            FlowCommands::Show { slug } => commands::flow::cmd_flow_show(&client, mode, slug).await,
            FlowCommands::Create {
                slug,
                name,
                description,
            } => commands::flow::cmd_flow_create(&client, mode, slug, name, description).await,
            FlowCommands::Rm { slug } => commands::flow::cmd_flow_remove(&client, mode, slug).await,
            FlowCommands::Validate { slug } => {
                commands::flow::cmd_flow_validate(&client, mode, slug).await
            }
        },
```

注意 `client` 变量名 / `mode` 类型按现有 board dispatch 调整。

- [ ] **Step 3: 实现 commands/flow.rs**

```rust
use anyhow::Result;
use gitim_client::GitimClient;
use serde_json::Value;

use crate::output::OutputMode;

pub async fn cmd_flow_list(client: &GitimClient, mode: OutputMode) -> Result<()> {
    let resp = client.flow_list().await?;
    let data = resp.into_data()?;
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&data)?),
        OutputMode::Human => print_flow_list_human(&data),
    }
    Ok(())
}

pub async fn cmd_flow_show(client: &GitimClient, mode: OutputMode, slug: String) -> Result<()> {
    let resp = client.flow_show(&slug).await?;
    let data = resp.into_data()?;
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&data)?),
        OutputMode::Human => print_flow_show_human(&data),
    }
    Ok(())
}

pub async fn cmd_flow_create(
    client: &GitimClient,
    mode: OutputMode,
    slug: String,
    name: String,
    description: String,
) -> Result<()> {
    let resp = client.flow_create(&slug, &name, &description).await?;
    let data = resp.into_data()?;
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&data)?),
        OutputMode::Human => println!(
            "已创建 flow `{}` ({} 个节点)\n路径: {}\ncommit: {}\n下一步: 编辑 flows/{}/index.md 加节点",
            data["slug"].as_str().unwrap_or(""),
            0,
            data["path"].as_str().unwrap_or(""),
            data["commit_id"].as_str().unwrap_or(""),
            slug,
        ),
    }
    Ok(())
}

pub async fn cmd_flow_remove(client: &GitimClient, mode: OutputMode, slug: String) -> Result<()> {
    let resp = client.flow_remove(&slug).await?;
    let data = resp.into_data()?;
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&data)?),
        OutputMode::Human => println!("已删除 flow `{}` (移至 .trash/)", slug),
    }
    Ok(())
}

pub async fn cmd_flow_validate(client: &GitimClient, mode: OutputMode, slug: String) -> Result<()> {
    let resp = client.flow_validate(&slug).await?;
    let data = resp.into_data()?;
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&data)?),
        OutputMode::Human => print_flow_validate_human(&data),
    }
    Ok(())
}

fn print_flow_list_human(data: &Value) {
    let flows = data["flows"].as_array().cloned().unwrap_or_default();
    if flows.is_empty() {
        println!("(no flows)");
        return;
    }
    for f in flows {
        println!(
            "  {:<20} {:<30} ({} nodes)",
            f["slug"].as_str().unwrap_or(""),
            f["name"].as_str().unwrap_or(""),
            f["node_count"].as_u64().unwrap_or(0),
        );
        if let Some(desc) = f["description"].as_str() {
            if !desc.is_empty() {
                println!("    {}", desc);
            }
        }
    }
}

fn print_flow_show_human(data: &Value) {
    println!("# {} ({})", data["name"].as_str().unwrap_or(""), data["slug"].as_str().unwrap_or(""));
    if let Some(d) = data["description"].as_str() {
        if !d.is_empty() {
            println!("{}\n", d);
        }
    }
    println!("---");
    println!("DAG:");
    let nodes = data["nodes"].as_array().cloned().unwrap_or_default();
    for n in &nodes {
        let id = n["id"].as_str().unwrap_or("");
        let needs: Vec<String> = n["needs"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if needs.is_empty() {
            println!("  ⊙ {}", id);
        } else {
            println!("  → {}  (needs: {})", id, needs.join(", "));
        }
    }
    println!("---");
    println!("Raw markdown:\n{}", data["raw_markdown"].as_str().unwrap_or(""));
}

fn print_flow_validate_human(data: &Value) {
    let slug = data["slug"].as_str().unwrap_or("");
    let ok = data["ok"].as_bool().unwrap_or(false);
    println!("flow `{}`: {}", slug, if ok { "OK" } else { "FAIL" });
    let items = data["items"].as_array().cloned().unwrap_or_default();
    for it in items {
        let kind = it["kind"].as_str().unwrap_or("");
        let msg = it["message"].as_str().unwrap_or("");
        let marker = if kind == "error" { "✗" } else { "⚠" };
        println!("  {} [{}] {}", marker, kind, msg);
    }
}
```

注意:`OutputMode` / `client.flow_list().into_data()` 等函数名以现有 board 命令为准。如果 ApiResponse 没有 `into_data()`,改用直接读 `.data` 字段并 unwrap。

- [ ] **Step 4: 跑 daemon 测试 + cli compile**

Run:
```bash
cargo check -p gitim-cli
cargo test -p gitim-cli --lib
```

Expected: 0 errors,unit tests 绿。

- [ ] **Step 5: 写 e2e 测试**

参考现有 `crates/gitim-cli/tests/cli_status.rs` 模板,创建 `crates/gitim-cli/tests/flow_commands.rs`:

```rust
//! e2e CLI flow 命令测试。Spawn daemon + spawn cli binary。
//! 复用 board test 的 setup helper。

mod common;

use common::TestEnv;

#[tokio::test]
async fn flow_lifecycle_create_show_validate_remove() {
    let env = TestEnv::new_with_daemon().await;

    // create
    let out = env.run_cli(&["flow", "create", "release", "--name", "Release", "--description", "test"]);
    assert!(out.status.success(), "create failed: {:?}", out);

    // list
    let out = env.run_cli(&["flow", "list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("release"), "stdout={stdout}");

    // show
    let out = env.run_cli(&["flow", "show", "release"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Release"));

    // validate
    let out = env.run_cli(&["flow", "validate", "release"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK"));

    // rm
    let out = env.run_cli(&["flow", "rm", "release"]);
    assert!(out.status.success());

    // list again — empty
    let out = env.run_cli(&["flow", "list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(no flows)"));
}
```

`common::TestEnv` 是现有 CLI 集成测试用的 helper(看其他 `cli_*.rs` 文件 import)。如果其方法名 / 类型不匹配,根据实际调整。

- [ ] **Step 6: 跑 CLI 测试**

Run:
```bash
cargo test -p gitim-cli --test flow_commands
```

Expected: 1 passed。

- [ ] **Step 7: Commit**

```bash
cargo fmt -p gitim-cli
git add crates/gitim-cli/src/commands/flow.rs crates/gitim-cli/src/commands/mod.rs crates/gitim-cli/src/main.rs crates/gitim-cli/tests/flow_commands.rs
git commit -m "feat(cli): add gitim flow subcommands (list/show/create/rm/validate)

ASCII DAG rendering in show. Human + JSON output modes.
e2e test covers full lifecycle.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: provider system prompt 增量

**Files:**
- Modify: `crates/gitim-agent-provider/src/prompts.rs`

- [ ] **Step 1: 加 Flows 段**

定位 `default_gitim_api()` 函数(`crates/gitim-agent-provider/src/prompts.rs:347`),在 "### 状态板 (Boards)" 段结束后、"### 周期任务 (Cron)" 段开始前,插入:

```text
### 流程模板 (Flows)

Flows 是团队沉淀的 SOP 流程库 —— 每个 flow 是 git 里的 markdown 模板,frontmatter 声明节点和 needs[] 依赖关系,body 用 `## <node-id>` 给每个节点的 prompt。**模板是参考不是脚本**:有人让你"按某 flow 走"时,自己读、自己 adapt 到当前情境、自己用 thread/channel 派单、自己判断每个节点是否完成,不要把它当 DAG executor 跑。

存储路径:`flows/<slug>/index.md`。任何人(任何 agent)都能改,改完 daemon 自动 commit。

- `gitim flow list` — 看团队都有哪些 flow(slug / name / 节点数 / 描述)
- `gitim flow show <slug>` — 读完整模板(markdown 原文 + ascii DAG)
- `gitim flow validate <slug>` — schema 检查 + 双源对齐报告
- `gitim flow create <slug> --name <name>` — 创建 stub 模板(frontmatter only,body 为空)
- `gitim flow rm <slug>` — soft delete(移到 .trash/)

什么时候用 flow:做"我们以前做过这件事"的工作时(release、kickoff、incident response 等),先 `gitim flow list` 看团队有没有沉淀,有就 `gitim flow show <slug>` 看一眼再开工,没有可以做完后 `gitim flow create` 把流程沉淀下来给团队下次用。

```

- [ ] **Step 2: 跑测试 confirm 没破坏**

Run:
```bash
cargo test -p gitim-agent-provider
```

Expected: 全绿。

- [ ] **Step 3: Commit**

```bash
cargo fmt -p gitim-agent-provider
git add crates/gitim-agent-provider/src/prompts.rs
git commit -m "feat(provider): add Flows section to default_gitim_api prompt

All agents (not just coordinator) get exposed to flow tools through
this prompt. Emphasizes flows are reference templates, not executable
pipelines.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: frontend deps + types + client adapter

**Files:**
- Modify: `products/gitim/frontend/package.json`
- Modify: `products/gitim/frontend/src/lib/types.ts`
- Modify: `products/gitim/frontend/src/lib/client.ts`

- [ ] **Step 1: 加依赖**

Run(在 frontend 目录):
```bash
cd products/gitim/frontend && npm install mermaid@^11 react-markdown@^9 && cd -
```

- [ ] **Step 2: 加 TS 类型**

在 `products/gitim/frontend/src/lib/types.ts` 末尾追加:

```typescript
export type NodeType =
  | "agent_mention"
  | "channel_thread"
  | "human_review"
  | "wait_for_signal";

export interface FlowNodeSummary {
  id: string;
  type: NodeType;
  owner?: string;
  participants?: string[];
  needs?: string[];
  prompt: string;
}

export interface FlowSummary {
  slug: string;
  name: string;
  description: string;
  node_count: number;
  updated_at?: string;
}

export interface FlowDocument {
  slug: string;
  name: string;
  description: string;
  created_by: string;
  created_at: string;
  updated_at?: string;
  nodes: FlowNodeSummary[];
  raw_markdown: string;
}

export interface FlowValidationItem {
  kind: "error" | "warning";
  message: string;
}

export interface FlowValidationResult {
  slug: string;
  ok: boolean;
  items: FlowValidationItem[];
}
```

- [ ] **Step 3: 加 client adapter**

在 `products/gitim/frontend/src/lib/client.ts` 末尾(或合适的 section)追加(按现有 board / channel client 风格匹配,可能用 fetch / 也可能用一个 RPC wrapper):

```typescript
import type {
  FlowDocument,
  FlowSummary,
  FlowValidationResult,
} from "./types";

export async function listFlows(workspaceSlug: string): Promise<FlowSummary[]> {
  // workspaceSlug 是 daemon-web 现有 client.ts 用的 path prefix。
  // 实际 method 调用按现有 listBoards / listChannels 模式 mirror。
  const data = await daemonRequest(workspaceSlug, "flow_list", {});
  return data.flows ?? [];
}

export async function getFlow(workspaceSlug: string, slug: string): Promise<FlowDocument> {
  return await daemonRequest(workspaceSlug, "flow_show", { slug });
}

export async function createFlow(
  workspaceSlug: string,
  slug: string,
  name: string,
  description: string,
): Promise<void> {
  await daemonRequest(workspaceSlug, "flow_create", { slug, name, description });
}

export async function removeFlow(workspaceSlug: string, slug: string): Promise<void> {
  await daemonRequest(workspaceSlug, "flow_remove", { slug });
}

export async function validateFlow(
  workspaceSlug: string,
  slug: string,
): Promise<FlowValidationResult> {
  return await daemonRequest(workspaceSlug, "flow_validate", { slug });
}
```

**注意**:`daemonRequest` 是占位名,**必须**对齐现有 client.ts 里 `listBoards` / `listChannels` 用的 wrapper(可能叫 `apiCall` / `fetchDaemon` / 直接 axios)。`workspaceSlug` 参数也按现有 API 命名。在实施前先读 `lib/client.ts` 头部 80 行确定 wrapper 签名。

- [ ] **Step 4: 跑 type check**

Run(在 frontend 目录):
```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
```

Expected: 0 errors.

- [ ] **Step 5: Commit**

```bash
git add products/gitim/frontend/package.json products/gitim/frontend/package-lock.json products/gitim/frontend/src/lib/types.ts products/gitim/frontend/src/lib/client.ts
git commit -m "feat(frontend): add flow types + client + mermaid/react-markdown deps

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: zustand store + flows list view

**Files:**
- Create: `products/gitim/frontend/src/hooks/use-flow-store.ts`
- Create: `products/gitim/frontend/src/components/flows/flows-view.tsx`

- [ ] **Step 1: 创建 store**

```typescript
// products/gitim/frontend/src/hooks/use-flow-store.ts
import { create } from "zustand";

import {
  createFlow as apiCreateFlow,
  getFlow as apiGetFlow,
  listFlows as apiListFlows,
  removeFlow as apiRemoveFlow,
  validateFlow as apiValidateFlow,
} from "@/lib/client";
import type { FlowDocument, FlowSummary, FlowValidationResult } from "@/lib/types";

interface FlowState {
  workspaceSlug: string | null;
  flows: FlowSummary[];
  selected: FlowDocument | null;
  loading: boolean;
  error: string | null;

  setWorkspace: (slug: string) => void;
  loadFlows: () => Promise<void>;
  loadFlow: (slug: string) => Promise<void>;
  createFlow: (slug: string, name: string, description: string) => Promise<void>;
  removeFlow: (slug: string) => Promise<void>;
  validateFlow: (slug: string) => Promise<FlowValidationResult>;
}

export const useFlowStore = create<FlowState>((set, get) => ({
  workspaceSlug: null,
  flows: [],
  selected: null,
  loading: false,
  error: null,

  setWorkspace: (slug) => set({ workspaceSlug: slug, flows: [], selected: null }),

  loadFlows: async () => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    set({ loading: true, error: null });
    try {
      const flows = await apiListFlows(ws);
      set({ flows, loading: false });
    } catch (e: any) {
      set({ error: String(e), loading: false });
    }
  },

  loadFlow: async (slug) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    set({ loading: true, error: null });
    try {
      const doc = await apiGetFlow(ws, slug);
      set({ selected: doc, loading: false });
    } catch (e: any) {
      set({ error: String(e), loading: false });
    }
  },

  createFlow: async (slug, name, description) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    await apiCreateFlow(ws, slug, name, description);
    await get().loadFlows();
  },

  removeFlow: async (slug) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    await apiRemoveFlow(ws, slug);
    await get().loadFlows();
    if (get().selected?.slug === slug) {
      set({ selected: null });
    }
  },

  validateFlow: async (slug) => {
    const ws = get().workspaceSlug;
    if (!ws) throw new Error("no workspace");
    return apiValidateFlow(ws, slug);
  },
}));
```

注意 `@/lib/client` import path 按现有项目的 tsconfig path alias;如不用 alias,改成相对 path `../lib/client`。

- [ ] **Step 2: 创建 flows-view (list panel)**

```typescript
// products/gitim/frontend/src/components/flows/flows-view.tsx
import { useEffect, useState } from "react";

import { useFlowStore } from "@/hooks/use-flow-store";

import { FlowDetail } from "./flow-detail";

export function FlowsView({ workspaceSlug }: { workspaceSlug: string }) {
  const setWorkspace = useFlowStore((s) => s.setWorkspace);
  const flows = useFlowStore((s) => s.flows);
  const selected = useFlowStore((s) => s.selected);
  const loading = useFlowStore((s) => s.loading);
  const error = useFlowStore((s) => s.error);
  const loadFlows = useFlowStore((s) => s.loadFlows);
  const loadFlow = useFlowStore((s) => s.loadFlow);
  const createFlow = useFlowStore((s) => s.createFlow);

  useEffect(() => {
    setWorkspace(workspaceSlug);
    loadFlows();
  }, [workspaceSlug, setWorkspace, loadFlows]);

  const [newSlug, setNewSlug] = useState("");
  const [newName, setNewName] = useState("");

  return (
    <div className="flex h-full">
      <aside className="w-72 border-r flex flex-col">
        <div className="p-3 border-b">
          <h2 className="font-semibold mb-2">Flows</h2>
          <div className="space-y-1">
            <input
              className="w-full border rounded px-2 py-1 text-sm"
              placeholder="slug (e.g. release)"
              value={newSlug}
              onChange={(e) => setNewSlug(e.target.value)}
            />
            <input
              className="w-full border rounded px-2 py-1 text-sm"
              placeholder="name"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <button
              className="w-full bg-blue-500 text-white rounded px-2 py-1 text-sm disabled:opacity-50"
              disabled={!newSlug || !newName}
              onClick={async () => {
                await createFlow(newSlug, newName, "");
                setNewSlug("");
                setNewName("");
              }}
            >
              + Create
            </button>
          </div>
        </div>
        <div className="flex-1 overflow-y-auto">
          {loading && <div className="p-3 text-sm text-gray-500">Loading...</div>}
          {error && <div className="p-3 text-sm text-red-500">{error}</div>}
          {flows.map((f) => (
            <button
              key={f.slug}
              className={`block w-full text-left px-3 py-2 hover:bg-gray-100 ${
                selected?.slug === f.slug ? "bg-blue-50" : ""
              }`}
              onClick={() => loadFlow(f.slug)}
            >
              <div className="font-medium text-sm">{f.name}</div>
              <div className="text-xs text-gray-500">
                {f.slug} · {f.node_count} nodes
              </div>
            </button>
          ))}
        </div>
      </aside>
      <main className="flex-1 overflow-y-auto">
        {selected ? <FlowDetail doc={selected} /> : (
          <div className="p-6 text-gray-400">Select a flow to view its template</div>
        )}
      </main>
    </div>
  );
}
```

注意 className 样式按现有项目 Tailwind 约定(如有 Radix UI 组件库则替换 inline 元素)。

- [ ] **Step 3: 跑 type check**

Run:
```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
```

Expected: 0 errors(可能因 FlowDetail 未定义报错,Task 16 才修;先 stub):

```typescript
// products/gitim/frontend/src/components/flows/flow-detail.tsx (temporary stub)
import type { FlowDocument } from "@/lib/types";

export function FlowDetail({ doc }: { doc: FlowDocument }) {
  return <div className="p-6">{doc.name}</div>;
}
```

- [ ] **Step 4: Commit**

```bash
git add products/gitim/frontend/src/hooks/use-flow-store.ts products/gitim/frontend/src/components/flows/
git commit -m "feat(frontend): add flow zustand store + list view

FlowDetail is a stub here (full impl in next commit).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 16: flow-detail (mermaid lazy + markdown) + nav tab

**Files:**
- Create: `products/gitim/frontend/src/components/flows/flow-dag.tsx`
- Modify: `products/gitim/frontend/src/components/flows/flow-detail.tsx`
- Modify: navigation root component(找 Boards / Channels tab 注册处)

- [ ] **Step 1: 创建 mermaid lazy 组件**

```typescript
// products/gitim/frontend/src/components/flows/flow-dag.tsx
import { lazy, Suspense, useEffect, useId, useRef } from "react";
import type { FlowNodeSummary } from "@/lib/types";

const MermaidLazy = lazy(async () => {
  const m = await import("mermaid");
  return { default: m.default };
});

function buildMermaidSource(nodes: FlowNodeSummary[]): string {
  // example: flowchart TD\n  A --> B\n  B --> C
  const lines: string[] = ["flowchart TD"];
  for (const n of nodes) {
    if (!n.needs || n.needs.length === 0) {
      lines.push(`  ${n.id}["${escapeLabel(n.id)}"]`);
    }
    for (const dep of n.needs ?? []) {
      lines.push(`  ${dep} --> ${n.id}`);
    }
  }
  return lines.join("\n");
}

function escapeLabel(s: string): string {
  return s.replace(/"/g, "\\\"");
}

function MermaidRenderer({ source }: { source: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const id = useId().replace(/:/g, "_");

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const mermaid = (await import("mermaid")).default;
      mermaid.initialize({ startOnLoad: false, theme: "default" });
      try {
        const { svg } = await mermaid.render(`mermaid-${id}`, source);
        if (!cancelled && ref.current) ref.current.innerHTML = svg;
      } catch (e) {
        if (!cancelled && ref.current) {
          ref.current.textContent = `mermaid render failed: ${e}`;
        }
      }
    })();
    return () => { cancelled = true; };
  }, [id, source]);

  return <div ref={ref} className="mermaid-container" />;
}

export function FlowDAG({ nodes }: { nodes: FlowNodeSummary[] }) {
  if (nodes.length === 0) {
    return <div className="text-gray-400 italic">(no nodes)</div>;
  }
  const source = buildMermaidSource(nodes);
  return (
    <Suspense fallback={<div className="text-gray-500">Loading diagram...</div>}>
      <MermaidRenderer source={source} />
    </Suspense>
  );
}
```

- [ ] **Step 2: 实现完整 flow-detail**

```typescript
// products/gitim/frontend/src/components/flows/flow-detail.tsx
import { lazy, Suspense } from "react";

import { useFlowStore } from "@/hooks/use-flow-store";
import type { FlowDocument } from "@/lib/types";

import { FlowDAG } from "./flow-dag";

const ReactMarkdown = lazy(() => import("react-markdown"));

export function FlowDetail({ doc }: { doc: FlowDocument }) {
  const removeFlow = useFlowStore((s) => s.removeFlow);

  return (
    <div className="p-6 space-y-6">
      <header>
        <div className="flex items-center justify-between">
          <h1 className="text-2xl font-bold">{doc.name}</h1>
          <div className="flex gap-2">
            <button
              className="px-3 py-1 border rounded text-sm"
              onClick={() => {
                navigator.clipboard.writeText(`@coordinator 用 ${doc.slug}`);
              }}
              title="复制到剪贴板;粘到 channel 输入框 review 后发送"
            >
              Run this flow
            </button>
            <button
              className="px-3 py-1 border rounded text-sm text-red-600"
              onClick={() => {
                if (confirm(`Soft-delete flow "${doc.slug}"?`)) {
                  removeFlow(doc.slug);
                }
              }}
            >
              Remove
            </button>
          </div>
        </div>
        <p className="text-sm text-gray-500">
          {doc.slug} · created by @{doc.created_by} · {doc.created_at}
        </p>
        {doc.description && <p className="mt-2">{doc.description}</p>}
      </header>

      <section>
        <h2 className="text-lg font-semibold mb-2">DAG</h2>
        <div className="border rounded p-4 bg-white overflow-x-auto">
          <FlowDAG nodes={doc.nodes} />
        </div>
      </section>

      <section>
        <h2 className="text-lg font-semibold mb-2">Nodes</h2>
        <div className="space-y-4">
          {doc.nodes.map((n) => (
            <div key={n.id} className="border rounded p-4 bg-white">
              <div className="flex items-center justify-between mb-2">
                <div className="font-mono font-medium">{n.id}</div>
                <div className="text-xs text-gray-500">
                  {n.type}
                  {n.owner && ` · @${n.owner}`}
                  {n.participants && n.participants.length > 0 && (
                    <> · participants: {n.participants.map((p) => `@${p}`).join(", ")}</>
                  )}
                  {n.needs && n.needs.length > 0 && (
                    <> · needs: {n.needs.join(", ")}</>
                  )}
                </div>
              </div>
              {n.prompt ? (
                <Suspense fallback={<div className="text-gray-400">Loading...</div>}>
                  <div className="prose prose-sm max-w-none">
                    <ReactMarkdown>{n.prompt}</ReactMarkdown>
                  </div>
                </Suspense>
              ) : (
                <div className="text-gray-400 italic text-sm">(no prompt body)</div>
              )}
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
```

- [ ] **Step 3: 在 navigation 加 Flows tab**

Run 来定位 nav root:
```bash
grep -rn "Boards\|Channels" products/gitim/frontend/src/components/ | grep -v "boards/\|chat/" | head -10
```

找到 nav 注册位置(类似 `<TabsList>` 或 sidebar item 列表),按现有 Boards / Channels 模式加 "Flows" tab,onClick → render `<FlowsView workspaceSlug={...} />`。

具体 nav 文件路径需要在实施时确定;典型位置 `products/gitim/frontend/src/components/app-shell.tsx` 或 `products/gitim/frontend/src/components/sidebar.tsx`。Mirror 现有 Boards tab 的写法。

- [ ] **Step 4: 跑 dev server + manual smoke test**

Run(在 frontend 目录):
```bash
cd products/gitim/frontend && npm run dev
```

打开浏览器到 dev URL,confirm:
- Flows tab 出现在 nav
- 点击进入 Flows view,显示 list panel(可能 empty)
- 用 list panel 的 create form 创建 `release` flow
- list 出现 `release`
- 点击 `release` → detail 显示 mermaid DAG(0 nodes 时 fallback 显示 "(no nodes)")
- 直接 vim 改 `flows/release/index.md` 加 2 个节点 + needs,刷新 frontend → DAG 渲染出来 + markdown prompt 渲染
- 点 "Run this flow" → 剪贴板里有 `@coordinator 用 release`
- 点 "Remove" → flow 被删,list 空

如果 ReactMarkdown / mermaid 报错,看 console & adjust import path / config。

- [ ] **Step 5: 跑 type check**

Run:
```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
```

Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add products/gitim/frontend/src/components/flows/ products/gitim/frontend/src/components/<nav-root>.tsx
git commit -m "feat(frontend): add flow detail view (mermaid DAG + react-markdown) + nav tab

mermaid lazy-imported via dynamic import to keep main bundle clean.
'Run this flow' copies '@coordinator 用 <slug>' to clipboard for human
review before sending.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 17: final verification + baseline tests

**Files:** (none modified — pure verification)

- [ ] **Step 1: 跑完整 workspace 测试**

Run:
```bash
cargo test --workspace
```

Expected: 全绿。当前 baseline 是 700+ tests,加上 ~30 个新增 flow tests,total ~730+。

如有失败:定位 failing test,修正,re-run。**不要标 task complete 直到 cargo test --workspace 是绿的**。

- [ ] **Step 2: 跑 frontend type check + build**

Run:
```bash
cd products/gitim/frontend && npx tsc --noEmit && npm run build && cd -
```

Expected: 0 type errors,build 成功,bundle stats 合理(mermaid lazy chunk 单独 split,不污染 main bundle 超过 +50KB)。

- [ ] **Step 3: 手动 e2e dogfood**

启动一个 workspace,跑 daemon + agent runtime,确认:
1. agent 在 system prompt 里能看到 `### 流程模板 (Flows)` 段
2. `gitim flow create test --name "Test Flow"` 成功
3. `gitim flow list` 显示
4. 手动 vim 改 `flows/test/index.md` 加 2 个节点
5. daemon log 显示 `flow modified: test` + commit
6. `gitim flow validate test` 报告 ok
7. 在 WebUI Flows tab 看到 DAG 渲染 + 节点 prompt
8. agent 在 IM 里被 `@coordinator 用 test` 时能 `gitim flow show test` 读到模板内容

如果 self-write loop 触发(daemon log 显示 `flow modified: test` 无限重复),加 dedup:在 main.rs 的 FlowModified consumer 用 HashMap 记录最近 5s commit 过的 slug,跳过。

- [ ] **Step 4: 确认无残留 fmt 噪音**

Run:
```bash
cargo fmt --check --all
```

Expected: 0 diff.

- [ ] **Step 5: 看 git log 整理**

Run:
```bash
git log --oneline main..HEAD
```

预期 ~17 commits,信息清晰。若有 fixup commits 想 squash,这是时机(按 SOP 提示用户决定是否 squash before merge,或在 finishing-a-development-branch 阶段)。

- [ ] **Step 6: 更新 CLAUDE.md 的 "Current Orientation" 段**

Read `CLAUDE.md` 顶部,在 "Where we are" 段加一句:

```text
**Team Flows v1** 已落地:`flows/<slug>/index.md` 模板系统 —— frontmatter 描述 DAG + body section 给节点 prompt;daemon flow_handlers + file watcher recursive watch flows/ + ascii/mermaid 双端 DAG 渲染;agent 通过 default_gitim_api prompt 自动暴露 flows.list/show/validate;Phase 2 fork instance + executor + conditional 留 schema 位但 v1 不实现。
```

把 "Where we're going" 里 "team flows(待实现)" 之类的项目划掉。

```bash
git add CLAUDE.md
git commit -m "docs(claude): record team-flows v1 landing

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 7: 准备 PR(由 finishing-a-development-branch skill 接管)**

Plan 到这里 task 收尾,后续 PR 创建 + review 由 `superpowers:finishing-a-development-branch` 接管。

---

## Self-Review Checklist(plan 写完后自审)

- [x] **Spec coverage**:design 的 9 个章节都有 task 覆盖:
  - 节点类型 (Task 2 NodeType enum)
  - File Structure (Task 8 flow_handlers handle_flow_create 写 `flows/<slug>/index.md`)
  - Schema (Task 2 + Task 3 frontmatter + body)
  - DAG (Task 5 cycle 检测 + Task 12 ascii + Task 16 mermaid)
  - v1 接口 CLI (Task 12) + WebUI (Task 15+16) + Agent IPC (Task 7-9) + Provider prompt (Task 13)
  - 写入路径 (Task 8 + Task 10 watcher)
  - Validation (Task 5)
  - v2 占位 (Task 2 exits 字段 optional + 默认不读)
  - 复用基线 (整 plan 都对齐 board/channel 模式)
- [x] **Placeholder scan**:无 TBD / TODO 等。"按现有 board / channel 调整" 类 contextual 指令是允许的(不是 placeholder,是 inputs)。
- [x] **Type consistency**:`FlowSlug` / `FlowMeta` / `FlowDocument` / `FlowNode` / `NodeType` / `FlowWarning` / `FlowError` 命名一致。`flow_path()` helper 在 Task 1 定义并在 Task 8 使用。`commit_flow_document_locked` 跟 board 的 `commit_board_document_locked` 命名对称。`handle_flow_*` 函数名 follow `handle_board_*` 模式。
