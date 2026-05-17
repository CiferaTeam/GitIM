# Team Flows v1.5 — Runs + Channel Binding + Node State

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 v1 template-only flows 之上加 run 层 —— `flows/<slug>/runs/<run_id>/state.yaml` 跟踪一次具体执行的状态:run_id 唯一标识、绑一个 channel、记录每个节点的 5 状态机(`pending → in_progress → done | failed | skipped`),解决 v1 "悬空看不到、状态没沉淀" 的缺陷。

**Architecture:** Run 是 template 的实例化:`gitim flow start <slug> --channel <ch>` 生成新 `run_id`(`YYYYMMDDTHHMMSS-<6hex>`),snapshot template 的节点 ID 列表到 `state.yaml`(全 pending),写到 `flows/<slug>/runs/<run_id>/state.yaml`。Agent 通过 `gitim flow node-set` 推进每节点状态;所有节点终态(done|skipped|failed)→ daemon 自动 flip `run.status`(done 如全 done/skipped、failed 如有任一 failed)。Channel 绑定是 1 run ↔ 1 channel(必绑),1 channel ↔ 0..N runs(查询走 grep state.yaml.channel)。复用 v1 的 commit_lock + git_storage.add_and_commit_only_as 单文件 commit 模式。

**Tech Stack:** Rust (gitim-core/daemon/sync/client/cli + agent-provider) + TypeScript (React 19 + Zustand + mermaid lazy)。复用 v1 已建好的 dispatch / watcher / HTTP gateway / WebUI 骨架。

**Spec:** v1 design [docs/design/team-flows-design.md](../../design/team-flows-design.md) 的 "v2 占位" 段,本 plan 把 fork instance + 节点 status 落到 v1.5。

---

## File Structure

### gitim-core (新增)

| 文件 | 责任 |
|---|---|
| `crates/gitim-core/src/flow/run.rs` | `RunId`、`RunStatus`、`NodeStatus` enum、`FlowRun`、`FlowRunNode` 类型 + `parse_run_state` / `stringify_run_state` + 验证 |

### gitim-core (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-core/src/flow/mod.rs` | `pub mod run;` + re-export `RunId, RunStatus, NodeStatus, FlowRun, FlowRunNode, run_path, parse_run_state, stringify_run_state, validate_node_transition` |
| `crates/gitim-core/src/responses.rs` | 加 `StartFlowRunResponse`、`FlowRunSummary`、`ListFlowRunsResponse`、`ShowFlowRunResponse`、`FlowRunNodeSummary`、`UpdateFlowNodeResponse`、`CancelFlowRunResponse` |

### gitim-daemon (新增)

| 文件 | 责任 |
|---|---|
| `crates/gitim-daemon/src/flow_run_handlers.rs` | 5 handler: `handle_flow_run_start`、`handle_flow_run_list`、`handle_flow_run_show`、`handle_flow_node_set`、`handle_flow_run_cancel` + `commit_run_state_locked` + auto-complete 逻辑 |
| `crates/gitim-daemon/tests/flow_run_handlers.rs` | tempdir 集成测试: e2e start → node-set → 自动 done、cancel、并发 runs、template drift |

### gitim-daemon (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-daemon/src/api.rs` | `Request` 加 `FlowRunStart`、`FlowRunList`、`FlowRunShow`、`FlowNodeSet`、`FlowRunCancel`;`Event` 加 `FlowRunStarted { run_id }`、`FlowRunNodeUpdated { run_id, node_id }`、`FlowRunCompleted { run_id, status }` |
| `crates/gitim-daemon/src/handlers/mod.rs` | dispatch 5 个新 Request;`is_write` guard 加 Start/NodeSet/Cancel |
| `crates/gitim-daemon/src/lib.rs` | `pub mod flow_run_handlers;` |

### gitim-client (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-client/src/client.rs` | 加 `flow_run_start`、`flow_run_list`、`flow_run_show`、`flow_node_set`、`flow_run_cancel` 5 method |

### gitim-cli (新增 + 修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-cli/src/commands/flow.rs` | 加 `cmd_flow_run_start`、`cmd_flow_runs`、`cmd_flow_run_show`、`cmd_flow_node_set`、`cmd_flow_run_cancel` |
| `crates/gitim-cli/src/main.rs` | `FlowCommands` enum 加 5 个 variant (`Start`、`Runs`、`RunShow`、`NodeSet`、`RunCancel`) + dispatch |

### gitim-runtime (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-runtime/src/http.rs` | 加 5 路由: `POST /im/flows/:slug/runs`、`GET /im/runs` (query: channel/slug/status)、`GET /im/runs/:run_id`、`PATCH /im/runs/:run_id/nodes/:node_id`、`DELETE /im/runs/:run_id` |

### gitim-agent-provider (修改)

| 文件 | 修改点 |
|---|---|
| `crates/gitim-agent-provider/src/prompts.rs` | `default_gitim_api()` Flows 段下追加 "触发 flow 时" 的 run 契约段(start → node-set → cancel) |

### frontend (新增 + 修改)

| 文件 | 责任 |
|---|---|
| `products/gitim/frontend/src/lib/types.ts` | `RunStatus`、`NodeStatus`、`FlowRunSummary`、`FlowRunNodeSummary`、`FlowRunDetail` TS 类型 |
| `products/gitim/frontend/src/lib/client.ts` | `startFlowRun`、`listFlowRuns`、`getFlowRun`、`updateFlowNode`、`cancelFlowRun` |
| `products/gitim/frontend/src/hooks/use-flow-run-store.ts` | zustand store(runs by channel、selected run detail、actions) |
| `products/gitim/frontend/src/components/flows/run-detail.tsx` | run 详情页:mermaid DAG + per-node 颜色 + 节点列表 |
| `products/gitim/frontend/src/components/flows/channel-active-runs.tsx` | 嵌入 channel 视图顶部的 "Active runs" 横条 |
| `products/gitim/frontend/src/components/flows/flow-detail.tsx` | (修改) flow 详情页底部追加 "Recent runs" section |
| `products/gitim/frontend/src/app.tsx` | (修改) 加 `/runs/:runId` 路由 |
| `products/gitim/frontend/src/components/chat/<channel-view>.tsx` | (修改) 顶部插入 `<ChannelActiveRuns channel={slug} />` |

---

## Task 1: gitim-core RunId + RunStatus + NodeStatus types

**Files:**
- Create: `crates/gitim-core/src/flow/run.rs`
- Modify: `crates/gitim-core/src/flow/mod.rs`

- [ ] **Step 1: 创建 run.rs 骨架 + 失败测试**

```rust
// crates/gitim-core/src/flow/run.rs
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RunIdError {
    #[error("run id is empty")]
    Empty,
    #[error("run id does not match YYYYMMDDTHHMMSS-XXXXXX pattern")]
    Format,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunId(String);

impl RunId {
    pub fn new(s: &str) -> Result<Self, RunIdError> {
        if s.is_empty() {
            return Err(RunIdError::Empty);
        }
        if !is_valid_run_id(s) {
            return Err(RunIdError::Format);
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 生成新的 run id:`YYYYMMDDTHHMMSS-XXXXXX`(XXXXXX = 6 hex chars)
    pub fn generate() -> Self {
        let now = chrono::Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%S");
        let mut hash_bytes = [0u8; 3];
        getrandom::getrandom(&mut hash_bytes).expect("getrandom failed");
        let hash = hex::encode(hash_bytes);
        Self(format!("{}-{}", timestamp, hash))
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn is_valid_run_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 22 {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        let ok = match i {
            0..=7 => b.is_ascii_digit(),
            8 => b == b'T',
            9..=14 => b.is_ascii_digit(),
            15 => b == b'-',
            16..=21 => b.is_ascii_hexdigit() && (b.is_ascii_digit() || b.is_ascii_lowercase()),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    InProgress,
    Done,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, RunStatus::Done | RunStatus::Failed | RunStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Pending,
    InProgress,
    Done,
    Failed,
    Skipped,
}

impl NodeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, NodeStatus::Done | NodeStatus::Failed | NodeStatus::Skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_id_generate_is_valid() {
        let id = RunId::generate();
        let parsed = RunId::new(id.as_str()).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_run_id_pattern_accepts_valid() {
        for s in &[
            "20260517T103045-a1b2c3",
            "00000000T000000-000000",
            "99991231T235959-ffffff",
        ] {
            assert!(RunId::new(s).is_ok(), "expected {} ok", s);
        }
    }

    #[test]
    fn test_run_id_pattern_rejects_invalid() {
        for s in &[
            "",
            "not-a-run-id",
            "20260517T103045a1b2c3",       // missing dash
            "20260517T103045-A1B2C3",      // uppercase hex
            "20260517T103045-a1b2c",       // too short hash
            "20260517T103045-a1b2c3d",     // too long hash
            "20260517 103045-a1b2c3",      // space instead of T
        ] {
            assert!(RunId::new(s).is_err(), "expected {} err", s);
        }
    }

    #[test]
    fn test_run_status_terminal() {
        assert!(!RunStatus::InProgress.is_terminal());
        assert!(RunStatus::Done.is_terminal());
        assert!(RunStatus::Failed.is_terminal());
        assert!(RunStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_node_status_terminal() {
        assert!(!NodeStatus::Pending.is_terminal());
        assert!(!NodeStatus::InProgress.is_terminal());
        assert!(NodeStatus::Done.is_terminal());
        assert!(NodeStatus::Failed.is_terminal());
        assert!(NodeStatus::Skipped.is_terminal());
    }

    #[test]
    fn test_serde_snake_case() {
        let json = serde_json::to_string(&RunStatus::InProgress).unwrap();
        assert_eq!(json, "\"in_progress\"");
        let json = serde_json::to_string(&NodeStatus::Pending).unwrap();
        assert_eq!(json, "\"pending\"");
    }
}
```

- [ ] **Step 2: 加 mod.rs 注册**

修改 `crates/gitim-core/src/flow/mod.rs`,在已有 re-exports 后追加:

```rust
pub mod run;

pub use run::{
    NodeStatus, RunId, RunIdError, RunStatus,
};
```

- [ ] **Step 3: 确认 Cargo.toml 有 `getrandom` 和 `hex` deps**

```bash
grep -E "getrandom|hex" crates/gitim-core/Cargo.toml
```

If either is missing, add to `[dependencies]`:
```toml
getrandom = "0.2"
hex = "0.4"
```

(`chrono` already a dep.)

- [ ] **Step 4: 跑测试**

```bash
cargo test -p gitim-core flow::run::tests
```

Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/flow/ crates/gitim-core/Cargo.toml
git commit -m "feat(core): add RunId + RunStatus + NodeStatus types

RunId pattern YYYYMMDDTHHMMSS-XXXXXX (6 lowercase hex).
NodeStatus: pending|in_progress|done|failed|skipped — 5 states.
RunStatus: in_progress|done|failed|cancelled — 4 states.
is_terminal() helper on both.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: FlowRun + FlowRunNode types + parser

**Files:**
- Modify: `crates/gitim-core/src/flow/run.rs`

- [ ] **Step 1: 加 FlowRun / FlowRunNode 类型 + 测试**

在 `run.rs` 的 enum 之后、`#[cfg(test)]` 之前追加:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRunNode {
    pub id: String,
    pub status: NodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRun {
    pub schema_version: u32,
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub started_at: String,
    pub started_by: String,
    pub status: RunStatus,
    pub nodes: Vec<FlowRunNode>,
    pub updated_at: String,
}

#[derive(Error, Debug)]
pub enum FlowRunError {
    #[error("invalid run id: {0}")]
    InvalidRunId(#[from] RunIdError),
    #[error("yaml parse: {0}")]
    YamlParse(String),
    #[error("schema mismatch: expected schema_version 1, got {0}")]
    SchemaVersion(u32),
    #[error("unknown node id `{0}`")]
    UnknownNodeId(String),
    #[error("invalid status transition: {from:?} → {to:?}")]
    InvalidTransition { from: NodeStatus, to: NodeStatus },
    #[error("run is terminal ({status:?}); refuse to mutate")]
    RunTerminal { status: RunStatus },
}

pub fn run_path(slug: &str, run_id: &RunId) -> std::path::PathBuf {
    std::path::PathBuf::from("flows")
        .join(slug)
        .join("runs")
        .join(run_id.as_str())
        .join("state.yaml")
}

pub fn parse_run_state(content: &str) -> Result<FlowRun, FlowRunError> {
    let run: FlowRun = serde_yaml::from_str(content)
        .map_err(|e| FlowRunError::YamlParse(e.to_string()))?;
    if run.schema_version != 1 {
        return Err(FlowRunError::SchemaVersion(run.schema_version));
    }
    Ok(run)
}

pub fn stringify_run_state(run: &FlowRun) -> Result<String, FlowRunError> {
    serde_yaml::to_string(run).map_err(|e| FlowRunError::YamlParse(e.to_string()))
}

/// 5-state machine: pending → in_progress → done|failed|skipped.
/// pending → done|failed|skipped 直接跳也允许(adjacent skip)。
/// Once terminal, no further changes.
pub fn validate_node_transition(from: NodeStatus, to: NodeStatus) -> Result<(), FlowRunError> {
    if from == to {
        return Ok(()); // no-op allowed
    }
    use NodeStatus::*;
    let allowed = match from {
        Pending => matches!(to, InProgress | Done | Failed | Skipped),
        InProgress => matches!(to, Done | Failed | Skipped),
        Done | Failed | Skipped => false, // terminal
    };
    if allowed {
        Ok(())
    } else {
        Err(FlowRunError::InvalidTransition { from, to })
    }
}
```

在 `tests` 模块底部追加测试:

```rust
    #[test]
    fn test_run_path() {
        let id = RunId::new("20260517T103045-a1b2c3").unwrap();
        assert_eq!(
            run_path("release", &id),
            std::path::PathBuf::from("flows/release/runs/20260517T103045-a1b2c3/state.yaml")
        );
    }

    #[test]
    fn test_parse_round_trip() {
        let yaml = r#"schema_version: 1
run_id: 20260517T103045-a1b2c3
flow_slug: release
channel: release-discuss
started_at: 2026-05-17T10:30:45Z
started_by: lewis
status: in_progress
nodes:
  - id: changelog
    status: done
    actor: alice
    started_at: 2026-05-17T10:31:00Z
    completed_at: 2026-05-17T11:15:00Z
  - id: e2e
    status: pending
updated_at: 2026-05-17T11:15:00Z
"#;
        let run = parse_run_state(yaml).unwrap();
        assert_eq!(run.run_id, "20260517T103045-a1b2c3");
        assert_eq!(run.nodes.len(), 2);
        assert_eq!(run.nodes[0].status, NodeStatus::Done);
        assert_eq!(run.nodes[0].actor.as_deref(), Some("alice"));
        assert_eq!(run.nodes[1].status, NodeStatus::Pending);
        let back = stringify_run_state(&run).unwrap();
        let again = parse_run_state(&back).unwrap();
        assert_eq!(again, run);
    }

    #[test]
    fn test_parse_schema_version_mismatch() {
        let yaml = "schema_version: 2\nrun_id: 20260517T103045-a1b2c3\nflow_slug: r\nchannel: c\nstarted_at: x\nstarted_by: l\nstatus: in_progress\nnodes: []\nupdated_at: x\n";
        let err = parse_run_state(yaml).unwrap_err();
        assert!(matches!(err, FlowRunError::SchemaVersion(2)));
    }

    #[test]
    fn test_validate_transition_forward() {
        use NodeStatus::*;
        assert!(validate_node_transition(Pending, InProgress).is_ok());
        assert!(validate_node_transition(Pending, Done).is_ok());
        assert!(validate_node_transition(Pending, Skipped).is_ok());
        assert!(validate_node_transition(InProgress, Done).is_ok());
        assert!(validate_node_transition(InProgress, Failed).is_ok());
        assert!(validate_node_transition(InProgress, Skipped).is_ok());
        // no-op
        assert!(validate_node_transition(Done, Done).is_ok());
    }

    #[test]
    fn test_validate_transition_backward_rejected() {
        use NodeStatus::*;
        for (f, t) in &[
            (InProgress, Pending),
            (Done, Pending),
            (Done, InProgress),
            (Done, Failed),
            (Failed, Done),
            (Skipped, Done),
        ] {
            let err = validate_node_transition(*f, *t).unwrap_err();
            assert!(matches!(err, FlowRunError::InvalidTransition { .. }));
        }
    }

    #[test]
    fn test_skip_optional_fields_serialize() {
        let node = FlowRunNode {
            id: "n".into(),
            status: NodeStatus::Pending,
            actor: None,
            started_at: None,
            completed_at: None,
            result_ref: None,
        };
        let yaml = serde_yaml::to_string(&node).unwrap();
        assert!(yaml.contains("id: n"), "yaml={yaml}");
        assert!(yaml.contains("status: pending"), "yaml={yaml}");
        assert!(!yaml.contains("actor"), "yaml={yaml}");
        assert!(!yaml.contains("started_at"), "yaml={yaml}");
        assert!(!yaml.contains("completed_at"), "yaml={yaml}");
        assert!(!yaml.contains("result_ref"), "yaml={yaml}");
    }
```

- [ ] **Step 2: 更新 mod.rs re-exports**

```rust
pub use run::{
    parse_run_state, run_path, stringify_run_state, validate_node_transition,
    FlowRun, FlowRunError, FlowRunNode, NodeStatus, RunId, RunIdError, RunStatus,
};
```

- [ ] **Step 3: 跑测试**

```bash
cargo test -p gitim-core flow::run::tests
```

Expected: 11 passed (6 from Task 1 + 5 new).

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-core
git add crates/gitim-core/src/flow/ 
git commit -m "feat(core): add FlowRun + FlowRunNode + parser/stringifier + validator

State machine: pending → in_progress → done|failed|skipped (forward-only).
schema_version 1 enforced. YAML round-trip preserves all fields.
Optional fields (actor, timestamps, result_ref) skip-serialize when None.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire response types

**Files:**
- Modify: `crates/gitim-core/src/responses.rs`

- [ ] **Step 1: 追加 7 个 response 类型**

文件末尾追加(放在 v1 flow responses 之后,前面已有 `use crate::flow::{...}`,扩展 import):

```rust
use crate::flow::{FlowRun, FlowRunNode, NodeStatus, RunStatus};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StartFlowRunResponse {
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub path: String,
    pub commit_id: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FlowRunSummary {
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub status: RunStatus,
    pub started_by: String,
    pub started_at: String,
    pub updated_at: String,
    pub node_count: usize,
    pub nodes_done: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ListFlowRunsResponse {
    pub runs: Vec<FlowRunSummary>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FlowRunNodeSummary {
    pub id: String,
    pub status: NodeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<String>,
}

impl From<&FlowRunNode> for FlowRunNodeSummary {
    fn from(n: &FlowRunNode) -> Self {
        Self {
            id: n.id.clone(),
            status: n.status,
            actor: n.actor.clone(),
            started_at: n.started_at.clone(),
            completed_at: n.completed_at.clone(),
            result_ref: n.result_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShowFlowRunResponse {
    pub run_id: String,
    pub flow_slug: String,
    pub channel: String,
    pub started_at: String,
    pub started_by: String,
    pub status: RunStatus,
    pub updated_at: String,
    pub nodes: Vec<FlowRunNodeSummary>,
}

impl From<&FlowRun> for ShowFlowRunResponse {
    fn from(r: &FlowRun) -> Self {
        Self {
            run_id: r.run_id.clone(),
            flow_slug: r.flow_slug.clone(),
            channel: r.channel.clone(),
            started_at: r.started_at.clone(),
            started_by: r.started_by.clone(),
            status: r.status,
            updated_at: r.updated_at.clone(),
            nodes: r.nodes.iter().map(FlowRunNodeSummary::from).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateFlowNodeResponse {
    pub run_id: String,
    pub node_id: String,
    pub status: NodeStatus,
    pub run_status: RunStatus,
    pub commit_id: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CancelFlowRunResponse {
    pub run_id: String,
    pub commit_id: String,
}
```

- [ ] **Step 2: cargo check + commit**

```bash
cargo check -p gitim-core
cargo fmt -p gitim-core
git add crates/gitim-core/src/responses.rs
git commit -m "feat(core): add flow run wire response types

7 new types: StartFlowRun, FlowRunSummary, ListFlowRuns, FlowRunNodeSummary,
ShowFlowRun, UpdateFlowNode, CancelFlowRun. All derive PartialEq for
round-trip test ergonomics.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: daemon api.rs Request + Event variants

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`

- [ ] **Step 1: 加 Request variants (after `FlowValidate`)**

```rust
    #[serde(rename = "flow_run_start")]
    FlowRunStart {
        slug: String,
        channel: String,
        author: Option<String>,
    },
    #[serde(rename = "flow_run_list")]
    FlowRunList {
        #[serde(default)]
        slug: Option<String>,
        #[serde(default)]
        channel: Option<String>,
        #[serde(default)]
        status: Option<String>,
    },
    #[serde(rename = "flow_run_show")]
    FlowRunShow {
        run_id: String,
    },
    #[serde(rename = "flow_node_set")]
    FlowNodeSet {
        run_id: String,
        node_id: String,
        status: String,
        #[serde(default)]
        actor: Option<String>,
        #[serde(default)]
        result_ref: Option<String>,
        author: Option<String>,
    },
    #[serde(rename = "flow_run_cancel")]
    FlowRunCancel {
        run_id: String,
        author: Option<String>,
    },
```

- [ ] **Step 2: 加 Event variants (after `FlowChanged`)**

```rust
    #[serde(rename = "flow_run_started")]
    FlowRunStarted {
        run_id: String,
        flow_slug: String,
        channel: String,
    },
    #[serde(rename = "flow_run_node_updated")]
    FlowRunNodeUpdated {
        run_id: String,
        node_id: String,
        status: String,
    },
    #[serde(rename = "flow_run_completed")]
    FlowRunCompleted {
        run_id: String,
        status: String,
    },
```

- [ ] **Step 3: 确认 compile (会有 dispatch missing error,这是预期的)**

```bash
cargo check -p gitim-daemon
```

Expected: E0004 non-exhaustive match in handlers/mod.rs(Task 6 wires)。api.rs 本身要 compile 过。

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/api.rs
git commit -m "feat(daemon): add FlowRun* Request + Event variants

Dispatch wiring deferred to next commit (will break build until then).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: daemon flow_run_handlers

**Files:**
- Create: `crates/gitim-daemon/src/flow_run_handlers.rs`
- Modify: `crates/gitim-daemon/src/lib.rs` (add `pub mod flow_run_handlers;`)

- [ ] **Step 1: 创建 flow_run_handlers.rs**

```rust
use std::io::ErrorKind;

use crate::api::{Event, Response};
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;
use gitim_core::flow::{
    flow_path, parse_flow_markdown, parse_run_state, run_path, stringify_run_state,
    validate_flow_document, validate_node_transition, FlowRun, FlowRunNode, FlowRunError,
    FlowSlug, NodeStatus, RunId, RunStatus,
};
use gitim_core::responses::{
    CancelFlowRunResponse, FlowRunNodeSummary, FlowRunSummary, ListFlowRunsResponse,
    ShowFlowRunResponse, StartFlowRunResponse, UpdateFlowNodeResponse,
};
use gitim_core::types::ChannelName;

struct CommittedRun {
    run_id: String,
    flow_slug: String,
    channel: String,
    path: String,
    commit_id: String,
}

pub async fn handle_flow_run_start(
    state: SharedState,
    slug: String,
    channel: String,
    author: String,
) -> Response {
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let slug = match FlowSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid slug: {}", e)),
    };
    let channel = match ChannelName::new(&channel) {
        Ok(c) => c,
        Err(e) => return Response::error(format!("invalid channel: {}", e)),
    };

    // validate template exists + parses
    let template_abs = state.repo_root.join(flow_path(&slug));
    let template_content = match std::fs::read_to_string(&template_abs) {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Response::error_with_code(
                format!("flow not found: {}", slug),
                "not_found",
            );
        }
        Err(e) => return Response::error(format!("failed to read flow: {}", e)),
    };
    let template = match parse_flow_markdown(&template_content) {
        Ok(t) => t,
        Err(e) => return Response::error(format!("invalid flow template: {}", e)),
    };
    if let Err(e) = validate_flow_document(&template, slug.as_str()) {
        return Response::error(format!("flow template invalid: {}", e));
    }

    // validate channel exists
    let channel_meta = state
        .repo_root
        .join(format!("channels/{}.meta.yaml", channel));
    if !channel_meta.exists() {
        return Response::error_with_code(
            format!("channel not found: {}", channel),
            "not_found",
        );
    }

    // build the run
    let run_id = RunId::generate();
    let now = current_timestamp();
    let nodes: Vec<FlowRunNode> = template
        .meta
        .nodes
        .iter()
        .map(|n| FlowRunNode {
            id: n.id.clone(),
            status: NodeStatus::Pending,
            actor: None,
            started_at: None,
            completed_at: None,
            result_ref: None,
        })
        .collect();

    let run = FlowRun {
        schema_version: 1,
        run_id: run_id.to_string(),
        flow_slug: slug.to_string(),
        channel: channel.to_string(),
        started_at: now.clone(),
        started_by: author.clone(),
        status: RunStatus::InProgress,
        nodes,
        updated_at: now,
    };

    match commit_run_state_locked(&state, &run_id, &slug, run, "flow run: start", &author) {
        Ok(c) => {
            let _ = state.event_tx.send(Event::FlowRunStarted {
                run_id: c.run_id.clone(),
                flow_slug: c.flow_slug.clone(),
                channel: c.channel.clone(),
            });
            state.push_notify.notify_one();
            Response::success(
                serde_json::to_value(StartFlowRunResponse {
                    run_id: c.run_id,
                    flow_slug: c.flow_slug,
                    channel: c.channel,
                    path: c.path,
                    commit_id: c.commit_id,
                })
                .unwrap(),
            )
        }
        Err(resp) => resp,
    }
}

pub async fn handle_flow_run_list(
    state: SharedState,
    slug_filter: Option<String>,
    channel_filter: Option<String>,
    status_filter: Option<String>,
) -> Response {
    let flows_root = state.repo_root.join("flows");
    let mut summaries = Vec::new();
    if !flows_root.exists() {
        return Response::success(serde_json::to_value(ListFlowRunsResponse { runs: vec![] }).unwrap());
    }
    let slug_entries = match std::fs::read_dir(&flows_root) {
        Ok(e) => e,
        Err(e) => return Response::error(format!("failed to list flows: {}", e)),
    };
    for slug_entry in slug_entries.flatten() {
        let slug_name = slug_entry.file_name().to_string_lossy().to_string();
        if let Some(ref filter) = slug_filter {
            if filter != &slug_name {
                continue;
            }
        }
        if FlowSlug::new(&slug_name).is_err() {
            continue;
        }
        let runs_root = slug_entry.path().join("runs");
        if !runs_root.exists() {
            continue;
        }
        let run_entries = match std::fs::read_dir(&runs_root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for run_entry in run_entries.flatten() {
            let run_name = run_entry.file_name().to_string_lossy().to_string();
            if RunId::new(&run_name).is_err() {
                continue;
            }
            let state_file = run_entry.path().join("state.yaml");
            let Ok(content) = std::fs::read_to_string(&state_file) else {
                continue;
            };
            let Ok(run) = parse_run_state(&content) else {
                continue;
            };
            if let Some(ref ch) = channel_filter {
                if &run.channel != ch {
                    continue;
                }
            }
            if let Some(ref st) = status_filter {
                let want = match st.as_str() {
                    "in_progress" => RunStatus::InProgress,
                    "done" => RunStatus::Done,
                    "failed" => RunStatus::Failed,
                    "cancelled" => RunStatus::Cancelled,
                    _ => continue,
                };
                if run.status != want {
                    continue;
                }
            }
            let nodes_done = run
                .nodes
                .iter()
                .filter(|n| n.status == NodeStatus::Done)
                .count();
            summaries.push(FlowRunSummary {
                run_id: run.run_id,
                flow_slug: run.flow_slug,
                channel: run.channel,
                status: run.status,
                started_by: run.started_by,
                started_at: run.started_at,
                updated_at: run.updated_at,
                node_count: run.nodes.len(),
                nodes_done,
            });
        }
    }
    // sort: newest first
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Response::success(serde_json::to_value(ListFlowRunsResponse { runs: summaries }).unwrap())
}

pub async fn handle_flow_run_show(state: SharedState, run_id: String) -> Response {
    let run_id_typed = match RunId::new(&run_id) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid run id: {}", e)),
    };
    let run = match find_run(&state, &run_id_typed) {
        Ok((_path, r)) => r,
        Err(resp) => return resp,
    };
    let payload: ShowFlowRunResponse = (&run).into();
    Response::success(serde_json::to_value(payload).unwrap())
}

pub async fn handle_flow_node_set(
    state: SharedState,
    run_id: String,
    node_id: String,
    status_str: String,
    actor: Option<String>,
    result_ref: Option<String>,
    author: String,
) -> Response {
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let run_id_typed = match RunId::new(&run_id) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid run id: {}", e)),
    };
    let new_status = match parse_node_status(&status_str) {
        Some(s) => s,
        None => {
            return Response::error(format!(
                "invalid status: {} (expected pending|in_progress|done|failed|skipped)",
                status_str
            ));
        }
    };

    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");

    let (run_state_path, mut run) = match find_run(&state, &run_id_typed) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    if run.status.is_terminal() {
        return Response::error(format!(
            "run is terminal ({:?}); refuse to mutate",
            run.status
        ));
    }

    let node_idx = match run.nodes.iter().position(|n| n.id == node_id) {
        Some(i) => i,
        None => {
            return Response::error_with_code(
                format!("unknown node id: {}", node_id),
                "not_found",
            );
        }
    };

    if let Err(e) = validate_node_transition(run.nodes[node_idx].status, new_status) {
        return Response::error(format!("{}", e));
    }

    let now = current_timestamp();
    let node = &mut run.nodes[node_idx];
    if node.status == NodeStatus::Pending && new_status == NodeStatus::InProgress {
        node.started_at = Some(now.clone());
    }
    if new_status.is_terminal() && node.completed_at.is_none() {
        if node.started_at.is_none() {
            node.started_at = Some(now.clone());
        }
        node.completed_at = Some(now.clone());
    }
    node.status = new_status;
    if actor.is_some() {
        node.actor = actor;
    }
    if result_ref.is_some() {
        node.result_ref = result_ref;
    }

    // auto-complete check
    let all_terminal = run.nodes.iter().all(|n| n.status.is_terminal());
    if all_terminal {
        let any_failed = run
            .nodes
            .iter()
            .any(|n| n.status == NodeStatus::Failed);
        run.status = if any_failed {
            RunStatus::Failed
        } else {
            RunStatus::Done
        };
    }
    run.updated_at = now;

    let rendered = match stringify_run_state(&run) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("stringify: {}", e)),
    };
    let path_str = run_state_path
        .strip_prefix(&state.repo_root)
        .unwrap_or(&run_state_path)
        .to_string_lossy()
        .to_string();
    let abs = state.repo_root.join(&path_str);
    if let Err(e) = std::fs::write(&abs, rendered) {
        return Response::error(format!("write: {}", e));
    }

    let (a_name, a_email) = state.author_for(&author);
    let commit_id = match state.git_storage.add_and_commit_only_as(
        &path_str,
        &format!(
            "flow run: node {} → {:?} @{}",
            node_id, new_status, author
        ),
        Some((&a_name, &a_email)),
    ) {
        Ok(id) => id,
        Err(e) => return Response::error(format!("commit: {}", e)),
    };

    let _ = state.event_tx.send(Event::FlowRunNodeUpdated {
        run_id: run.run_id.clone(),
        node_id: node_id.clone(),
        status: format!("{:?}", new_status).to_lowercase(),
    });
    if run.status.is_terminal() {
        let _ = state.event_tx.send(Event::FlowRunCompleted {
            run_id: run.run_id.clone(),
            status: format!("{:?}", run.status).to_lowercase(),
        });
    }
    state.push_notify.notify_one();

    Response::success(
        serde_json::to_value(UpdateFlowNodeResponse {
            run_id: run.run_id,
            node_id,
            status: new_status,
            run_status: run.status,
            commit_id,
        })
        .unwrap(),
    )
}

pub async fn handle_flow_run_cancel(
    state: SharedState,
    run_id: String,
    author: String,
) -> Response {
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    let run_id_typed = match RunId::new(&run_id) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("invalid run id: {}", e)),
    };

    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");

    let (run_state_path, mut run) = match find_run(&state, &run_id_typed) {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    if run.status.is_terminal() {
        return Response::error(format!(
            "run already terminal ({:?})",
            run.status
        ));
    }

    run.status = RunStatus::Cancelled;
    run.updated_at = current_timestamp();

    let rendered = match stringify_run_state(&run) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("stringify: {}", e)),
    };
    let path_str = run_state_path
        .strip_prefix(&state.repo_root)
        .unwrap_or(&run_state_path)
        .to_string_lossy()
        .to_string();
    let abs = state.repo_root.join(&path_str);
    if let Err(e) = std::fs::write(&abs, rendered) {
        return Response::error(format!("write: {}", e));
    }

    let (a_name, a_email) = state.author_for(&author);
    let commit_id = match state.git_storage.add_and_commit_only_as(
        &path_str,
        &format!("flow run: cancel {} @{}", run_id, author),
        Some((&a_name, &a_email)),
    ) {
        Ok(id) => id,
        Err(e) => return Response::error(format!("commit: {}", e)),
    };

    let _ = state.event_tx.send(Event::FlowRunCompleted {
        run_id: run.run_id.clone(),
        status: "cancelled".into(),
    });
    state.push_notify.notify_one();

    Response::success(
        serde_json::to_value(CancelFlowRunResponse {
            run_id: run.run_id,
            commit_id,
        })
        .unwrap(),
    )
}

fn commit_run_state_locked(
    state: &SharedState,
    run_id: &RunId,
    slug: &FlowSlug,
    run: FlowRun,
    message_prefix: &str,
    author: &str,
) -> Result<CommittedRun, Response> {
    let _guard = state.commit_lock.lock().expect("commit_lock poisoned");
    let rel = run_path(slug.as_str(), run_id);
    let rendered =
        stringify_run_state(&run).map_err(|e| Response::error(format!("stringify: {}", e)))?;
    let abs = state.repo_root.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Response::error(format!("mkdir: {}", e)))?;
    }
    std::fs::write(&abs, rendered)
        .map_err(|e| Response::error(format!("write: {}", e)))?;
    let path = rel.to_string_lossy().to_string();
    let (a_name, a_email) = state.author_for(author);
    let commit_id = state
        .git_storage
        .add_and_commit_only_as(
            &path,
            &format!("{} {} @{}", message_prefix, run_id, author),
            Some((&a_name, &a_email)),
        )
        .map_err(|e| Response::error(format!("commit: {}", e)))?;
    Ok(CommittedRun {
        run_id: run_id.to_string(),
        flow_slug: run.flow_slug,
        channel: run.channel,
        path,
        commit_id,
    })
}

fn find_run(
    state: &SharedState,
    run_id: &RunId,
) -> Result<(std::path::PathBuf, FlowRun), Response> {
    let flows_root = state.repo_root.join("flows");
    if !flows_root.exists() {
        return Err(Response::error_with_code(
            format!("run not found: {}", run_id),
            "not_found",
        ));
    }
    let slug_entries = std::fs::read_dir(&flows_root)
        .map_err(|e| Response::error(format!("list flows: {}", e)))?;
    for slug_entry in slug_entries.flatten() {
        let candidate = slug_entry
            .path()
            .join("runs")
            .join(run_id.as_str())
            .join("state.yaml");
        if candidate.exists() {
            let content = std::fs::read_to_string(&candidate)
                .map_err(|e| Response::error(format!("read: {}", e)))?;
            let run = parse_run_state(&content)
                .map_err(|e| Response::error(format!("parse: {}", e)))?;
            return Ok((candidate, run));
        }
    }
    Err(Response::error_with_code(
        format!("run not found: {}", run_id),
        "not_found",
    ))
}

fn parse_node_status(s: &str) -> Option<NodeStatus> {
    Some(match s {
        "pending" => NodeStatus::Pending,
        "in_progress" => NodeStatus::InProgress,
        "done" => NodeStatus::Done,
        "failed" => NodeStatus::Failed,
        "skipped" => NodeStatus::Skipped,
        _ => return None,
    })
}

fn current_timestamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}
```

- [ ] **Step 2: 更新 lib.rs**

```rust
pub mod flow_run_handlers;
```

(放在 `pub mod flow_handlers;` 旁边,alphabetical 顺序内。)

- [ ] **Step 3: 确认 compile (仍有 dispatch missing,Task 6 wire)**

```bash
cargo check -p gitim-daemon
```

预期 E0004 不变(只 add 了新 handlers 没 wire dispatch)。flow_run_handlers.rs 本身要 compile 过。

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/flow_run_handlers.rs crates/gitim-daemon/src/lib.rs
git commit -m "feat(daemon): add flow_run_handlers (start/list/show/node_set/cancel)

Mirrors flow_handlers pattern: commit_lock + single-file commit +
FlowRun* events. node_set has 5-state validation + auto-complete
(all nodes terminal → run.status = done|failed). cancel only on
non-terminal runs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: dispatch + integration tests

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
- Create: `crates/gitim-daemon/tests/flow_run_handlers.rs`

- [ ] **Step 1: 加 5 dispatch arms (after FlowValidate dispatch)**

```rust
        Request::FlowRunStart {
            slug,
            channel,
            author,
        } => {
            let resolved = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_run_handlers::handle_flow_run_start(state, slug, channel, resolved).await
        }
        Request::FlowRunList {
            slug,
            channel,
            status,
        } => {
            crate::flow_run_handlers::handle_flow_run_list(state, slug, channel, status).await
        }
        Request::FlowRunShow { run_id } => {
            crate::flow_run_handlers::handle_flow_run_show(state, run_id).await
        }
        Request::FlowNodeSet {
            run_id,
            node_id,
            status,
            actor,
            result_ref,
            author,
        } => {
            let resolved = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_run_handlers::handle_flow_node_set(
                state, run_id, node_id, status, actor, result_ref, resolved,
            )
            .await
        }
        Request::FlowRunCancel { run_id, author } => {
            let resolved = match resolve_author(author, &state).await {
                Ok(a) => a,
                Err(r) => return r,
            };
            crate::flow_run_handlers::handle_flow_run_cancel(state, run_id, resolved).await
        }
```

- [ ] **Step 2: is_write guard 加 Start/NodeSet/Cancel**

定位 `is_write` 函数(grep `fn is_write`),加这 3 个 variant 到 write 的 match arm:

```rust
        Request::FlowRunStart { .. }
        | Request::FlowNodeSet { .. }
        | Request::FlowRunCancel { .. } => true,
```

- [ ] **Step 3: 确认 daemon clean compile**

```bash
cargo check -p gitim-daemon
```

Expected: 0 errors.

- [ ] **Step 4: 创建集成测试**

参考 `crates/gitim-daemon/tests/flow_handlers.rs` 的 inline setup pattern。Create `crates/gitim-daemon/tests/flow_run_handlers.rs`:

```rust
//! Flow run handler 集成测试。复用 flow_handlers.rs 的 setup pattern。
use std::process::Command;
use std::sync::Arc;

use gitim_daemon::flow_handlers;
use gitim_daemon::flow_run_handlers;
use gitim_daemon::state::AppState;

fn git(repo: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {:?}: {:?}", args, out);
}

async fn setup() -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);

    // user
    let users_dir = root.join("users");
    std::fs::create_dir_all(&users_dir).unwrap();
    std::fs::write(
        users_dir.join("lewis.meta.yaml"),
        "handler: lewis\ndisplay_name: Lewis\n",
    )
    .unwrap();

    // channel
    let channels_dir = root.join("channels");
    std::fs::create_dir_all(&channels_dir).unwrap();
    std::fs::write(
        channels_dir.join("release-discuss.meta.yaml"),
        "name: release-discuss\ndisplay_name: Release\nintroduction: x\nmembers: [lewis]\ncreated_at: 2026-05-17T10:00:00Z\n",
    )
    .unwrap();
    std::fs::write(channels_dir.join("release-discuss.thread"), "").unwrap();

    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "init"]);

    let state = AppState::new(root.to_path_buf(), "lewis".into())
        .expect("state");

    // create a flow template
    let r = flow_handlers::handle_flow_create(
        state.clone(),
        "release".into(),
        "Release".into(),
        "test".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "flow create: {:?}", r.error);

    // write template body to add 2 nodes (changelog + e2e)
    let template_yaml = r#"---
schema_version: 1
slug: release
name: Release
description: test
created_by: lewis
created_at: 2026-05-17T10:00:00Z
updated_at: 2026-05-17T10:00:00Z
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

generate changelog

## e2e

run tests
"#;
    std::fs::write(root.join("flows/release/index.md"), template_yaml).unwrap();
    git(root, &["add", "flows/release/index.md"]);
    git(root, &["commit", "-qm", "add nodes"]);

    (dir, state)
}

#[tokio::test]
async fn run_start_then_node_set_then_auto_complete() {
    let (_dir, state) = setup().await;

    // start
    let r = flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok, "start: {:?}", r.error);
    let data = r.data.unwrap();
    let run_id = data["run_id"].as_str().unwrap().to_string();
    assert_eq!(data["flow_slug"], "release");
    assert_eq!(data["channel"], "release-discuss");

    // node-set changelog → in_progress
    let r = flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "changelog".into(),
        "in_progress".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(r.ok, "node_set in_progress: {:?}", r.error);

    // node-set changelog → done
    let r = flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "changelog".into(),
        "done".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(r.ok, "node_set done: {:?}", r.error);
    assert_eq!(r.data.as_ref().unwrap()["run_status"], "in_progress");

    // node-set e2e → done
    let r = flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "e2e".into(),
        "done".into(),
        Some("bob".into()),
        None,
        "bob".into(),
    )
    .await;
    assert!(r.ok, "node_set e2e done: {:?}", r.error);
    // auto-complete: all nodes done → run done
    assert_eq!(r.data.as_ref().unwrap()["run_status"], "done");

    // show now returns run.status = done
    let r = flow_run_handlers::handle_flow_run_show(state.clone(), run_id.clone()).await;
    assert!(r.ok);
    assert_eq!(r.data.unwrap()["status"], "done");
}

#[tokio::test]
async fn run_start_for_unknown_channel_404() {
    let (_dir, state) = setup().await;
    let r = flow_run_handlers::handle_flow_run_start(
        state,
        "release".into(),
        "no-such-channel".into(),
        "lewis".into(),
    )
    .await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn run_node_set_rejects_invalid_transition() {
    let (_dir, state) = setup().await;
    let r = flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    let run_id = r.data.unwrap()["run_id"].as_str().unwrap().to_string();

    // changelog done
    let _ = flow_run_handlers::handle_flow_node_set(
        state.clone(),
        run_id.clone(),
        "changelog".into(),
        "done".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    // try to set back to in_progress → reject
    let r = flow_run_handlers::handle_flow_node_set(
        state,
        run_id,
        "changelog".into(),
        "in_progress".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(!r.ok);
    assert!(r.error.as_deref().unwrap_or("").contains("transition"));
}

#[tokio::test]
async fn run_cancel_then_node_set_rejected() {
    let (_dir, state) = setup().await;
    let r = flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    let run_id = r.data.unwrap()["run_id"].as_str().unwrap().to_string();

    let r = flow_run_handlers::handle_flow_run_cancel(
        state.clone(),
        run_id.clone(),
        "lewis".into(),
    )
    .await;
    assert!(r.ok);

    let r = flow_run_handlers::handle_flow_node_set(
        state,
        run_id,
        "changelog".into(),
        "done".into(),
        Some("alice".into()),
        None,
        "alice".into(),
    )
    .await;
    assert!(!r.ok);
    assert!(r.error.as_deref().unwrap_or("").contains("terminal"));
}

#[tokio::test]
async fn run_list_filters_by_channel() {
    let (_dir, state) = setup().await;
    // 2 runs in release-discuss
    let _ = flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;
    let _ = flow_run_handlers::handle_flow_run_start(
        state.clone(),
        "release".into(),
        "release-discuss".into(),
        "lewis".into(),
    )
    .await;

    let r = flow_run_handlers::handle_flow_run_list(
        state.clone(),
        None,
        Some("release-discuss".into()),
        None,
    )
    .await;
    assert!(r.ok);
    let runs = r.data.unwrap()["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 2);

    let r = flow_run_handlers::handle_flow_run_list(
        state,
        None,
        Some("no-such-channel".into()),
        None,
    )
    .await;
    let runs = r.data.unwrap()["runs"].as_array().unwrap().clone();
    assert_eq!(runs.len(), 0);
}
```

- [ ] **Step 5: 跑测试**

```bash
cargo test -p gitim-daemon --test flow_run_handlers
```

Expected: 5 passed.

- [ ] **Step 6: 跑 daemon 全测**

```bash
cargo test -p gitim-daemon
```

Expected: 全绿。

- [ ] **Step 7: Commit**

```bash
cargo fmt -p gitim-daemon
git add crates/gitim-daemon/src/handlers/mod.rs crates/gitim-daemon/tests/flow_run_handlers.rs
git commit -m "feat(daemon): wire flow run dispatch + integration tests

5 tests cover: full lifecycle + auto-complete; unknown channel → 404;
invalid status transition rejected; cancelled run blocks node_set;
list filters by channel.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: gitim-client methods

**Files:**
- Modify: `crates/gitim-client/src/client.rs`

- [ ] **Step 1: 加 5 method (after flow_validate)**

```rust
    pub async fn flow_run_start(
        &self,
        slug: &str,
        channel: &str,
    ) -> Result<ApiResponse, ClientError> {
        self.request(
            "flow_run_start",
            json!({"slug": slug, "channel": channel}),
        )
        .await
    }

    pub async fn flow_run_list(
        &self,
        slug: Option<&str>,
        channel: Option<&str>,
        status: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        let mut params = serde_json::Map::new();
        if let Some(s) = slug {
            params.insert("slug".into(), json!(s));
        }
        if let Some(c) = channel {
            params.insert("channel".into(), json!(c));
        }
        if let Some(st) = status {
            params.insert("status".into(), json!(st));
        }
        self.request("flow_run_list", serde_json::Value::Object(params)).await
    }

    pub async fn flow_run_show(&self, run_id: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_run_show", json!({"run_id": run_id})).await
    }

    pub async fn flow_node_set(
        &self,
        run_id: &str,
        node_id: &str,
        status: &str,
        actor: Option<&str>,
        result_ref: Option<&str>,
    ) -> Result<ApiResponse, ClientError> {
        let mut params = serde_json::Map::new();
        params.insert("run_id".into(), json!(run_id));
        params.insert("node_id".into(), json!(node_id));
        params.insert("status".into(), json!(status));
        if let Some(a) = actor {
            params.insert("actor".into(), json!(a));
        }
        if let Some(r) = result_ref {
            params.insert("result_ref".into(), json!(r));
        }
        self.request("flow_node_set", serde_json::Value::Object(params)).await
    }

    pub async fn flow_run_cancel(&self, run_id: &str) -> Result<ApiResponse, ClientError> {
        self.request("flow_run_cancel", json!({"run_id": run_id})).await
    }
```

- [ ] **Step 2: 验证 + Commit**

```bash
cargo check -p gitim-client
cargo fmt -p gitim-client
git add crates/gitim-client/src/client.rs
git commit -m "feat(client): add flow_run_* IPC methods

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: gitim-cli subcommands

**Files:**
- Modify: `crates/gitim-cli/src/main.rs` (FlowCommands enum + dispatch)
- Modify: `crates/gitim-cli/src/commands/flow.rs` (新增 5 个 cmd 函数)

- [ ] **Step 1: 在 `FlowCommands` enum 加 5 个 variant**

定位 `enum FlowCommands` (in main.rs),加:

```rust
    /// Start a new flow run, bound to a channel
    Start {
        slug: String,
        #[arg(long)]
        channel: String,
    },
    /// List flow runs (filter by --slug / --channel / --status)
    Runs {
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        channel: Option<String>,
        #[arg(long, help = "in_progress | done | failed | cancelled")]
        status: Option<String>,
    },
    /// Show a flow run (DAG + per-node status)
    RunShow {
        run_id: String,
    },
    /// Update a node's status in a run
    NodeSet {
        run_id: String,
        node_id: String,
        #[arg(long, help = "pending|in_progress|done|failed|skipped")]
        status: String,
        #[arg(long)]
        actor: Option<String>,
        #[arg(long)]
        result_ref: Option<String>,
    },
    /// Cancel an in-progress run (terminal state)
    RunCancel {
        run_id: String,
    },
```

- [ ] **Step 2: 加 dispatch 分支**

定位 main.rs `Commands::Flow { command } => match command {`,在已有 5 分支后追加:

```rust
            FlowCommands::Start { slug, channel } => {
                commands::flow::cmd_flow_run_start(&client, &mode, slug, channel).await
            }
            FlowCommands::Runs {
                slug,
                channel,
                status,
            } => commands::flow::cmd_flow_runs(&client, &mode, slug, channel, status).await,
            FlowCommands::RunShow { run_id } => {
                commands::flow::cmd_flow_run_show(&client, &mode, run_id).await
            }
            FlowCommands::NodeSet {
                run_id,
                node_id,
                status,
                actor,
                result_ref,
            } => {
                commands::flow::cmd_flow_node_set(
                    &client,
                    &mode,
                    run_id,
                    node_id,
                    status,
                    actor,
                    result_ref,
                )
                .await
            }
            FlowCommands::RunCancel { run_id } => {
                commands::flow::cmd_flow_run_cancel(&client, &mode, run_id).await
            }
```

- [ ] **Step 3: 加 cmd functions to commands/flow.rs (末尾)**

```rust
pub async fn cmd_flow_run_start(
    client: &GitimClient,
    mode: &OutputMode,
    slug: String,
    channel: String,
) {
    let resp = match client.flow_run_start(&slug, &channel).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("network: {}", e);
            std::process::exit(1);
        }
    };
    print_or_exit(resp, mode, |data, mode| match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(data).unwrap()),
        OutputMode::Human => {
            println!(
                "已启动 flow run `{}` (flow={}, channel={})\ncommit: {}",
                data["run_id"].as_str().unwrap_or(""),
                data["flow_slug"].as_str().unwrap_or(""),
                data["channel"].as_str().unwrap_or(""),
                data["commit_id"].as_str().unwrap_or(""),
            );
        }
    });
}

pub async fn cmd_flow_runs(
    client: &GitimClient,
    mode: &OutputMode,
    slug: Option<String>,
    channel: Option<String>,
    status: Option<String>,
) {
    let resp = match client
        .flow_run_list(slug.as_deref(), channel.as_deref(), status.as_deref())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("network: {}", e);
            std::process::exit(1);
        }
    };
    print_or_exit(resp, mode, |data, mode| match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(data).unwrap()),
        OutputMode::Human => {
            let runs = data["runs"].as_array().cloned().unwrap_or_default();
            if runs.is_empty() {
                println!("(no runs)");
                return;
            }
            for r in runs {
                println!(
                    "  {} {} [{:<11}] {}/{} nodes (channel={}, started_by={})",
                    r["run_id"].as_str().unwrap_or(""),
                    r["flow_slug"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["nodes_done"].as_u64().unwrap_or(0),
                    r["node_count"].as_u64().unwrap_or(0),
                    r["channel"].as_str().unwrap_or(""),
                    r["started_by"].as_str().unwrap_or(""),
                );
            }
        }
    });
}

pub async fn cmd_flow_run_show(client: &GitimClient, mode: &OutputMode, run_id: String) {
    let resp = match client.flow_run_show(&run_id).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("network: {}", e);
            std::process::exit(1);
        }
    };
    print_or_exit(resp, mode, |data, mode| match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(data).unwrap()),
        OutputMode::Human => {
            println!(
                "run `{}` ({})\nflow: {}  channel: {}  by: {}\nstarted: {}  updated: {}\n",
                data["run_id"].as_str().unwrap_or(""),
                data["status"].as_str().unwrap_or(""),
                data["flow_slug"].as_str().unwrap_or(""),
                data["channel"].as_str().unwrap_or(""),
                data["started_by"].as_str().unwrap_or(""),
                data["started_at"].as_str().unwrap_or(""),
                data["updated_at"].as_str().unwrap_or(""),
            );
            println!("Nodes:");
            for n in data["nodes"].as_array().cloned().unwrap_or_default() {
                let id = n["id"].as_str().unwrap_or("");
                let st = n["status"].as_str().unwrap_or("");
                let actor = n["actor"].as_str().unwrap_or("-");
                let marker = match st {
                    "done" => "o",
                    "in_progress" => ">",
                    "pending" => ".",
                    "failed" => "x",
                    "skipped" => "~",
                    _ => "?",
                };
                println!("  {} [{:<11}] {}  @{}", marker, st, id, actor);
            }
        }
    });
}

pub async fn cmd_flow_node_set(
    client: &GitimClient,
    mode: &OutputMode,
    run_id: String,
    node_id: String,
    status: String,
    actor: Option<String>,
    result_ref: Option<String>,
) {
    let resp = match client
        .flow_node_set(
            &run_id,
            &node_id,
            &status,
            actor.as_deref(),
            result_ref.as_deref(),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("network: {}", e);
            std::process::exit(1);
        }
    };
    print_or_exit(resp, mode, |data, mode| match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(data).unwrap()),
        OutputMode::Human => {
            println!(
                "已更新 node `{}` → {} (run={}, run_status={})\ncommit: {}",
                data["node_id"].as_str().unwrap_or(""),
                data["status"].as_str().unwrap_or(""),
                data["run_id"].as_str().unwrap_or(""),
                data["run_status"].as_str().unwrap_or(""),
                data["commit_id"].as_str().unwrap_or(""),
            );
        }
    });
}

pub async fn cmd_flow_run_cancel(client: &GitimClient, mode: &OutputMode, run_id: String) {
    let resp = match client.flow_run_cancel(&run_id).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("network: {}", e);
            std::process::exit(1);
        }
    };
    print_or_exit(resp, mode, |data, mode| match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(data).unwrap()),
        OutputMode::Human => {
            println!(
                "已取消 flow run `{}`\ncommit: {}",
                data["run_id"].as_str().unwrap_or(""),
                data["commit_id"].as_str().unwrap_or(""),
            );
        }
    });
}
```

- [ ] **Step 4: cargo check + test**

```bash
cargo check -p gitim-cli
cargo test -p gitim-cli
```

Expected: 0 errors, all existing tests still green.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-cli
git add crates/gitim-cli/src/commands/flow.rs crates/gitim-cli/src/main.rs
git commit -m "feat(cli): add gitim flow run subcommands (start/runs/run-show/node-set/run-cancel)

Human output uses ASCII markers per node status (o=done, >=in_progress,
.=pending, x=failed, ~=skipped). Inherits Json/Human dual mode + 
process::exit(1) on error from existing flow command pattern.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: provider system prompt 增量

**Files:**
- Modify: `crates/gitim-agent-provider/src/prompts.rs`

- [ ] **Step 1: 在 v1 Flows 段末追加 run 契约**

定位 `default_gitim_api()` 里的 "### 流程模板 (Flows)" 段。在该段最后一段(`什么时候用 flow ...`)之后追加:

```text

触发一个 flow:
  1. `gitim flow start <slug> --channel <当前 channel>` —— 拿到 `run_id`,记下来
  2. 整个 run 期间在消息里带上 run_id(或 ref 当前 thread),让别人 / 你自己未来能找回
  3. 开始一个节点:`gitim flow node-set <run_id> <node-id> --status in_progress --actor <handler>`
  4. 节点完成:`--status done`(成功 / 失败:`failed` + 在 thread 里讲原因 / 跳过:`skipped`)
  5. 不记得当前 channel 里有哪些活的 run:`gitim flow runs --channel <ch> --status in_progress`
  6. 想看整个 run 现在啥样:`gitim flow run-show <run_id>` —— DAG + 各节点 status + actor
  7. 终止:所有 node 都到终态(done/failed/skipped),run 会自动 done(全 done/skipped)或 failed(任一 failed)。不可恢复要起新 run。
  8. 强制取消:`gitim flow run-cancel <run_id>`(只对未终态 run 有效)

状态机:`pending → in_progress → done | failed | skipped`。**只前向,不回退**。run.status 走 `in_progress → done | failed | cancelled`,同样只前向。
```

- [ ] **Step 2: 加 regression test**

定位 `crates/gitim-agent-provider/tests/prompt_test.rs` 里已有的 `gitim_api_exposes_flow_commands`。在其后加:

```rust
#[test]
fn gitim_api_exposes_flow_run_commands() {
    let ctx = test_context();
    let api = default_gitim_api(&ctx);
    assert!(api.contains("gitim flow start"));
    assert!(api.contains("gitim flow runs"));
    assert!(api.contains("gitim flow run-show"));
    assert!(api.contains("gitim flow node-set"));
    assert!(api.contains("gitim flow run-cancel"));
    assert!(api.contains("pending → in_progress → done"));
}
```

- [ ] **Step 3: 验证 + commit**

```bash
cargo test -p gitim-agent-provider
cargo fmt -p gitim-agent-provider
git add crates/gitim-agent-provider/src/prompts.rs crates/gitim-agent-provider/tests/prompt_test.rs
git commit -m "feat(provider): add flow run lifecycle to default agent prompt

Agent contract: start → node-set per node → auto-complete or cancel.
State machine documented in prompt (forward-only).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: runtime HTTP routes

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: 加 5 路由 (mirror flow_handlers pattern from Task 14 of v1)**

定位 `ws_router` 里 `/im/flows` 注册块,在其旁追加:

```rust
.route("/im/flows/{flow_slug}/runs", post(flows_run_start))
.route("/im/runs", get(flows_run_list))
.route(
    "/im/runs/{run_id}",
    get(flows_run_show).delete(flows_run_cancel),
)
.route("/im/runs/{run_id}/nodes/{node_id}", patch(flows_node_set))
```

加 5 个 axum handler functions(mirror `flows_create` / `flows_list` from v1 task 14 — read those for reference shape):

```rust
async fn flows_run_start(
    State(state): State<AppState>,
    Path((ws_slug, flow_slug)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(channel) = body.get("channel").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(ErrorBody::new("missing 'channel'"))).into_response();
    };
    let client = match state.client_for(&ws_slug).await {
        Ok(c) => c,
        Err(e) => return flow_client_error_to_response(e),
    };
    match client.flow_run_start(&flow_slug, channel).await {
        Ok(resp) => flow_write_response(resp),
        Err(e) => flow_client_error_to_response(e),
    }
}

async fn flows_run_list(
    State(state): State<AppState>,
    Path(ws_slug): Path<String>,
    Query(params): Query<RunListQuery>,
) -> Response {
    let client = match state.client_for(&ws_slug).await {
        Ok(c) => c,
        Err(e) => return flow_client_error_to_response(e),
    };
    match client
        .flow_run_list(
            params.slug.as_deref(),
            params.channel.as_deref(),
            params.status.as_deref(),
        )
        .await
    {
        Ok(resp) => flow_raw_data_response(resp),
        Err(e) => flow_client_error_to_response(e),
    }
}

async fn flows_run_show(
    State(state): State<AppState>,
    Path((ws_slug, run_id)): Path<(String, String)>,
) -> Response {
    let client = match state.client_for(&ws_slug).await {
        Ok(c) => c,
        Err(e) => return flow_client_error_to_response(e),
    };
    match client.flow_run_show(&run_id).await {
        Ok(resp) => flow_raw_data_response(resp),
        Err(e) => flow_client_error_to_response(e),
    }
}

async fn flows_node_set(
    State(state): State<AppState>,
    Path((ws_slug, run_id, node_id)): Path<(String, String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(status) = body.get("status").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(ErrorBody::new("missing 'status'"))).into_response();
    };
    let actor = body.get("actor").and_then(|v| v.as_str());
    let result_ref = body.get("result_ref").and_then(|v| v.as_str());
    let client = match state.client_for(&ws_slug).await {
        Ok(c) => c,
        Err(e) => return flow_client_error_to_response(e),
    };
    match client
        .flow_node_set(&run_id, &node_id, status, actor, result_ref)
        .await
    {
        Ok(resp) => flow_write_response(resp),
        Err(e) => flow_client_error_to_response(e),
    }
}

async fn flows_run_cancel(
    State(state): State<AppState>,
    Path((ws_slug, run_id)): Path<(String, String)>,
) -> Response {
    let client = match state.client_for(&ws_slug).await {
        Ok(c) => c,
        Err(e) => return flow_client_error_to_response(e),
    };
    match client.flow_run_cancel(&run_id).await {
        Ok(resp) => flow_write_response(resp),
        Err(e) => flow_client_error_to_response(e),
    }
}

#[derive(Debug, serde::Deserialize)]
struct RunListQuery {
    slug: Option<String>,
    channel: Option<String>,
    status: Option<String>,
}

fn flow_write_response(resp: ApiResponse) -> Response {
    if resp.ok {
        let data = resp.data.unwrap_or(serde_json::json!({}));
        (StatusCode::OK, Json(data)).into_response()
    } else if resp.error_code.as_deref() == Some("not_found") {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorBody::with_code(
                resp.error.unwrap_or_default(),
                resp.error_code.unwrap_or_default(),
            )),
        )
            .into_response()
    } else {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorBody::with_code(
                resp.error.unwrap_or_default(),
                resp.error_code.unwrap_or_default(),
            )),
        )
            .into_response()
    }
}
```

Note: `flow_raw_data_response` and `flow_client_error_to_response` are existing helpers from v1 task 14. `flow_write_response` is the same shape as `api_response_to_json` but with proper 404 on not_found — defined per Task 6's runtime fix pattern, included here for self-contained reuse.

- [ ] **Step 2: 加 smoke tests**

定位 `crates/gitim-runtime/tests/flow_http.rs`(v1 Task 14 加的)。在文件末追加:

```rust
#[tokio::test]
async fn flow_run_start_creates_run_with_404_on_unknown_channel() {
    // ... scripted-daemon pattern, mirroring existing flow_show_returns_404_for_missing test
    // assert POST /im/flows/release/runs with unknown channel → 404
}

#[tokio::test]
async fn flow_runs_list_returns_filtered() {
    // ... POST 2 runs, GET /im/runs?channel=release-discuss → list of 2
}
```

(Inline the test bodies following exactly the existing flow_http.rs test pattern — scripted daemon stdin/stdout simulation. Reference: tests/flow_http.rs first test.)

- [ ] **Step 3: 验证**

```bash
cargo check -p gitim-runtime
cargo test -p gitim-runtime --test flow_http
cargo test -p gitim-runtime
```

Expected: existing tests + 2 new pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p gitim-runtime
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/flow_http.rs
git commit -m "feat(runtime): add HTTP routes for flow runs

POST /im/flows/:slug/runs  → start
GET  /im/runs               → list (filter by ?slug=&channel=&status=)
GET  /im/runs/:run_id       → show
PATCH /im/runs/:rid/nodes/:nid → node set
DELETE /im/runs/:run_id     → cancel

All write endpoints return non-2xx on error (matching the v1 fix for
flow_raw_data_response). not_found error_code → 404; other errors → 422.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: frontend types + client adapter

**Files:**
- Modify: `products/gitim/frontend/src/lib/types.ts`
- Modify: `products/gitim/frontend/src/lib/client.ts`

- [ ] **Step 1: 加 TS 类型**

types.ts 文件末追加:

```typescript
export type RunStatus = "in_progress" | "done" | "failed" | "cancelled";
export type NodeStatus = "pending" | "in_progress" | "done" | "failed" | "skipped";

export interface FlowRunSummary {
  run_id: string;
  flow_slug: string;
  channel: string;
  status: RunStatus;
  started_by: string;
  started_at: string;
  updated_at: string;
  node_count: number;
  nodes_done: number;
}

export interface FlowRunNodeSummary {
  id: string;
  status: NodeStatus;
  actor?: string;
  started_at?: string;
  completed_at?: string;
  result_ref?: string;
}

export interface FlowRunDetail {
  run_id: string;
  flow_slug: string;
  channel: string;
  started_at: string;
  started_by: string;
  status: RunStatus;
  updated_at: string;
  nodes: FlowRunNodeSummary[];
}
```

- [ ] **Step 2: 加 client adapter**

client.ts 文件末追加(参考 v1 Task 14 的 listFlows 等 wrapper):

```typescript
export async function startFlowRun(
  workspaceSlug: string,
  flowSlug: string,
  channel: string,
): Promise<{ run_id: string; flow_slug: string; channel: string; commit_id: string }> {
  const res = await fetch(
    `${wsBase(workspaceSlug)}/im/flows/${encodeURIComponent(flowSlug)}/runs`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel }),
    },
  );
  if (!res.ok) throw new Error(`startFlowRun failed: ${res.status} ${await res.text()}`);
  return res.json();
}

export async function listFlowRuns(
  workspaceSlug: string,
  opts: { slug?: string; channel?: string; status?: RunStatus } = {},
): Promise<FlowRunSummary[]> {
  const qs = new URLSearchParams();
  if (opts.slug) qs.set("slug", opts.slug);
  if (opts.channel) qs.set("channel", opts.channel);
  if (opts.status) qs.set("status", opts.status);
  const url = `${wsBase(workspaceSlug)}/im/runs${qs.toString() ? "?" + qs : ""}`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`listFlowRuns failed: ${res.status}`);
  const data = await res.json();
  return data.runs ?? [];
}

export async function getFlowRun(
  workspaceSlug: string,
  runId: string,
): Promise<FlowRunDetail> {
  const res = await fetch(`${wsBase(workspaceSlug)}/im/runs/${encodeURIComponent(runId)}`);
  if (!res.ok) throw new Error(`getFlowRun failed: ${res.status}`);
  return res.json();
}

export async function updateFlowNode(
  workspaceSlug: string,
  runId: string,
  nodeId: string,
  payload: { status: NodeStatus; actor?: string; result_ref?: string },
): Promise<void> {
  const res = await fetch(
    `${wsBase(workspaceSlug)}/im/runs/${encodeURIComponent(runId)}/nodes/${encodeURIComponent(nodeId)}`,
    {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    },
  );
  if (!res.ok) throw new Error(`updateFlowNode failed: ${res.status}`);
}

export async function cancelFlowRun(workspaceSlug: string, runId: string): Promise<void> {
  const res = await fetch(`${wsBase(workspaceSlug)}/im/runs/${encodeURIComponent(runId)}`, {
    method: "DELETE",
  });
  if (!res.ok) throw new Error(`cancelFlowRun failed: ${res.status}`);
}
```

(Import the new types at the top of client.ts:`import type { FlowRunDetail, FlowRunSummary, NodeStatus, RunStatus } from "./types";`)

- [ ] **Step 3: 验证 + Commit**

```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
git add products/gitim/frontend/src/lib/types.ts products/gitim/frontend/src/lib/client.ts
git commit -m "feat(frontend): add flow run TS types + client adapter

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: frontend zustand store + run detail page

**Files:**
- Create: `products/gitim/frontend/src/hooks/use-flow-run-store.ts`
- Create: `products/gitim/frontend/src/components/flows/run-detail.tsx`
- Modify: `products/gitim/frontend/src/app.tsx` (route + nav 不变)

- [ ] **Step 1: 创建 store**

`hooks/use-flow-run-store.ts`:

```typescript
import { create } from "zustand";

import {
  cancelFlowRun as apiCancelFlowRun,
  getFlowRun as apiGetFlowRun,
  listFlowRuns as apiListFlowRuns,
  startFlowRun as apiStartFlowRun,
  updateFlowNode as apiUpdateFlowNode,
} from "@/lib/client";
import type {
  FlowRunDetail,
  FlowRunSummary,
  NodeStatus,
  RunStatus,
} from "@/lib/types";

interface FlowRunState {
  workspaceSlug: string | null;
  runsByChannel: Record<string, FlowRunSummary[]>;
  selectedRun: FlowRunDetail | null;
  loading: boolean;
  error: string | null;

  setWorkspace: (slug: string) => void;
  loadRunsForChannel: (channel: string) => Promise<void>;
  loadRun: (runId: string) => Promise<void>;
  startRun: (flowSlug: string, channel: string) => Promise<string>;
  setNodeStatus: (
    runId: string,
    nodeId: string,
    status: NodeStatus,
    actor?: string,
  ) => Promise<void>;
  cancelRun: (runId: string) => Promise<void>;
}

export const useFlowRunStore = create<FlowRunState>((set, get) => ({
  workspaceSlug: null,
  runsByChannel: {},
  selectedRun: null,
  loading: false,
  error: null,

  setWorkspace: (slug) => set({ workspaceSlug: slug, runsByChannel: {}, selectedRun: null }),

  loadRunsForChannel: async (channel) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    set({ loading: true, error: null });
    try {
      const runs = await apiListFlowRuns(ws, { channel });
      set((s) => ({
        runsByChannel: { ...s.runsByChannel, [channel]: runs },
        loading: false,
      }));
    } catch (e: any) {
      set({ error: String(e), loading: false });
    }
  },

  loadRun: async (runId) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    set({ loading: true, error: null });
    try {
      const run = await apiGetFlowRun(ws, runId);
      set({ selectedRun: run, loading: false });
    } catch (e: any) {
      set({ error: String(e), loading: false });
    }
  },

  startRun: async (flowSlug, channel) => {
    const ws = get().workspaceSlug;
    if (!ws) throw new Error("no workspace");
    const { run_id } = await apiStartFlowRun(ws, flowSlug, channel);
    // refresh channel's runs
    await get().loadRunsForChannel(channel);
    return run_id;
  },

  setNodeStatus: async (runId, nodeId, status, actor) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    await apiUpdateFlowNode(ws, runId, nodeId, { status, actor });
    await get().loadRun(runId);
  },

  cancelRun: async (runId) => {
    const ws = get().workspaceSlug;
    if (!ws) return;
    await apiCancelFlowRun(ws, runId);
    await get().loadRun(runId);
  },
}));
```

- [ ] **Step 2: 创建 run-detail.tsx**

```typescript
import { lazy, Suspense, useEffect } from "react";
import { useParams } from "react-router-dom";

import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { useFlowRunStore } from "@/hooks/use-flow-run-store";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { NodeStatus } from "@/lib/types";

const FlowDAG = lazy(() =>
  import("./flow-dag").then((m) => ({ default: m.FlowDAG })),
);

const STATUS_COLORS: Record<NodeStatus, string> = {
  pending: "text-muted-foreground",
  in_progress: "text-yellow-600",
  done: "text-green-600",
  failed: "text-red-600",
  skipped: "text-gray-400 line-through",
};

const STATUS_BG: Record<NodeStatus, string> = {
  pending: "bg-muted",
  in_progress: "bg-yellow-100 dark:bg-yellow-950",
  done: "bg-green-100 dark:bg-green-950",
  failed: "bg-red-100 dark:bg-red-950",
  skipped: "bg-muted opacity-60",
};

export function RunDetail() {
  const { runId } = useParams<{ runId: string }>();
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const setWorkspace = useFlowRunStore((s) => s.setWorkspace);
  const selected = useFlowRunStore((s) => s.selectedRun);
  const loading = useFlowRunStore((s) => s.loading);
  const error = useFlowRunStore((s) => s.error);
  const loadRun = useFlowRunStore((s) => s.loadRun);
  const cancelRun = useFlowRunStore((s) => s.cancelRun);

  useEffect(() => {
    if (activeSlug) setWorkspace(activeSlug);
    if (runId) loadRun(runId);
  }, [activeSlug, runId, setWorkspace, loadRun]);

  if (loading) return <div className="p-6 text-muted-foreground">Loading...</div>;
  if (error) return <div className="p-6 text-destructive">{error}</div>;
  if (!selected) return <div className="p-6 text-muted-foreground">Run not found.</div>;

  // build DAG nodes — for now, just the run's nodes (DAG edges come from
  // template, but we can render flat for run view)
  const dagNodes = selected.nodes.map((n) => ({
    id: n.id,
    type: "agent_mention" as const,
    owner: n.actor,
    needs: [],
    prompt: "",
  }));

  return (
    <div className="p-6 space-y-6 max-w-4xl mx-auto">
      <header>
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-2xl font-bold font-mono">{selected.run_id}</h1>
            <p className="text-sm text-muted-foreground">
              flow {selected.flow_slug} · channel #{selected.channel} · by @{selected.started_by}
            </p>
            <p className="text-xs text-muted-foreground">
              started {selected.started_at} · updated {selected.updated_at}
            </p>
          </div>
          <div className="flex gap-2 items-center">
            <span className={cn("px-2 py-1 rounded text-xs font-medium", STATUS_BG[selected.status as NodeStatus] || STATUS_BG.pending)}>
              {selected.status}
            </span>
            {selected.status === "in_progress" && (
              <Button
                size="sm"
                variant="outline"
                className="text-destructive"
                onClick={() => {
                  if (confirm(`Cancel run ${selected.run_id}?`)) {
                    cancelRun(selected.run_id);
                  }
                }}
              >
                Cancel run
              </Button>
            )}
          </div>
        </div>
      </header>

      <section>
        <h2 className="text-lg font-semibold mb-2">DAG</h2>
        <div className="border rounded p-4 bg-card overflow-x-auto">
          <Suspense fallback={<div>Loading diagram...</div>}>
            <FlowDAG nodes={dagNodes} />
          </Suspense>
        </div>
      </section>

      <section>
        <h2 className="text-lg font-semibold mb-2">Nodes</h2>
        <div className="space-y-2">
          {selected.nodes.map((n) => (
            <div
              key={n.id}
              className={cn(
                "border rounded px-3 py-2 flex items-center justify-between",
                STATUS_BG[n.status],
              )}
            >
              <div className="font-mono">{n.id}</div>
              <div className="text-xs flex gap-2 items-center">
                <span className={STATUS_COLORS[n.status]}>{n.status}</span>
                {n.actor && <span>@{n.actor}</span>}
                {n.completed_at && (
                  <span className="text-muted-foreground">
                    {n.completed_at}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
```

- [ ] **Step 3: 加 route 到 app.tsx**

定位 `<Route path="/flows" element={<FlowsView />} />` 旁(同条件 `mode === "remote"` 内),追加:

```tsx
{mode === "remote" && <Route path="/runs/:runId" element={<RunDetail />} />}
```

(Top of app.tsx 加 import `import { RunDetail } from "@/components/flows/run-detail";`)

- [ ] **Step 4: 验证 + Commit**

```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
git add products/gitim/frontend/src/hooks/use-flow-run-store.ts \
        products/gitim/frontend/src/components/flows/run-detail.tsx \
        products/gitim/frontend/src/app.tsx
git commit -m "feat(frontend): add flow run store + RunDetail page

/runs/:runId page renders DAG (lazy mermaid) + per-node status with
color coding (green=done, yellow=in_progress, red=failed, grey=pending,
struck=skipped). 'Cancel run' button on in_progress runs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: ChannelActiveRuns strip + flow recent runs section

**Files:**
- Create: `products/gitim/frontend/src/components/flows/channel-active-runs.tsx`
- Modify: `products/gitim/frontend/src/components/flows/flow-detail.tsx` (append "Recent runs")
- Modify: channel view component (insert `<ChannelActiveRuns channel={slug} />`)

- [ ] **Step 1: 创建 ChannelActiveRuns**

`components/flows/channel-active-runs.tsx`:

```typescript
import { useEffect } from "react";
import { Link } from "react-router-dom";

import { useFlowRunStore } from "@/hooks/use-flow-run-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { cn } from "@/lib/utils";
import type { RunStatus } from "@/lib/types";

const STATUS_PILL: Record<RunStatus, string> = {
  in_progress: "bg-yellow-100 dark:bg-yellow-950 text-yellow-800 dark:text-yellow-200",
  done: "bg-green-100 dark:bg-green-950 text-green-800 dark:text-green-200",
  failed: "bg-red-100 dark:bg-red-950 text-red-800 dark:text-red-200",
  cancelled: "bg-muted text-muted-foreground",
};

export function ChannelActiveRuns({ channel }: { channel: string }) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const setWorkspace = useFlowRunStore((s) => s.setWorkspace);
  const runsByChannel = useFlowRunStore((s) => s.runsByChannel);
  const loadRunsForChannel = useFlowRunStore((s) => s.loadRunsForChannel);

  useEffect(() => {
    if (activeSlug) setWorkspace(activeSlug);
    loadRunsForChannel(channel);
  }, [activeSlug, channel, setWorkspace, loadRunsForChannel]);

  const runs = (runsByChannel[channel] ?? []).filter((r) => r.status === "in_progress");
  if (runs.length === 0) return null;

  return (
    <div className="border-b bg-muted/30 px-4 py-2 flex flex-wrap gap-2 items-center">
      <span className="text-xs text-muted-foreground">Active runs:</span>
      {runs.map((r) => (
        <Link
          key={r.run_id}
          to={`/runs/${r.run_id}`}
          className={cn(
            "px-2 py-0.5 rounded text-xs font-mono hover:underline",
            STATUS_PILL[r.status],
          )}
          title={`${r.flow_slug} · by @${r.started_by}`}
        >
          {r.flow_slug} · {r.nodes_done}/{r.node_count}
        </Link>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: 把 strip 加到 channel view**

Run to locate:
```bash
grep -rn "useChatStore\|messages.map\|channel.*\.thread" products/gitim/frontend/src/components/chat/ | head -10
```

找到 channel view 的主组件(probably `chat-view.tsx` 或 `channel-view.tsx`)。在 message list 上方追加:

```tsx
import { ChannelActiveRuns } from "@/components/flows/channel-active-runs";

// inside the JSX, ABOVE the messages section:
<ChannelActiveRuns channel={currentChannelSlug} />
```

`currentChannelSlug` 用 component 现有的频道标识变量名(可能是 `channel`、`activeChannel`、或 `name`)。读现有 code 确认。

- [ ] **Step 3: 在 flow-detail.tsx 加 Recent runs**

定位 `components/flows/flow-detail.tsx`,在 Nodes section 之后追加 RecentRuns section:

```tsx
import { useEffect, useState } from "react";
// ... existing imports ...
import { listFlowRuns } from "@/lib/client";
import type { FlowRunSummary } from "@/lib/types";

// in component:
const [recentRuns, setRecentRuns] = useState<FlowRunSummary[]>([]);
useEffect(() => {
  if (!activeSlug) return;
  listFlowRuns(activeSlug, { slug: doc.slug }).then((rs) => setRecentRuns(rs.slice(0, 10)));
}, [activeSlug, doc.slug]);

// in JSX, after the Nodes section:
{recentRuns.length > 0 && (
  <section>
    <h2 className="text-lg font-semibold mb-2">Recent runs</h2>
    <div className="space-y-1">
      {recentRuns.map((r) => (
        <Link
          key={r.run_id}
          to={`/runs/${r.run_id}`}
          className="block px-3 py-1.5 rounded hover:bg-muted text-sm font-mono"
        >
          <span>{r.run_id}</span>
          <span className="ml-2 text-xs text-muted-foreground">
            [{r.status}] · {r.nodes_done}/{r.node_count} nodes · #{r.channel}
          </span>
        </Link>
      ))}
    </div>
  </section>
)}
```

- [ ] **Step 4: 验证 + Commit**

```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
git add products/gitim/frontend/src/components/flows/channel-active-runs.tsx \
        products/gitim/frontend/src/components/flows/flow-detail.tsx \
        products/gitim/frontend/src/components/chat/<channel-view-file>.tsx
git commit -m "feat(frontend): add ChannelActiveRuns strip + flow Recent runs

Channel view shows active runs as pill links at top of message list.
Flow detail shows last 10 runs below Nodes section. Both link to
/runs/:run_id detail page.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: final verification + CLAUDE.md update

**Files:** (pure verification + 1 docs commit)

- [ ] **Step 1: 跑完整 workspace 测试**

```bash
cargo test --workspace
```

Expected: all green (v1's 1845 + flow_run tests + agent prompt test ≈ 1860).

- [ ] **Step 2: frontend type check**

```bash
cd products/gitim/frontend && npx tsc --noEmit && cd -
```

Expected: 0 errors. Skip `npm run build` (still broken on pre-existing provider-badge.tsx, not flow-related).

- [ ] **Step 3: cargo fmt check**

```bash
cargo fmt --check --all
```

Expected: 0 diff.

- [ ] **Step 4: 手动 smoke (E2E,跟 Task 17 v1 同套路)**

启动一个真实 workspace,跑 `gitim runtime`,确认:
1. `gitim flow create test --name "Test Flow"` 创建
2. vim 编辑 flows/test/index.md 加 2 个 agent_mention 节点 (alice / bob)
3. `gitim create-channel test-ch`
4. `gitim flow start test --channel test-ch` → 返 run_id,记下来
5. `gitim flow runs --channel test-ch --status in_progress` → 显示 1 run
6. `gitim flow run-show <run_id>` → 两节点 pending
7. `gitim flow node-set <run_id> node1 --status in_progress --actor alice` → ok
8. `gitim flow node-set <run_id> node1 --status done --actor alice` → run_status=in_progress
9. `gitim flow node-set <run_id> node2 --status done --actor bob` → run_status=done (auto-complete)
10. `gitim flow run-show <run_id>` → 全 done
11. 试 invalid transition: `gitim flow node-set <run_id> node1 --status pending` → 拒绝
12. WebUI:`/runs/<run_id>` 页面渲染 DAG + 节点列表 + status pill 颜色
13. channel `test-ch` 顶部不再有 active runs pill(已 done)
14. flow detail 页底部 "Recent runs" 显示这次 run
15. 起第二 run → cancel → 第二 run 显示 cancelled

如有 self-write loop 或意外行为,记录到 report。

- [ ] **Step 5: Update CLAUDE.md "Where we are"**

读 CLAUDE.md 顶部,在 "Team Flows v1" 段落末追加:

```
**Team Flows v1.5(runs+state)** 已落地:`flows/<slug>/runs/<run_id>/state.yaml` 记录每次具体执行的状态 —— `run_id` 格式 `YYYYMMDDTHHMMSS-XXXXXX`、必绑一个 channel(1 run ↔ 1 channel,1 channel ↔ 0..N runs)、节点 5 状态机 `pending → in_progress → done | failed | skipped`(只前向);run 4 状态机 `in_progress → done | failed | cancelled`(daemon 在所有节点终态时自动 flip)。CLI:`gitim flow start --channel`、`runs`、`run-show`、`node-set`、`run-cancel`。WebUI:channel 顶部 active runs pill 横条、flow detail 底部 Recent runs 列表、`/runs/:run_id` 详情页(mermaid DAG + per-node status color)。Agent prompt 在 `default_gitim_api` 的 Flows 段后追加了 run lifecycle 契约。**悬空缓解**:配 cron 让 coordinator 定时扫 in_progress runs 的 updated_at,超阈值的 escalate;flow_runs.list --status in_progress 是 watchdog 的核心 query。Phase 2 真正的 executor + conditional 路由 + run UI 编辑 仍 v2 留位。
```

也清理 v1 段 "Where we're going" 里 "flow runs(待实现)" 类提及(如果有)。

- [ ] **Step 6: Final commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude): record team-flows v1.5 (runs + state) landing

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Checklist

- [x] **Spec coverage**:7 个 design decision 都有 task 覆盖:
  - `run_id` 格式 + 验证 (Task 1)
  - 5 节点 + 4 run 状态机 (Task 1-2)
  - state.yaml 存盘 + 解析 (Task 2)
  - channel binding 验证 (Task 5: `handle_flow_run_start` 检 channel meta 存在 → 404)
  - Auto-complete (Task 5: 所有节点终态 → flip run.status)
  - 并发 runs (Task 6 test `run_list_filters_by_channel` 在同 channel 启 2 runs)
  - Template drift (Task 5: state.yaml snapshot template node IDs;后续 template 编辑不影响 in-flight runs —— 测试待加)
  - WebUI 三个 surface (Task 12-13: /runs/:id + channel strip + flow Recent runs)
  - Agent prompt 契约 (Task 9)
  - Watchdog 走 cron + flow_runs.list (CLAUDE.md 说明,不实现额外 hook)
- [x] **Placeholder scan**:无 TBD。"在你的项目里查 X" 类 contextual 指令带了 grep 命令。
- [x] **Type consistency**:`RunId`、`RunStatus`、`NodeStatus`、`FlowRun`、`FlowRunNode` 名字 Task 1-2 定义,后续一致。`run_path()` Task 2 定义、Task 5 使用。`commit_run_state_locked` 内部 helper、`find_run` lookup helper(都在 flow_run_handlers.rs 内私有)。TS `FlowRunSummary`、`FlowRunDetail`、`FlowRunNodeSummary` Task 11 定义,Task 12-13 使用。
- [x] **Scope**:14 task,大致 ~3 天,合理 single-plan scope。比 v1 的 17 task 小是因为很多骨架(API dispatch / runtime HTTP / client adapter / nav)已经从 v1 复用。
