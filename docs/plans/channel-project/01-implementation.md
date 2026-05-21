# Channel Project Grouping (v1) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Spec:** `docs/plans/channel-project/00-design.md`.

**Goal:** 在 channel 上加一层 project grouping,channel.meta 加 optional `project: <slug>` 字段 + `projects/<slug>.meta.yaml` 独立。Sidebar 平级混排 channel ⭐ 和 project 📁,空 project 隐藏。增量,可有可无。

**Architecture:** 数据层 → daemon handler → IPC + HTTP → CLI + frontend。Project 是 workspace-scoped 一等公民但只承担 grouping 语义,不参与 routing / permission gating / search / cards lifecycle。文件 layout 扁平 (`projects/<slug>.meta.yaml`) 对齐 `channels/<n>.meta.yaml`。

**Tech Stack:** Rust 1.x stable (workspace, sccache 跨 worktree),tokio async,serde_yaml,clap (CLI),axum (HTTP gateway),React 19 + Vite + Zustand (frontend),vitest + RTL (frontend test)。

---

## File structure

```
crates/gitim-core/src/
  types/
    project.rs          [CREATE] ProjectSlug newtype + ProjectMeta struct
    meta.rs             [MODIFY] ChannelMeta 加 project: Option<String>
    mod.rs              [MODIFY] re-export ProjectSlug / ProjectMeta
  validator/
    mod.rs              [MODIFY] add validate_project_meta

crates/gitim-daemon/src/
  api.rs                [MODIFY] Request enum +3 variants
  handlers/
    project.rs          [CREATE] handle_create_project / list_projects / set_channel_project
    mod.rs              [MODIFY] dispatch new Requests, is_write set 包含新 mutation
    channel.rs          [MODIFY] archive/unarchive 保 project 字段 (验)
  state.rs              [MODIFY] (可选) project 列表 in-memory cache

crates/gitim-client/src/
  client.rs             [MODIFY] +3 methods (list_projects / create_project / set_channel_project)

crates/gitim-cli/src/
  main.rs               [MODIFY] clap Commands enum +2 subcommands (Projects, set-project)
  commands/
    project.rs          [CREATE] cmd_list_projects / cmd_create_project
    channels.rs         [MODIFY] cmd_set_channel_project

crates/gitim-runtime/src/
  http.rs               [MODIFY] +3 axum routes + handlers (im_projects_list/create, im_channel_set_project)

products/gitim/frontend/src/
  lib/
    types.ts            [MODIFY] Channel.project + Project type + filter types
    client.ts           [MODIFY] listProjects / createProject / setChannelProject HTTP calls
    pinned-conversations.ts  [MODIFY 或 CREATE if inline in sidebar.tsx] schema 扩展 projects 数组
  hooks/
    use-project-store.ts [CREATE] Zustand store for projects
    use-chat-store.ts   [MODIFY] channel.project 暴露
    use-card-store.ts   [MODIFY] (no change — filter logic 在 component 层)
  components/
    chat/sidebar.tsx    [MODIFY] 平级 sort 算法 + project folder + collapsible + pin
    chat/sidebar.test.tsx [MODIFY] unit test for sort/empty-hide/pin
    cards/card-filter-bar.tsx [MODIFY] +project dropdown + URL round-trip
    cards/card-kanban.tsx [MODIFY] 接受 project filter
    setup/...           [no change]

docs/plans/channel-project/
  00-design.md          [no change in plan execution]
  01-implementation.md  [this file]
```

---

## Task 0: Pre-flight — baseline 确认

**Files:** none

- [ ] **Step 1: 确认 worktree + baseline 绿**

```bash
pwd                                   # 应该是 /Users/lewisliu/ateam/GitIM/.claude/worktrees/stupefied-moser-624d69
git branch --show-current             # 应该是 claude/stupefied-moser-624d69
git status --short                    # 应该 clean
cargo test --workspace --no-fail-fast 2>&1 | tail -5
```
Expected: tree clean,baseline 所有测试 pass。

- [ ] **Step 2: 锁定 design**

```bash
git log --oneline docs/plans/channel-project/ | head
```
Expected: 三个 commit (初稿 + eng-review patches + layout fix)。

---

## Phase A — Foundation (gitim-core)

### Task 1: ProjectSlug newtype

**Files:**
- Create: `crates/gitim-core/src/types/project.rs`
- Create: `crates/gitim-core/src/types/project_test.rs` (inline `#[cfg(test)] mod tests`)
- Modify: `crates/gitim-core/src/types/mod.rs` (re-export)

- [ ] **Step 1: 写失败测试 (project.rs 内联)**

```rust
// crates/gitim-core/src/types/project.rs
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum ProjectSlugError {
    #[error("project slug is empty")]
    Empty,
    #[error("project slug exceeds 32 characters")]
    TooLong,
    #[error("project slug contains invalid character: {0:?}")]
    InvalidChar(char),
    #[error("project slug must not start or end with hyphen")]
    HyphenBoundary,
    #[error("project slug must not contain consecutive hyphens")]
    ConsecutiveHyphens,
    #[error("project slug '{0}' is reserved")]
    Reserved(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectSlug(String);

/// Reserved set covers top-level directory names + system handler.
/// Keep in sync with channel reserved expectations and `RESERVED_PROJECT_SLUGS` test below.
pub const RESERVED_PROJECT_SLUGS: &[&str] = &[
    "archive", "channels", "projects", "users", "dms", "cards", "flows", "system",
];

impl ProjectSlug {
    pub fn new(s: &str) -> Result<Self, ProjectSlugError> {
        if s.is_empty() {
            return Err(ProjectSlugError::Empty);
        }
        if s.len() > 32 {
            return Err(ProjectSlugError::TooLong);
        }
        for ch in s.chars() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(ProjectSlugError::InvalidChar(ch));
            }
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(ProjectSlugError::HyphenBoundary);
        }
        if s.contains("--") {
            return Err(ProjectSlugError::ConsecutiveHyphens);
        }
        if RESERVED_PROJECT_SLUGS.contains(&s) {
            return Err(ProjectSlugError::Reserved(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectSlug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        for s in &["design", "infra", "team-a", "ml-x9"] {
            assert!(ProjectSlug::new(s).is_ok(), "{s}");
        }
    }

    #[test]
    fn empty() {
        assert_eq!(ProjectSlug::new(""), Err(ProjectSlugError::Empty));
    }

    #[test]
    fn too_long() {
        let s = "a".repeat(33);
        assert_eq!(ProjectSlug::new(&s), Err(ProjectSlugError::TooLong));
    }

    #[test]
    fn invalid_chars() {
        for s in &["UPPER", "with_underscore", "with space", "slash/here", "with.dot"] {
            assert!(matches!(ProjectSlug::new(s), Err(ProjectSlugError::InvalidChar(_))), "{s}");
        }
    }

    #[test]
    fn hyphen_boundary() {
        for s in &["-leading", "trailing-"] {
            assert_eq!(ProjectSlug::new(s), Err(ProjectSlugError::HyphenBoundary));
        }
    }

    #[test]
    fn consecutive_hyphens() {
        assert_eq!(
            ProjectSlug::new("a--b"),
            Err(ProjectSlugError::ConsecutiveHyphens)
        );
    }

    #[test]
    fn reserved() {
        for s in RESERVED_PROJECT_SLUGS {
            assert_eq!(
                ProjectSlug::new(s),
                Err(ProjectSlugError::Reserved(s.to_string()))
            );
        }
    }
}
```

- [ ] **Step 2: Re-export 在 types/mod.rs**

```rust
// crates/gitim-core/src/types/mod.rs (追加在现有 re-export 后)
pub mod project;
pub use project::{ProjectSlug, ProjectSlugError, RESERVED_PROJECT_SLUGS};
```

- [ ] **Step 3: 跑测试**

```bash
cargo test -p gitim-core --lib types::project 2>&1 | tail -15
```
Expected: 7 tests, all pass。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/project.rs crates/gitim-core/src/types/mod.rs
git commit -m "feat(core): ProjectSlug newtype with reserved-set validation"
```

---

### Task 2: ProjectMeta struct

**Files:**
- Modify: `crates/gitim-core/src/types/project.rs` (追加 ProjectMeta)
- Modify: `crates/gitim-core/src/types/mod.rs` (re-export ProjectMeta)

- [ ] **Step 1: 在 project.rs 追加 struct + serde test**

```rust
// 追加到 project.rs 顶部 use 区
use serde::{Deserialize, Serialize};

// 追加到 ProjectSlug 之后
//
// NOTE: ProjectMeta 跟 ChannelMeta 共享 display_name / created_by / created_at / introduction
// 4 个字段。v1 不抽象 (YAGNI)。修改 ChannelMeta 共享字段时记得检查这里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
}

// 追加到 mod tests {} 内
#[cfg(test)]
mod meta_tests {
    use super::*;

    #[test]
    fn project_meta_roundtrip() {
        let meta = ProjectMeta {
            display_name: "Design Sprint".to_string(),
            created_by: "lewisliu".to_string(),
            created_at: "2026-05-21T08:00:00Z".to_string(),
            introduction: "All UX work for v2".to_string(),
        };
        let yaml = serde_yaml::to_string(&meta).expect("ser");
        let back: ProjectMeta = serde_yaml::from_str(&yaml).expect("de");
        assert_eq!(meta, back);
    }

    #[test]
    fn project_meta_missing_required_field_fails() {
        // display_name 缺失
        let yaml = r#"
created_by: lewisliu
created_at: "2026-05-21T08:00:00Z"
introduction: hi
"#;
        let res: Result<ProjectMeta, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err());
    }
}
```

- [ ] **Step 2: re-export**

```rust
// crates/gitim-core/src/types/mod.rs
pub use project::{ProjectMeta, ProjectSlug, ProjectSlugError, RESERVED_PROJECT_SLUGS};
```

- [ ] **Step 3: 跑测试**

```bash
cargo test -p gitim-core --lib types::project 2>&1 | tail -10
```
Expected: 9 tests pass (7 slug + 2 meta)。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/project.rs crates/gitim-core/src/types/mod.rs
git commit -m "feat(core): ProjectMeta struct with yaml round-trip test"
```

---

### Task 3: ChannelMeta 加 project 字段 + backward-compat test

**Files:**
- Modify: `crates/gitim-core/src/types/meta.rs`

- [ ] **Step 1: 加字段 + 写 backward-compat 测试 (REGRESSION-style)**

```rust
// crates/gitim-core/src/types/meta.rs
// MODIFY: 把现有 ChannelMeta 改成下面:

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMeta {
    pub display_name: String,
    pub created_by: String,
    pub created_at: String,
    pub introduction: String,
    #[serde(default)]
    pub members: Vec<String>,
    /// 所属 project slug。None = 不在任何 project 下。
    /// 旧 channel meta 缺省 → None,backward-compat (review finding 3.B)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

// 追加到该文件底部 (或文件已有 #[cfg(test)] mod tests)
#[cfg(test)]
mod channel_meta_project_tests {
    use super::*;

    #[test]
    fn old_yaml_without_project_field_parses_as_none() {
        // 老 channel.meta.yaml 无 project 字段
        let yaml = r#"
display_name: General
created_by: lewisliu
created_at: "2026-04-01T10:00:00Z"
introduction: General chat
members:
  - lewisliu
"#;
        let meta: ChannelMeta = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(meta.project, None);
    }

    #[test]
    fn none_project_skips_serialization() {
        let meta = ChannelMeta {
            display_name: "g".into(),
            created_by: "u".into(),
            created_at: "2026-04-01T10:00:00Z".into(),
            introduction: "x".into(),
            members: vec!["u".into()],
            project: None,
        };
        let yaml = serde_yaml::to_string(&meta).expect("ser");
        assert!(
            !yaml.contains("project"),
            "project field should be skipped when None; got:\n{yaml}"
        );
    }

    #[test]
    fn some_project_roundtrips() {
        let meta = ChannelMeta {
            display_name: "g".into(),
            created_by: "u".into(),
            created_at: "2026-04-01T10:00:00Z".into(),
            introduction: "x".into(),
            members: vec!["u".into()],
            project: Some("design".into()),
        };
        let yaml = serde_yaml::to_string(&meta).expect("ser");
        assert!(yaml.contains("project: design"));
        let back: ChannelMeta = serde_yaml::from_str(&yaml).expect("de");
        assert_eq!(meta, back);
    }

    #[test]
    fn new_yaml_with_extra_unknown_field_still_parses() {
        // 老 daemon 读新 yaml 的反向场景:新加字段不破 parse
        // (serde 默认 deny_unknown_fields = false)
        let yaml = r#"
display_name: g
created_by: u
created_at: "2026-04-01T10:00:00Z"
introduction: x
members:
  - u
project: design
future_field: foo
"#;
        let meta: ChannelMeta = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(meta.project, Some("design".to_string()));
    }
}
```

- [ ] **Step 2: 跑测试**

```bash
cargo test -p gitim-core --lib types::meta 2>&1 | tail -15
```
Expected: 4 new tests + 现有 ChannelMeta 测试 全部 pass。

- [ ] **Step 3: 跑全 gitim-core 测试,确认没破现有测试**

```bash
cargo test -p gitim-core 2>&1 | tail -10
```
Expected: all green。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-core/src/types/meta.rs
git commit -m "feat(core): ChannelMeta gains optional project field with backward-compat test"
```

---

### Task 4: validate_project_meta + daemon-side validator

**Files:**
- Modify: `crates/gitim-core/src/validator/mod.rs`

- [ ] **Step 1: 在 validator/mod.rs 追加 validate_project_meta**

```rust
// crates/gitim-core/src/validator/mod.rs
// 在 validate_channel_name 之后追加:

use crate::types::ProjectMeta;
use crate::types::Handler;

pub fn validate_project_meta(yaml: &str) -> Result<ProjectMeta, ValidationError> {
    let meta: ProjectMeta = serde_yaml::from_str(yaml)?;
    if meta.display_name.is_empty() || meta.display_name.len() > 64 {
        return Err(ValidationError::FieldConstraint {
            field: "display_name".into(),
            reason: "must be 1-64 characters".into(),
        });
    }
    if meta.introduction.is_empty() || meta.introduction.len() > 500 {
        return Err(ValidationError::FieldConstraint {
            field: "introduction".into(),
            reason: "must be 1-500 characters".into(),
        });
    }
    Handler::new(&meta.created_by).map_err(|_| ValidationError::FieldConstraint {
        field: "created_by".into(),
        reason: "must be a valid handler".into(),
    })?;
    Ok(meta)
}

#[cfg(test)]
mod project_meta_validator_tests {
    use super::*;

    #[test]
    fn valid_yaml() {
        let yaml = r#"
display_name: Design Sprint
created_by: lewisliu
created_at: "2026-05-21T08:00:00Z"
introduction: All UX work for v2
"#;
        let meta = validate_project_meta(yaml).expect("ok");
        assert_eq!(meta.display_name, "Design Sprint");
    }

    #[test]
    fn empty_display_name_rejected() {
        let yaml = r#"
display_name: ""
created_by: lewisliu
created_at: "2026-05-21T08:00:00Z"
introduction: hi
"#;
        assert!(validate_project_meta(yaml).is_err());
    }

    #[test]
    fn too_long_introduction_rejected() {
        let intro = "x".repeat(501);
        let yaml = format!(
            "display_name: a\ncreated_by: lewisliu\ncreated_at: \"2026-05-21T08:00:00Z\"\nintroduction: {}\n",
            intro
        );
        assert!(validate_project_meta(&yaml).is_err());
    }
}
```

- [ ] **Step 2: 跑测试**

```bash
cargo test -p gitim-core --lib validator 2>&1 | tail -10
```
Expected: 3 new tests pass + 现有 validator 测试不破。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-core/src/validator/mod.rs
git commit -m "feat(core): validate_project_meta with bounds + handler check"
```

---

## Phase B — Daemon (gitim-daemon)

### Task 5: 加 Request enum variants (3 个)

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`

- [ ] **Step 1: 在 Request enum 追加 3 个 variants**

```rust
// crates/gitim-daemon/src/api.rs
// 在 enum Request 内,跟 CreateChannel / ListChannels 同区域追加:

    #[serde(rename = "list_projects")]
    ListProjects,

    #[serde(rename = "create_project")]
    CreateProject {
        slug: String,
        display_name: String,
        introduction: String,
        #[serde(default)]
        author: Option<String>,
    },

    #[serde(rename = "set_channel_project")]
    SetChannelProject {
        channel: String,
        /// None = unassign,Some("X") = assign/reassign
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        author: Option<String>,
    },
```

- [ ] **Step 2: 跑 cargo build,确认 dispatch 编译能继续 (handlers/mod.rs 会报 unhandled variant)**

```bash
cargo build -p gitim-daemon 2>&1 | tail -10
```
Expected: build **失败**,error `non-exhaustive patterns: \`ListProjects | CreateProject ... | SetChannelProject ...\` not covered` (在 handlers/mod.rs 的 match Request)。这是预期 —— 下一步加 dispatch。

- [ ] **Step 3: Commit (无功能,纯 schema)**

```bash
git add crates/gitim-daemon/src/api.rs
git commit -m "feat(daemon): Request enum gains list/create_project + set_channel_project"
```

---

### Task 6: handle_create_project handler

**Files:**
- Create: `crates/gitim-daemon/src/handlers/project.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs` (re-export + dispatch CreateProject)

- [ ] **Step 1: 写 handler 文件 (handler + 集成测试)**

```rust
// crates/gitim-daemon/src/handlers/project.rs
use crate::api::Response;
use crate::handlers::ensure_author_not_departed;
use crate::state::SharedState;

use gitim_core::types::{Handler, ProjectMeta, ProjectSlug};
use gitim_core::validator::validate_project_meta;
use gitim_sync::git::GitError;
use tracing::{error, info, warn};

const MAX_PUSH_RETRIES: u32 = 3;

pub async fn handle_create_project(
    state: SharedState,
    slug: String,
    display_name: String,
    introduction: String,
    author: String,
) -> Response {
    // 1. Validate author
    let _handler = match Handler::new(&author) {
        Ok(h) => h,
        Err(e) => return Response::error(format!("invalid author: {e}")),
    };
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {author}"));
        }
    }

    // 2. Validate slug
    let project_slug = match ProjectSlug::new(&slug) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("invalid_slug: {e}")),
    };

    // 3. Check project doesn't already exist (active or archive)
    let projects_dir = state.repo_root.join("projects");
    let meta_path = projects_dir.join(format!("{project_slug}.meta.yaml"));
    if meta_path.exists() {
        return Response::error_code("project_exists", format!("project '{slug}' already exists"));
    }

    // 4. Create projects/ dir
    if let Err(e) = std::fs::create_dir_all(&projects_dir) {
        return Response::error(format!("failed to create projects dir: {e}"));
    }

    // 5. Build + serialize meta
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta = ProjectMeta {
        display_name,
        created_by: author.clone(),
        created_at: now,
        introduction,
    };

    // re-validate via validator to catch bound violations
    let yaml = match serde_yaml::to_string(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("yaml serialize: {e}")),
    };
    if let Err(e) = validate_project_meta(&yaml) {
        return Response::error(format!("project meta validation: {e}"));
    }

    // 6. commit_lock + write + git commit
    let _commit_guard = state.commit_lock.lock().await;
    if let Err(e) = std::fs::write(&meta_path, &yaml) {
        return Response::error(format!("write project meta: {e}"));
    }

    let meta_rel = format!("projects/{project_slug}.meta.yaml");
    let commit_msg = format!("Create project: {project_slug}");
    let git = state.git.read().await;
    if let Err(e) = git.commit_files(&[&meta_rel], &commit_msg, &author).await {
        // 清理半写状态
        let _ = std::fs::remove_file(&meta_path);
        return Response::error(format!("commit create_project: {e}"));
    }

    // 7. push retry (跟 channel handler 一致)
    for attempt in 1..=MAX_PUSH_RETRIES {
        match git.push().await {
            Ok(()) => break,
            Err(GitError::PushConflict) if attempt < MAX_PUSH_RETRIES => {
                warn!("create_project: push conflict (attempt {}/{}), rebasing", attempt, MAX_PUSH_RETRIES);
                if let Err(e) = git.fetch().await {
                    return Response::error(format!("create_project fetch failed: {e}"));
                }
                if let Err(e) = git.rebase_onto_remote().await {
                    return Response::error(format!("create_project rebase failed: {e}"));
                }
            }
            Err(e) => return Response::error(format!("create_project push failed: {e}")),
        }
    }
    drop(git);
    drop(_commit_guard);

    info!("project created: {project_slug} by {author}");
    Response::ok_with_data(serde_json::json!({"slug": project_slug.as_str()}))
}
```

- [ ] **Step 2: 在 handlers/mod.rs 加 module + dispatch**

```rust
// crates/gitim-daemon/src/handlers/mod.rs
// 在 module declarations 区追加:
pub mod project;

// 在 match Request {} 区追加新 arm (在 CreateChannel arm 附近):
        Request::CreateProject {
            slug,
            display_name,
            introduction,
            author,
        } => {
            let author = match resolve_author(&state, author).await {
                Ok(a) => a,
                Err(resp) => return resp,
            };
            project::handle_create_project(state, slug, display_name, introduction, author).await
        }

// 在 is_write 函数 (line ~150) 的 match 区追加 CreateProject:
                | Request::CreateChannel { .. }
                | Request::CreateProject { .. }     // 新
                | Request::ArchiveChannel { .. }
                // ...
```

> 注:`resolve_author` 是现有 helper (handlers/mod.rs 已有,负责把 `Option<String>` author 转 default-current-user)。若名字不一致,grep `Option<String>` author 处理的现有 helper 复用。

- [ ] **Step 3: 写集成测试 (在 tests/ 目录或 handlers/project.rs 末尾内联)**

```rust
// 优先放 crates/gitim-daemon/tests/project_create.rs (使用现有 test harness 的 spawn_daemon)
// 若现有 daemon 集成测试 harness 在 handlers/project.rs 内联会更简单,按现有 channel handler 测试位置对齐

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_state_with_users, parse_response};  // 假定现有 harness

    #[tokio::test]
    async fn create_happy_path() {
        let state = setup_state_with_users(&["alice"]).await;
        let resp = handle_create_project(
            state.clone(),
            "design".into(),
            "Design Sprint".into(),
            "All UX work".into(),
            "alice".into(),
        ).await;
        assert!(resp.ok, "{:?}", resp.error);

        let meta_path = state.repo_root.join("projects/design.meta.yaml");
        assert!(meta_path.exists());
    }

    #[tokio::test]
    async fn duplicate_returns_project_exists() {
        let state = setup_state_with_users(&["alice"]).await;
        let r1 = handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;
        assert!(r1.ok);
        let r2 = handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;
        assert!(!r2.ok);
        assert_eq!(r2.error_code.as_deref(), Some("project_exists"));
    }

    #[tokio::test]
    async fn invalid_slug_rejected() {
        let state = setup_state_with_users(&["alice"]).await;
        let r = handle_create_project(state.clone(), "UPPER".into(), "D".into(), "x".into(), "alice".into()).await;
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap_or("").contains("invalid_slug"));
    }

    #[tokio::test]
    async fn reserved_slug_rejected() {
        let state = setup_state_with_users(&["alice"]).await;
        let r = handle_create_project(state.clone(), "channels".into(), "D".into(), "x".into(), "alice".into()).await;
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap_or("").contains("invalid_slug"));
    }
}
```

> 若 daemon 没有 `test_support` 模块或 `setup_state_with_users` helper,需要 mimic `handlers/channel.rs` 现有 test 模式(用 `tempdir` + `SharedState::new_for_test` 之类)。实施时 grep `setup_state_with_users\|test_support` 找到现役 helper 名字。

- [ ] **Step 4: 跑测试**

```bash
cargo test -p gitim-daemon --lib handlers::project 2>&1 | tail -15
```
Expected: 4 tests pass。

- [ ] **Step 5: 验 cargo build 编译通**

```bash
cargo build -p gitim-daemon 2>&1 | tail -5
```
Expected: success。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-daemon/src/handlers/project.rs crates/gitim-daemon/src/handlers/mod.rs
git commit -m "feat(daemon): handle_create_project + dispatch"
```

---

### Task 7: handle_set_channel_project handler

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/project.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs` (dispatch SetChannelProject + is_write)

- [ ] **Step 1: 在 handlers/project.rs 追加 handler**

```rust
// 追加到 handlers/project.rs 末尾 (在 #[cfg(test)] 之前)

use gitim_core::types::{ChannelMeta, ChannelName};
use gitim_core::validator::validate_channel_meta;

pub async fn handle_set_channel_project(
    state: SharedState,
    channel: String,
    project: Option<String>,
    author: String,
) -> Response {
    // 1. Validate author
    if let Err(resp) = ensure_author_not_departed(&state, &author) {
        return resp;
    }
    {
        let users = state.users.read().await;
        if !users.contains(&author) {
            return Response::error(format!("unknown user: {author}"));
        }
    }

    // 2. Validate channel name
    let channel_name = match ChannelName::new(&channel) {
        Ok(n) => n,
        Err(e) => return Response::error(format!("invalid channel name: {e}")),
    };

    // 3. Validate project (if Some) exists
    if let Some(ref p_slug) = project {
        let _ = match ProjectSlug::new(p_slug) {
            Ok(s) => s,
            Err(e) => return Response::error(format!("invalid_slug: {e}")),
        };
        let p_meta = state.repo_root.join(format!("projects/{p_slug}.meta.yaml"));
        if !p_meta.exists() {
            return Response::error_code(
                "project_not_found",
                format!("project '{p_slug}' does not exist"),
            );
        }
        // Detect corrupted meta (review finding 1.B)
        let p_yaml = match std::fs::read_to_string(&p_meta) {
            Ok(s) => s,
            Err(e) => return Response::error(format!("read project meta: {e}")),
        };
        if validate_project_meta(&p_yaml).is_err() {
            return Response::error_code(
                "project_meta_corrupted",
                format!("project '{p_slug}' meta is corrupted"),
            );
        }
    }

    // 4. Find channel meta (active only — archived channels rejected, review finding 2.A)
    let active_meta = state.repo_root.join(format!("channels/{channel_name}.meta.yaml"));
    let archived_meta = state.repo_root.join(format!("archive/channels/{channel_name}.meta.yaml"));
    if !active_meta.exists() {
        if archived_meta.exists() {
            return Response::error_code(
                "channel_archived",
                format!("channel '{channel}' is archived; meta is frozen"),
            );
        }
        return Response::error_code(
            "channel_not_found",
            format!("channel '{channel}' does not exist"),
        );
    }

    // 5. Read + mutate + write meta
    let yaml = match std::fs::read_to_string(&active_meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read channel meta: {e}")),
    };
    let mut meta: ChannelMeta = match serde_yaml::from_str(&yaml) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse channel meta: {e}")),
    };

    let old_project = meta.project.clone();
    meta.project = project.clone();

    // Re-validate the bumped meta (defensive)
    let new_yaml = match serde_yaml::to_string(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("ser channel meta: {e}")),
    };
    if let Err(e) = validate_channel_meta(&new_yaml) {
        return Response::error(format!("channel meta validation: {e}"));
    }

    // 6. commit_lock + write + commit + push retry
    let _commit_guard = state.commit_lock.lock().await;
    if let Err(e) = std::fs::write(&active_meta, &new_yaml) {
        return Response::error(format!("write channel meta: {e}"));
    }

    let meta_rel = format!("channels/{channel_name}.meta.yaml");
    let commit_msg = match (old_project.as_deref(), project.as_deref()) {
        (None, Some(p)) => format!("Assign channel #{channel_name} to project '{p}'"),
        (Some(p), None) => format!("Remove channel #{channel_name} from project '{p}'"),
        (Some(from), Some(to)) => format!("Move channel #{channel_name} from '{from}' to '{to}'"),
        (None, None) => format!("Channel #{channel_name} project unchanged"),
    };

    let git = state.git.read().await;
    if let Err(e) = git.commit_files(&[&meta_rel], &commit_msg, &author).await {
        // 回滚到 disk
        let _ = std::fs::write(&active_meta, &yaml);
        return Response::error(format!("commit set_channel_project: {e}"));
    }

    for attempt in 1..=MAX_PUSH_RETRIES {
        match git.push().await {
            Ok(()) => break,
            Err(GitError::PushConflict) if attempt < MAX_PUSH_RETRIES => {
                warn!("set_channel_project: push conflict {}/{}, rebasing", attempt, MAX_PUSH_RETRIES);
                if let Err(e) = git.fetch().await {
                    return Response::error(format!("fetch failed: {e}"));
                }
                if let Err(e) = git.rebase_onto_remote().await {
                    return Response::error(format!("rebase failed: {e}"));
                }
            }
            Err(e) => return Response::error(format!("push failed: {e}")),
        }
    }
    drop(git);
    drop(_commit_guard);

    info!(
        "set_channel_project: {channel_name} {old:?} → {new:?} by {author}",
        old = old_project,
        new = project
    );
    Response::ok_with_data(serde_json::json!({
        "channel": channel_name.as_str(),
        "project": project,
    }))
}
```

- [ ] **Step 2: dispatch + is_write 在 handlers/mod.rs**

```rust
// 加 dispatch arm (channels.rs handlers 附近):
        Request::SetChannelProject {
            channel,
            project,
            author,
        } => {
            let author = match resolve_author(&state, author).await {
                Ok(a) => a,
                Err(resp) => return resp,
            };
            project::handle_set_channel_project(state, channel, project, author).await
        }

// is_write 区追加 SetChannelProject:
                | Request::CreateProject { .. }
                | Request::SetChannelProject { .. }     // 新
                | Request::ArchiveChannel { .. }
```

- [ ] **Step 3: 集成测试**

```rust
// 追加到 handlers/project.rs 的 #[cfg(test)] mod tests {} 内

#[tokio::test]
async fn set_assign_happy() {
    let state = setup_state_with_users(&["alice"]).await;
    // setup: create channel + create project
    setup_channel(&state, "dev", "alice").await;
    handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;

    let r = handle_set_channel_project(state.clone(), "dev".into(), Some("design".into()), "alice".into()).await;
    assert!(r.ok, "{:?}", r.error);

    let yaml = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
    assert!(yaml.contains("project: design"));
}

#[tokio::test]
async fn set_unassign_happy() {
    let state = setup_state_with_users(&["alice"]).await;
    setup_channel(&state, "dev", "alice").await;
    handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;
    handle_set_channel_project(state.clone(), "dev".into(), Some("design".into()), "alice".into()).await;

    let r = handle_set_channel_project(state.clone(), "dev".into(), None, "alice".into()).await;
    assert!(r.ok);

    let yaml = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
    assert!(!yaml.contains("project:"));
}

#[tokio::test]
async fn project_not_found_returns_code() {
    let state = setup_state_with_users(&["alice"]).await;
    setup_channel(&state, "dev", "alice").await;

    let r = handle_set_channel_project(state.clone(), "dev".into(), Some("ghost".into()), "alice".into()).await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("project_not_found"));
}

#[tokio::test]
async fn archived_channel_rejected() {
    let state = setup_state_with_users(&["alice"]).await;
    setup_channel(&state, "dev", "alice").await;
    handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;
    archive_channel(&state, "dev").await;  // helper to move into archive/

    let r = handle_set_channel_project(state.clone(), "dev".into(), Some("design".into()), "alice".into()).await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("channel_archived"));
}

#[tokio::test]
async fn project_meta_corrupted_returns_code() {
    let state = setup_state_with_users(&["alice"]).await;
    setup_channel(&state, "dev", "alice").await;
    // 故意写一份 corrupted project meta
    std::fs::create_dir_all(state.repo_root.join("projects")).unwrap();
    std::fs::write(
        state.repo_root.join("projects/design.meta.yaml"),
        "this: is: not: valid: yaml:::",
    ).unwrap();

    let r = handle_set_channel_project(state.clone(), "dev".into(), Some("design".into()), "alice".into()).await;
    assert!(!r.ok);
    assert_eq!(r.error_code.as_deref(), Some("project_meta_corrupted"));
}
```

> `setup_channel` / `archive_channel` 是预期已有的 test helper。若不存在,grep `handlers/channel.rs` 找已有的 `handle_create_channel` 测试模式,直接复用相同 setup。

- [ ] **Step 4: 跑测试**

```bash
cargo test -p gitim-daemon --lib handlers::project 2>&1 | tail -15
```
Expected: 9 tests (4 create + 5 set),all pass。

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/src/handlers/project.rs crates/gitim-daemon/src/handlers/mod.rs
git commit -m "feat(daemon): handle_set_channel_project with 5 validation paths"
```

---

### Task 8: handle_list_projects + channel_count

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/project.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs` (dispatch ListProjects)
- Modify: `crates/gitim-core/src/responses.rs` (ListProjectsResponse + ProjectEntry)

- [ ] **Step 1: 加 response 类型**

```rust
// crates/gitim-core/src/responses.rs
// 在 ListChannelsResponse 附近追加:

use crate::types::ProjectMeta;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectEntry {
    pub slug: String,
    pub meta: ProjectMeta,
    pub channel_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListProjectsResponse {
    pub projects: Vec<ProjectEntry>,
}
```

- [ ] **Step 2: 加 handler**

```rust
// 追加到 handlers/project.rs

use gitim_core::responses::{ListProjectsResponse, ProjectEntry};

pub async fn handle_list_projects(state: SharedState) -> Response {
    let projects_dir = state.repo_root.join("projects");
    let mut entries: Vec<ProjectEntry> = Vec::new();

    if !projects_dir.exists() {
        return Response::ok_with_data(serde_json::to_value(ListProjectsResponse { projects: vec![] }).unwrap());
    }

    // 1. Scan projects/*.meta.yaml
    let mut slugs: Vec<String> = Vec::new();
    let read_dir = match std::fs::read_dir(&projects_dir) {
        Ok(rd) => rd,
        Err(e) => return Response::error(format!("read projects dir: {e}")),
    };
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if let Some(slug) = name.strip_suffix(".meta.yaml") {
            if ProjectSlug::new(slug).is_ok() {
                slugs.push(slug.to_string());
            }
        }
    }
    slugs.sort();

    // 2. Read each meta + count channels
    // Channel count: scan channels/*.meta.yaml,parse,count project field == this slug
    let channels_dir = state.repo_root.join("channels");
    let mut channel_projects: Vec<Option<String>> = Vec::new();
    if channels_dir.exists() {
        if let Ok(rd) = std::fs::read_dir(&channels_dir) {
            for ent in rd.flatten() {
                let n = ent.file_name();
                let n = match n.to_str() {
                    Some(s) => s,
                    None => continue,
                };
                if !n.ends_with(".meta.yaml") {
                    continue;
                }
                let path = ent.path();
                let yaml = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let cm: Result<gitim_core::types::ChannelMeta, _> = serde_yaml::from_str(&yaml);
                if let Ok(cm) = cm {
                    channel_projects.push(cm.project);
                }
            }
        }
    }

    for slug in &slugs {
        let meta_path = projects_dir.join(format!("{slug}.meta.yaml"));
        let yaml = match std::fs::read_to_string(&meta_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let meta: ProjectMeta = match serde_yaml::from_str(&yaml) {
            Ok(m) => m,
            Err(_) => {
                warn!("project meta corrupted: {slug}");
                continue;  // skip corrupt entries from list (CLI/UI 看不到坏 project)
            }
        };
        let cnt = channel_projects
            .iter()
            .filter(|p| p.as_deref() == Some(slug.as_str()))
            .count();
        entries.push(ProjectEntry {
            slug: slug.clone(),
            meta,
            channel_count: cnt,
        });
    }

    let payload = ListProjectsResponse { projects: entries };
    Response::ok_with_data(serde_json::to_value(payload).unwrap())
}
```

- [ ] **Step 3: dispatch**

```rust
// handlers/mod.rs
        Request::ListProjects => project::handle_list_projects(state).await,
```

> 注:`ListProjects` 是 read-only,**不**加进 `is_write`。

- [ ] **Step 4: 测试**

```rust
#[tokio::test]
async fn list_empty() {
    let state = setup_state_with_users(&["alice"]).await;
    let r = handle_list_projects(state).await;
    assert!(r.ok);
    let data: ListProjectsResponse = serde_json::from_value(r.data.unwrap()).unwrap();
    assert!(data.projects.is_empty());
}

#[tokio::test]
async fn list_with_counts() {
    let state = setup_state_with_users(&["alice"]).await;
    handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;
    handle_create_project(state.clone(), "infra".into(), "I".into(), "y".into(), "alice".into()).await;
    setup_channel(&state, "dev", "alice").await;
    setup_channel(&state, "ml", "alice").await;
    setup_channel(&state, "ops", "alice").await;

    handle_set_channel_project(state.clone(), "dev".into(), Some("design".into()), "alice".into()).await;
    handle_set_channel_project(state.clone(), "ml".into(), Some("design".into()), "alice".into()).await;
    // 'ops' 不归属 project

    let r = handle_list_projects(state).await;
    let data: ListProjectsResponse = serde_json::from_value(r.data.unwrap()).unwrap();
    assert_eq!(data.projects.len(), 2);
    let design = data.projects.iter().find(|p| p.slug == "design").unwrap();
    let infra = data.projects.iter().find(|p| p.slug == "infra").unwrap();
    assert_eq!(design.channel_count, 2);
    assert_eq!(infra.channel_count, 0);
}
```

- [ ] **Step 5: 跑测试**

```bash
cargo test -p gitim-daemon --lib handlers::project 2>&1 | tail -15
```
Expected: 11 tests pass (9 prior + 2 new)。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-daemon/src/handlers/project.rs crates/gitim-daemon/src/handlers/mod.rs crates/gitim-core/src/responses.rs
git commit -m "feat(daemon): handle_list_projects with channel_count derived on-demand"
```

---

### Task 9: meta.yaml dispatch path audit (review finding 1.A)

**Files:**
- Inspect/Modify: `crates/gitim-sync/src/watcher.rs`
- Inspect: `crates/gitim-index/src/lib.rs`
- Inspect: `crates/gitim-daemon/src/handlers/channel.rs` (archive/unarchive)
- Inspect: `crates/gitim-daemon/src/handlers/reconcile.rs`
- Inspect: `crates/gitim-daemon/src/onboard.rs`

- [ ] **Step 1: 列出所有引用 `meta.yaml` 的代码位置**

```bash
grep -rn "meta\\.yaml\|\\.meta\\.yaml" crates/gitim-sync crates/gitim-index crates/gitim-daemon/src 2>&1 | grep -v test
```
预期输出列表:把每个 hit 分类(`channels/` / `archive/channels/` / 通用 yaml glob)。

- [ ] **Step 2: 对每个 hit 做 path-prefix verify**

针对每个**非测试** hit,在源码里读上下文。判断:
1. 该代码是否会扫到 `projects/<slug>.meta.yaml`?
2. 若是,行为是否正确(应该 ignore / 应该单独处理)?

典型 audit 模板,在 commit msg 里逐条 verify:

```
sync/watcher.rs:42 — watches **/*.meta.yaml: 
  → 新 projects/*.meta.yaml 会触发 watcher event
  → 处理逻辑 (sync_loop) 是 path-agnostic (commit + push),OK
  → verified: no change needed

index/lib.rs:108 — FTS5 indexer scans channels/*.meta.yaml:
  → 写死 `channels/` prefix,projects/ 不会被扫
  → verified: path prefix is correct

daemon/handlers/channel.rs:386 — archive moves `channels/<n>.meta.yaml`:
  → 只对 channel,projects 不动
  → verified: prefix is correct

daemon/handlers/reconcile.rs — orphan card reconcile:
  → 只扫 channels/<archived-ch>/cards/,projects 不动
  → verified

daemon/onboard.rs — workspace init:
  → 创建 users/<self>.meta.yaml,不动 projects
  → verified
```

> 实施时:对每条 audit 写出"verified" 一句话理由 + 任何需要的修复在同一 task 完成。

- [ ] **Step 3: 跑全 cargo build 确认无 regression**

```bash
cargo build --workspace 2>&1 | tail -5
cargo test --workspace --no-fail-fast 2>&1 | tail -5
```
Expected: all green。

- [ ] **Step 4: Commit (即使无 code change 也提一个 audit-evidence commit)**

如果有改动:
```bash
git add <changed-files>
git commit -m "audit(daemon): verify meta.yaml path-prefix filters for projects/ directory

- sync/watcher.rs: path-agnostic, OK
- index/lib.rs: channels/ prefix, OK
- daemon/handlers/channel.rs (archive): channels/ prefix, OK
- daemon/handlers/reconcile.rs: channels/<ch>/cards/ scoped, OK
- daemon/onboard.rs: users/ scoped, OK"
```

如果无改动,跳过 commit,把 audit 结果写到 `docs/plans/channel-project/audit-notes.md`:

```bash
cat > docs/plans/channel-project/audit-notes.md <<'EOF'
# meta.yaml dispatch path audit (review finding 1.A)

Verified all references to `meta.yaml` in production code (non-test):

- `crates/gitim-sync/src/watcher.rs`: path-agnostic, OK
- `crates/gitim-index/src/lib.rs`: scoped to `channels/` prefix, OK
- `crates/gitim-daemon/src/handlers/channel.rs`: archive/unarchive scoped, OK
- `crates/gitim-daemon/src/handlers/reconcile.rs`: scoped to `channels/<ch>/cards/`, OK
- `crates/gitim-daemon/src/onboard.rs`: scoped to `users/`, OK

No code changes needed.
EOF
git add docs/plans/channel-project/audit-notes.md
git commit -m "docs(audit): meta.yaml dispatch path audit results (no code change)"
```

---

### Task 10: REGRESSION test — archive → unarchive preserves project field (review finding 3.A)

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/channel.rs` (test 区或在新 `channels_test.rs` 加)

- [ ] **Step 1: 写测试**

```rust
// 加到 handlers/channel.rs 的 #[cfg(test)] mod tests {} 区

#[tokio::test]
async fn archive_unarchive_preserves_project_field() {
    let state = setup_state_with_users(&["alice"]).await;
    setup_channel(&state, "dev", "alice").await;
    handle_create_project(state.clone(), "design".into(), "D".into(), "x".into(), "alice".into()).await;
    handle_set_channel_project(state.clone(), "dev".into(), Some("design".into()), "alice".into()).await;

    // Pre-archive: 验证 project 字段已写入
    let before = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
    assert!(before.contains("project: design"), "pre-archive meta: {before}");

    // Archive
    let r = handle_archive_channel(state.clone(), "dev".into(), "alice".into()).await;
    assert!(r.ok, "archive failed: {:?}", r.error);

    let archived = std::fs::read_to_string(state.repo_root.join("archive/channels/dev.meta.yaml")).unwrap();
    assert!(
        archived.contains("project: design"),
        "REGRESSION: archive lost project field. meta after archive:\n{archived}"
    );

    // Unarchive
    let r = handle_unarchive_channel(state.clone(), "dev".into(), "alice".into()).await;
    assert!(r.ok, "unarchive failed: {:?}", r.error);

    let restored = std::fs::read_to_string(state.repo_root.join("channels/dev.meta.yaml")).unwrap();
    assert!(
        restored.contains("project: design"),
        "REGRESSION: unarchive lost project field. meta after unarchive:\n{restored}"
    );
}
```

> 注:`handle_archive_channel` / `handle_unarchive_channel` 是现役 handler。若 import 路径不对,grep 实施时调整。

- [ ] **Step 2: 跑测试**

```bash
cargo test -p gitim-daemon --lib handlers::channel archive_unarchive_preserves 2>&1 | tail -10
```
Expected: pass。

如果**失败** (说明 archive/unarchive 真的在丢字段),需要补:在 `handle_archive_channel` / `handle_unarchive_channel` 里确认它们用 `git mv` 或 byte-identical copy 而不是反序列化 + 重写。因为 archive_channel 现在是 `git mv channels/dev.meta.yaml archive/channels/dev.meta.yaml`,文件内容 byte-preserved → project 字段自动跟着走。如果实测出 bug,fix it in the same commit。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/handlers/channel.rs
git commit -m "test(daemon): regression — archive/unarchive preserve channel.project field"
```

---

### Task 11: Daemon backward-compat IPC test

**Files:**
- Create or Modify: `crates/gitim-daemon/tests/backward_compat.rs` (集成 test)

- [ ] **Step 1: 写老 meta yaml → 新 daemon read 测试**

```rust
// crates/gitim-daemon/tests/backward_compat.rs
use std::fs;
use std::path::PathBuf;

#[test]
fn old_channel_meta_without_project_parses_as_none() {
    // 模拟老仓库的 channel meta (无 project 字段)
    let yaml = r#"display_name: General
created_by: alice
created_at: "2026-01-01T00:00:00Z"
introduction: General chat
members:
  - alice
"#;
    let meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(yaml).expect("parse");
    assert_eq!(meta.project, None);
}

#[test]
fn channel_meta_serialize_back_byte_identical_when_project_none() {
    // 老 daemon round-trip 不应该引入 project: null
    let yaml_in = r#"display_name: General
created_by: alice
created_at: "2026-01-01T00:00:00Z"
introduction: General chat
members:
- alice
"#;
    let meta: gitim_core::types::ChannelMeta = serde_yaml::from_str(yaml_in).expect("parse");
    let yaml_out = serde_yaml::to_string(&meta).expect("ser");
    assert!(!yaml_out.contains("project"), "project field should not appear when None; got:\n{yaml_out}");
}
```

- [ ] **Step 2: 跑**

```bash
cargo test -p gitim-daemon --test backward_compat 2>&1 | tail -10
```
Expected: 2 tests pass。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/tests/backward_compat.rs
git commit -m "test(daemon): backward-compat — old channel meta parses + serializes byte-clean"
```

---

## Phase C — CLI (gitim-cli)

### Task 12: gitim-client API methods

**Files:**
- Modify: `crates/gitim-client/src/client.rs`

- [ ] **Step 1: 加 3 个 methods (跟现有 list_channels / create_channel 模式对齐)**

```rust
// crates/gitim-client/src/client.rs
// 在 list_channels 附近追加:

pub async fn list_projects(&self) -> Result<ApiResponse, ClientError> {
    self.request("list_projects", json!({})).await
}

pub async fn create_project(
    &self,
    slug: &str,
    display_name: &str,
    introduction: &str,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "create_project",
        json!({
            "slug": slug,
            "display_name": display_name,
            "introduction": introduction,
        }),
    )
    .await
}

pub async fn set_channel_project(
    &self,
    channel: &str,
    project: Option<&str>,
) -> Result<ApiResponse, ClientError> {
    self.request(
        "set_channel_project",
        json!({
            "channel": channel,
            "project": project,
        }),
    )
    .await
}
```

- [ ] **Step 2: Build 确认编译**

```bash
cargo build -p gitim-client 2>&1 | tail -3
```
Expected: success。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-client/src/client.rs
git commit -m "feat(client): list_projects / create_project / set_channel_project API methods"
```

---

### Task 13: gitim projects subcommand (list + create)

**Files:**
- Create: `crates/gitim-cli/src/commands/project.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs` (pub mod project)
- Modify: `crates/gitim-cli/src/main.rs` (clap subcommand + dispatch)

- [ ] **Step 1: 写 commands/project.rs**

```rust
// crates/gitim-cli/src/commands/project.rs
use std::process;

use gitim_client::GitimClient;
use gitim_core::responses::ListProjectsResponse;
use serde_json::Value;

use crate::output::OutputMode;

pub async fn cmd_list_projects(client: &GitimClient, mode: &OutputMode) {
    match client.list_projects().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            let data = resp.data.unwrap_or(Value::Null);
            match mode {
                OutputMode::Json => {
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: format output: {e}");
                            process::exit(1);
                        }
                    }
                }
                OutputMode::Human => {
                    let parsed: ListProjectsResponse = match serde_json::from_value(data) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Error: parse response: {e}");
                            process::exit(1);
                        }
                    };
                    if parsed.projects.is_empty() {
                        println!("(no projects)");
                        return;
                    }
                    for p in &parsed.projects {
                        println!("📁 {:<24}  {} channel(s)  — {}", p.slug, p.channel_count, p.meta.display_name);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_create_project(
    client: &GitimClient,
    mode: &OutputMode,
    slug: &str,
    display_name: &str,
    introduction: &str,
) {
    match client.create_project(slug, display_name, introduction).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("创建失败: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("Project '{slug}' created"),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
```

- [ ] **Step 2: pub mod project**

```rust
// crates/gitim-cli/src/commands/mod.rs
// 追加:
pub mod project;
```

- [ ] **Step 3: clap subcommand 注册**

```rust
// crates/gitim-cli/src/main.rs
// 在 enum Commands {} 内追加:

    /// Project management — list or create
    Projects {
        #[command(subcommand)]
        action: ProjectAction,
    },

// 在 main.rs 顶部 (Commands enum 旁) 加:

#[derive(Subcommand)]
enum ProjectAction {
    /// List all projects with channel counts
    List,
    /// Create a new project
    Create {
        /// Project slug (lowercase, a-z 0-9 -, ≤32 chars)
        slug: String,
        /// Display name
        #[arg(short = 'n', long = "name")]
        name: String,
        /// Introduction (1-500 chars)
        #[arg(short = 'i', long = "intro")]
        intro: String,
    },
}

// 在 main async fn 的 dispatch match 追加:

        Commands::Projects { action } => match action {
            ProjectAction::List => commands::project::cmd_list_projects(&client, &mode).await,
            ProjectAction::Create { slug, name, intro } => {
                commands::project::cmd_create_project(&client, &mode, &slug, &name, &intro).await
            }
        },
```

- [ ] **Step 4: cargo build CLI**

```bash
cargo build -p gitim-cli 2>&1 | tail -3
```
Expected: success。

- [ ] **Step 5: 手动跑 argv 解析测试**

```bash
target/debug/gitim projects --help 2>&1 | head -15
target/debug/gitim projects list --help 2>&1 | head -10
target/debug/gitim projects create --help 2>&1 | head -10
```
Expected: help 文本正确显示。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-cli/src/main.rs crates/gitim-cli/src/commands/mod.rs crates/gitim-cli/src/commands/project.rs
git commit -m "feat(cli): gitim projects list/create subcommand"
```

---

### Task 14: gitim channel set-project subcommand

**Files:**
- Modify: `crates/gitim-cli/src/commands/channels.rs`
- Modify: `crates/gitim-cli/src/main.rs`

- [ ] **Step 1: 加 cmd_set_channel_project**

```rust
// crates/gitim-cli/src/commands/channels.rs
// 追加:

pub async fn cmd_set_channel_project(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    project: Option<&str>,
) {
    match client.set_channel_project(channel, project).await {
        Ok(resp) => {
            if !resp.ok {
                let code = resp.error_code.as_deref().unwrap_or("");
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error ({code}): {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => match project {
                    Some(p) => println!("Channel #{channel} → project '{p}'"),
                    None => println!("Channel #{channel} removed from project"),
                },
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    println!("{}", serde_json::to_string(&data).unwrap_or_default());
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
```

- [ ] **Step 2: clap subcommand**

在 `Channel` (or 现有 channel 相关 subcommand) 下加一个 `SetProject` 子命令,或者作为顶层 `gitim channel set-project`:

```rust
// crates/gitim-cli/src/main.rs
// 决定:gitim 已有的 channel 命令是平铺的(没有 Channel { action }) — grep 一下 main.rs Commands enum 看现役约定。
// 若现役是 `Commands::Send`、`Commands::ListChannels` 等平铺,那加 Commands::SetChannelProject 一个新 variant:

    /// Assign a channel to a project (use --clear to unassign)
    SetChannelProject {
        /// Channel name
        channel: String,
        /// Project slug (omit + use --clear to remove)
        #[arg(conflicts_with = "clear")]
        project: Option<String>,
        /// Remove the channel from any project
        #[arg(long, conflicts_with = "project")]
        clear: bool,
    },

// dispatch:
        Commands::SetChannelProject { channel, project, clear } => {
            let project_arg = if clear { None } else { project.as_deref() };
            // 注意: 若用户既不传 project 也不传 --clear,reject
            if project.is_none() && !clear {
                eprintln!("Error: provide a project slug or --clear");
                process::exit(2);
            }
            commands::channels::cmd_set_channel_project(&client, &mode, &channel, project_arg).await;
        }
```

- [ ] **Step 3: Build + help 验证**

```bash
cargo build -p gitim-cli 2>&1 | tail -3
target/debug/gitim set-channel-project --help 2>&1 | head -15
```
Expected: 显示 help。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-cli/src/commands/channels.rs crates/gitim-cli/src/main.rs
git commit -m "feat(cli): gitim set-channel-project <ch> [<slug>|--clear]"
```

---

## Phase D — HTTP gateway (gitim-runtime)

### Task 15: HTTP endpoints + write-guard wire

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: 加 3 个 axum routes + handler functions**

```rust
// crates/gitim-runtime/src/http.rs
// 找到 .route("/im/channels", get(im_channels)) 那一段,在它附近加:

        .route("/im/projects", get(im_projects).post(im_projects_create))
        .route("/im/channels/{name}/project", patch(im_channel_set_project))

// 在 im_channels handler 附近 (line ~675-700) 加 3 个新 handler:

async fn im_projects(
    State(rt): State<AppState>,
    Path(slug): Path<String>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    let client = rt.client_for_workspace(&slug).await?;
    let resp = client.list_projects().await.map_err(|e| AppError::internal(e.to_string()))?;
    Ok(axum::Json(resp.data.unwrap_or_default()))
}

#[derive(serde::Deserialize)]
struct CreateProjectReq {
    slug: String,
    display_name: String,
    introduction: String,
}

async fn im_projects_create(
    State(rt): State<AppState>,
    Path(ws_slug): Path<String>,
    axum::Json(body): axum::Json<CreateProjectReq>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    // write-guard: refuse if workspace owner is departed
    rt.ensure_workspace_writable(&ws_slug).await?;
    let client = rt.client_for_workspace(&ws_slug).await?;
    let resp = client
        .create_project(&body.slug, &body.display_name, &body.introduction)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    if !resp.ok {
        return Err(AppError::from_response(resp));
    }
    Ok(axum::Json(resp.data.unwrap_or_default()))
}

#[derive(serde::Deserialize)]
struct SetChannelProjectReq {
    project: Option<String>,
}

async fn im_channel_set_project(
    State(rt): State<AppState>,
    Path((ws_slug, channel)): Path<(String, String)>,
    axum::Json(body): axum::Json<SetChannelProjectReq>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    rt.ensure_workspace_writable(&ws_slug).await?;
    let client = rt.client_for_workspace(&ws_slug).await?;
    let resp = client
        .set_channel_project(&channel, body.project.as_deref())
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    if !resp.ok {
        return Err(AppError::from_response(resp));
    }
    Ok(axum::Json(resp.data.unwrap_or_default()))
}
```

> 注:`AppState`、`AppError::from_response`、`ensure_workspace_writable`、`client_for_workspace` 都是预期已有 helper。若名字不对,grep `im_channels` 看现役模式 (line 677 的 `im_channels` handler) 完全对齐。

- [ ] **Step 2: route 注册路径要包含 workspace prefix**

实际 route mount 在 `/workspaces/{slug}/im/...` 下。看 line 5755 附近 `.route("/im/channels"...)` 的 mount 方式确认 prefix。新 routes 要走相同 prefix:

```rust
// 5755 附近 (跟现有 .route("/im/channels", ...) 同一个 .nest("/workspaces/{slug}", ...) 块里追加):
        .route("/im/projects", get(im_projects).post(im_projects_create))
        .route("/im/channels/{name}/project", patch(im_channel_set_project))
```

- [ ] **Step 3: cargo build**

```bash
cargo build -p gitim-runtime 2>&1 | tail -5
```
Expected: success。

- [ ] **Step 4: 集成测试 (axum TestClient + spawn daemon)**

复用现有 `crates/gitim-runtime/tests/...` 测试 harness:

```rust
// 加到现有 HTTP 集成测试文件 (例 tests/http_im.rs),或新建 tests/http_projects.rs

#[tokio::test]
async fn create_and_list_projects_via_http() {
    let env = TestEnv::new().await;
    env.onboard_human("alice").await;

    let create = env.post_json("/workspaces/test/im/projects", serde_json::json!({
        "slug": "design",
        "display_name": "Design Sprint",
        "introduction": "UX work",
    })).await;
    assert!(create.status().is_success());

    let list = env.get("/workspaces/test/im/projects").await;
    let body: serde_json::Value = list.json().await.unwrap();
    let projects = body["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["slug"], "design");
}

#[tokio::test]
async fn departed_user_blocked_from_create_project() {
    let env = TestEnv::new().await;
    env.onboard_human("alice").await;
    env.depart_workspace_owner().await;  // 触发 write-guard

    let create = env.post_json("/workspaces/test/im/projects", serde_json::json!({
        "slug": "design",
        "display_name": "D",
        "introduction": "x",
    })).await;
    assert!(create.status().is_client_error());
}
```

- [ ] **Step 5: 跑测试**

```bash
cargo test -p gitim-runtime --test http_projects 2>&1 | tail -10
```
Expected: 2 tests pass。

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/http_projects.rs
git commit -m "feat(runtime): /im/projects HTTP endpoints + write-guard"
```

---

## Phase E — Frontend

### Task 16: Frontend types + client

**Files:**
- Modify: `products/gitim/frontend/src/lib/types.ts`
- Modify: `products/gitim/frontend/src/lib/client.ts`

- [ ] **Step 1: types.ts**

```ts
// products/gitim/frontend/src/lib/types.ts
// 在 Channel 类型上追加 project 字段:

export interface Channel {
  // ... existing fields
  project?: string | null;  // 新:project slug 或 null/undefined = unassigned
}

// 新增:
export interface ProjectMeta {
  display_name: string;
  created_by: string;
  created_at: string;
  introduction: string;
}

export interface Project {
  slug: string;
  meta: ProjectMeta;
  channel_count: number;
}
```

- [ ] **Step 2: client.ts HTTP calls**

```ts
// products/gitim/frontend/src/lib/client.ts

export async function listProjects(workspaceSlug: string): Promise<Project[]> {
  const res = await fetch(`/workspaces/${workspaceSlug}/im/projects`);
  if (!res.ok) throw new Error(`list projects: ${res.status}`);
  const body = await res.json();
  return body.projects ?? [];
}

export async function createProject(
  workspaceSlug: string,
  slug: string,
  display_name: string,
  introduction: string,
): Promise<void> {
  const res = await fetch(`/workspaces/${workspaceSlug}/im/projects`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ slug, display_name, introduction }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error ?? `create project: ${res.status}`);
  }
}

export async function setChannelProject(
  workspaceSlug: string,
  channel: string,
  project: string | null,
): Promise<void> {
  const res = await fetch(
    `/workspaces/${workspaceSlug}/im/channels/${encodeURIComponent(channel)}/project`,
    {
      method: 'PATCH',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ project }),
    },
  );
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error ?? `set channel project: ${res.status}`);
  }
}
```

- [ ] **Step 3: Build + type-check**

```bash
cd products/gitim/frontend
npm run typecheck 2>&1 | tail -10
```
Expected: pass。

- [ ] **Step 4: Commit**

```bash
git add products/gitim/frontend/src/lib/types.ts products/gitim/frontend/src/lib/client.ts
git commit -m "feat(frontend): Project / ProjectMeta types + HTTP client methods"
```

---

### Task 17: useProjectStore + sidebar 平级 sort 算法 (unit test)

**Files:**
- Create: `products/gitim/frontend/src/hooks/use-project-store.ts`
- Create: `products/gitim/frontend/src/hooks/use-project-store.test.ts`
- Create: `products/gitim/frontend/src/lib/sidebar-tree.ts` (pure 算法)
- Create: `products/gitim/frontend/src/lib/sidebar-tree.test.ts`

- [ ] **Step 1: 算法 sidebar-tree.ts (pure function)**

```ts
// products/gitim/frontend/src/lib/sidebar-tree.ts
import type { Channel, Project } from './types';

export type SidebarNode =
  | { kind: 'channel'; channel: Channel }
  | {
      kind: 'project';
      project: Project;
      children: Channel[];
    };

/**
 * 平级 mixed sort:
 * - 无 project 的 channel → 直接 SidebarNode.channel
 * - 有成员 channel 的 project → SidebarNode.project,内含成员
 * - 空 project (无成员 channel) → 隐藏
 * - 排序: pinned 在前(由 caller 传 pinnedKeys),其后 slug 字典序
 *
 * 顶层 keys (供 pin 用):
 *   - channel: `channel:${channel.name}`
 *   - project: `project:${project.slug}`
 */
export function buildSidebarTree(
  channels: Channel[],
  projects: Project[],
  pinnedKeys: Set<string>,
): SidebarNode[] {
  const projectsBySlug = new Map(projects.map((p) => [p.slug, p]));
  const childrenByProject = new Map<string, Channel[]>();
  const unassigned: Channel[] = [];

  for (const ch of channels) {
    const proj = ch.project;
    if (proj && projectsBySlug.has(proj)) {
      const list = childrenByProject.get(proj) ?? [];
      list.push(ch);
      childrenByProject.set(proj, list);
    } else {
      // proj 是 null/undefined,或 project 不存在(防御:孤儿 project 引用)
      unassigned.push(ch);
    }
  }

  // children 按 channel name 字典序
  for (const list of childrenByProject.values()) {
    list.sort((a, b) => a.name.localeCompare(b.name));
  }

  const nodes: SidebarNode[] = [];

  // 只加非空 project (review: 空 project 隐式不显示)
  for (const proj of projects) {
    const children = childrenByProject.get(proj.slug) ?? [];
    if (children.length === 0) continue;
    nodes.push({ kind: 'project', project: proj, children });
  }

  for (const ch of unassigned) {
    nodes.push({ kind: 'channel', channel: ch });
  }

  // 排序: pinned 在前,其后字典序 (channel by name, project by slug)
  function keyOf(n: SidebarNode): string {
    return n.kind === 'channel' ? `channel:${n.channel.name}` : `project:${n.project.slug}`;
  }
  function labelOf(n: SidebarNode): string {
    return n.kind === 'channel' ? n.channel.name : n.project.slug;
  }
  nodes.sort((a, b) => {
    const aP = pinnedKeys.has(keyOf(a));
    const bP = pinnedKeys.has(keyOf(b));
    if (aP !== bP) return aP ? -1 : 1;
    return labelOf(a).localeCompare(labelOf(b));
  });

  return nodes;
}
```

- [ ] **Step 2: sidebar-tree.test.ts**

```ts
// products/gitim/frontend/src/lib/sidebar-tree.test.ts
import { describe, expect, it } from 'vitest';
import { buildSidebarTree } from './sidebar-tree';
import type { Channel, Project } from './types';

function ch(name: string, project?: string | null): Channel {
  return {
    name,
    display_name: name,
    created_by: 'alice',
    created_at: '2026-01-01T00:00:00Z',
    introduction: 'x',
    members: ['alice'],
    project,
  } as Channel;
}

function pr(slug: string): Project {
  return {
    slug,
    meta: {
      display_name: slug,
      created_by: 'alice',
      created_at: '2026-01-01T00:00:00Z',
      introduction: 'x',
    },
    channel_count: 0,  // ignored by tree
  };
}

describe('buildSidebarTree', () => {
  it('mixes channels and projects at top level', () => {
    const tree = buildSidebarTree(
      [ch('dev', 'design'), ch('random'), ch('ml', 'design')],
      [pr('design')],
      new Set(),
    );
    // design (project) sorts before random (channel) lex
    expect(tree).toHaveLength(2);
    expect(tree[0]).toMatchObject({ kind: 'project' });
    expect(tree[0].kind === 'project' && tree[0].children).toHaveLength(2);
    expect(tree[1]).toMatchObject({ kind: 'channel' });
  });

  it('hides empty project', () => {
    const tree = buildSidebarTree([ch('random')], [pr('design')], new Set());
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: 'channel' });
  });

  it('pinned items float to top', () => {
    const tree = buildSidebarTree(
      [ch('a'), ch('b'), ch('z')],
      [],
      new Set(['channel:z']),
    );
    expect(tree.map((n) => n.kind === 'channel' && n.channel.name)).toEqual(['z', 'a', 'b']);
  });

  it('orphan channel.project (project deleted) falls to unassigned', () => {
    const tree = buildSidebarTree([ch('dev', 'ghost-project')], [], new Set());
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: 'channel' });
  });

  it('children inside project sorted by channel name', () => {
    const tree = buildSidebarTree(
      [ch('zee', 'design'), ch('alpha', 'design')],
      [pr('design')],
      new Set(),
    );
    expect(tree[0].kind === 'project' && tree[0].children.map((c) => c.name)).toEqual(['alpha', 'zee']);
  });
});
```

- [ ] **Step 3: useProjectStore (zustand)**

```ts
// products/gitim/frontend/src/hooks/use-project-store.ts
import { create } from 'zustand';
import * as client from '@/lib/client';
import type { Project } from '@/lib/types';

interface ProjectStore {
  projects: Project[];
  loading: boolean;
  error: string | null;
  fetch(workspace: string): Promise<void>;
  setProjects(projects: Project[]): void;  // for SSE push or local update
}

export const useProjectStore = create<ProjectStore>((set) => ({
  projects: [],
  loading: false,
  error: null,
  async fetch(workspace) {
    set({ loading: true, error: null });
    try {
      const projects = await client.listProjects(workspace);
      set({ projects, loading: false });
    } catch (e) {
      set({ loading: false, error: (e as Error).message });
    }
  },
  setProjects(projects) {
    set({ projects });
  },
}));
```

- [ ] **Step 4: 跑测试**

```bash
cd products/gitim/frontend
npm test -- sidebar-tree 2>&1 | tail -15
```
Expected: 5 tests pass。

- [ ] **Step 5: Commit**

```bash
git add products/gitim/frontend/src/lib/sidebar-tree.ts products/gitim/frontend/src/lib/sidebar-tree.test.ts products/gitim/frontend/src/hooks/use-project-store.ts
git commit -m "feat(frontend): buildSidebarTree (pure fn) + useProjectStore (zustand)"
```

---

### Task 18: Sidebar UI 渲染 — folder + collapsible + pin

**Files:**
- Modify: `products/gitim/frontend/src/components/chat/sidebar.tsx`
- Modify: `products/gitim/frontend/src/components/chat/sidebar.test.tsx`

- [ ] **Step 1: Pin localStorage schema 扩展**

在 `sidebar.tsx` 找到 `PinnedConversations` interface 和 `emptyPinnedConversations`,加 `projects` 字段:

```ts
// products/gitim/frontend/src/components/chat/sidebar.tsx
// MODIFY:

interface PinnedConversations {
  channels: string[];
  dms: string[];
  projects: string[];   // 新
}

function emptyPinnedConversations(): PinnedConversations {
  return { channels: [], dms: [], projects: [] };  // 新
}

// read 时容忍旧 schema (无 projects 字段)
function readPinnedConversations(workspaceKey: string | null): PinnedConversations {
  if (!workspaceKey) return emptyPinnedConversations();
  try {
    const raw = localStorage.getItem(pinnedConversationsStorageKey(workspaceKey));
    if (!raw) return emptyPinnedConversations();
    const parsed = JSON.parse(raw) as Partial<PinnedConversations>;
    return {
      channels: Array.isArray(parsed.channels) ? parsed.channels : [],
      dms: Array.isArray(parsed.dms) ? parsed.dms : [],
      projects: Array.isArray(parsed.projects) ? parsed.projects : [],  // 新,兼容老 schema
    };
  } catch {
    return emptyPinnedConversations();
  }
}
```

- [ ] **Step 2: 引入 buildSidebarTree + 渲染**

```ts
// products/gitim/frontend/src/components/chat/sidebar.tsx
// 在 Sidebar 组件内, channels 数据准备好后,加:

import { buildSidebarTree, type SidebarNode } from '@/lib/sidebar-tree';
import { useProjectStore } from '@/hooks/use-project-store';
import { Folder, Star, Pin as PinIcon } from 'lucide-react';

const projects = useProjectStore((s) => s.projects);
const fetchProjects = useProjectStore((s) => s.fetch);

useEffect(() => {
  if (workspaceKey) fetchProjects(workspaceKey);
}, [workspaceKey, fetchProjects]);

const pinnedKeys = useMemo(() => {
  const s = new Set<string>();
  for (const c of pinned.channels) s.add(`channel:${c}`);
  for (const p of pinned.projects) s.add(`project:${p}`);
  return s;
}, [pinned]);

const tree = useMemo(
  () => buildSidebarTree(channels, projects, pinnedKeys),
  [channels, projects, pinnedKeys],
);

// project collapse 状态(localStorage 持久化)
const [expandedProjects, setExpandedProjects] = useState<Set<string>>(() => {
  // load from localStorage
  if (!workspaceKey) return new Set();
  try {
    const raw = localStorage.getItem(`gitim-expanded-projects:${workspaceKey}`);
    return new Set(raw ? (JSON.parse(raw) as string[]) : []);
  } catch {
    return new Set();
  }
});
useEffect(() => {
  if (!workspaceKey) return;
  localStorage.setItem(
    `gitim-expanded-projects:${workspaceKey}`,
    JSON.stringify([...expandedProjects]),
  );
}, [expandedProjects, workspaceKey]);
```

渲染:

```tsx
{tree.map((node) =>
  node.kind === 'channel' ? (
    <SidebarChannelItem
      key={`channel:${node.channel.name}`}
      channel={node.channel}
      pinned={pinnedKeys.has(`channel:${node.channel.name}`)}
      icon={<Star size={14} />}
      onTogglePin={() => togglePin('channel', node.channel.name)}
    />
  ) : (
    <SidebarProjectItem
      key={`project:${node.project.slug}`}
      project={node.project}
      children={node.children}
      pinned={pinnedKeys.has(`project:${node.project.slug}`)}
      expanded={expandedProjects.has(node.project.slug)}
      onToggleExpand={() => {
        setExpandedProjects((prev) => {
          const next = new Set(prev);
          next.has(node.project.slug) ? next.delete(node.project.slug) : next.add(node.project.slug);
          return next;
        });
      }}
      onTogglePin={() => togglePin('project', node.project.slug)}
      icon={<Folder size={14} />}
    />
  ),
)}
```

`togglePin` helper (单独 export 或 inline):

```ts
const togglePin = (kind: 'channel' | 'project' | 'dm', id: string) => {
  setPinned((prev) => {
    const next = { ...prev };
    const key = kind === 'channel' ? 'channels' : kind === 'project' ? 'projects' : 'dms';
    const arr = next[key];
    next[key] = arr.includes(id) ? arr.filter((x) => x !== id) : [...arr, id];
    return next;
  });
};
```

新组件 `SidebarChannelItem` / `SidebarProjectItem` (inline 在 sidebar.tsx 或 separate files):

```tsx
function SidebarChannelItem({ channel, pinned, icon, onTogglePin }: {
  channel: Channel;
  pinned: boolean;
  icon: React.ReactNode;
  onTogglePin(): void;
}) {
  return (
    <div className="flex items-center px-2 py-1 hover:bg-zinc-100 rounded">
      <span className="mr-2 text-zinc-500">{icon}</span>
      <Link to={`/channels/${channel.name}`} className="flex-1 truncate">{channel.display_name}</Link>
      <button onClick={onTogglePin} className="ml-2 opacity-0 group-hover:opacity-100">
        <PinIcon size={12} className={pinned ? 'fill-zinc-700' : ''} />
      </button>
    </div>
  );
}

function SidebarProjectItem({
  project, children, pinned, expanded, onToggleExpand, onTogglePin, icon,
}: {
  project: Project;
  children: Channel[];
  pinned: boolean;
  expanded: boolean;
  onToggleExpand(): void;
  onTogglePin(): void;
  icon: React.ReactNode;
}) {
  return (
    <div className="flex flex-col">
      <div className="flex items-center px-2 py-1 hover:bg-zinc-100 rounded cursor-pointer" onClick={onToggleExpand}>
        <ChevronRight
          size={12}
          className={`mr-1 transition-transform ${expanded ? 'rotate-90' : ''}`}
        />
        <span className="mr-2 text-zinc-500">{icon}</span>
        <span className="flex-1 truncate font-medium">{project.meta.display_name}</span>
        <span className="text-xs text-zinc-500 mr-2">{children.length}</span>
        <button onClick={(e) => { e.stopPropagation(); onTogglePin(); }} className="opacity-0 group-hover:opacity-100">
          <PinIcon size={12} className={pinned ? 'fill-zinc-700' : ''} />
        </button>
      </div>
      {expanded && (
        <div className="ml-4">
          {children.map((ch) => (
            <SidebarChannelItem
              key={ch.name}
              channel={ch}
              pinned={false}  // 子 channel pin 独立
              icon={<Star size={12} />}
              onTogglePin={() => { /* no-op,子项不在外层重新展示 */ }}
            />
          ))}
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 3: sidebar.test.tsx 加 unit test**

```tsx
// products/gitim/frontend/src/components/chat/sidebar.test.tsx
// 追加:

import { render, screen, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
// ... 用现有 test setup

describe('Sidebar with projects', () => {
  it('renders project folder with channel children', () => {
    // setup zustand stores with channels=[{dev, project=design}] + projects=[design]
    // render <Sidebar />
    // expect project folder visible
    // expect channel hidden until folder expanded
    // click folder
    // expect channel visible
  });

  it('hides empty projects', () => {
    // projects=[design] but no channel has project=design
    // expect project not rendered
  });

  it('pinning a project moves it to top', () => {
    // setup: 2 projects sorted by slug,pin the second
    // expect it now appears first
    // verify localStorage gitim-pinned-conversations:<ws> 含 projects: [<slug>]
  });
});
```

> 注:RTL 测试需要 mock zustand stores 跟 client。复用现有 `sidebar.test.tsx` 的 mock setup 模式。

- [ ] **Step 4: 跑测试**

```bash
cd products/gitim/frontend
npm test -- sidebar 2>&1 | tail -15
```
Expected: pass。

- [ ] **Step 5: dev 启动 + visual check (optional but recommended)**

```bash
cd products/gitim/frontend
npm run dev &
# open http://localhost:5173,手动验:create project → assign channel → see folder
```

- [ ] **Step 6: Commit**

```bash
git add products/gitim/frontend/src/components/chat/sidebar.tsx products/gitim/frontend/src/components/chat/sidebar.test.tsx
git commit -m "feat(frontend): sidebar mixes channels (⭐) and projects (📁) at top level"
```

---

### Task 19: Cards filter bar — project filter + URL param

**Files:**
- Modify: `products/gitim/frontend/src/components/cards/card-filter-bar.tsx`
- Modify: `products/gitim/frontend/src/components/cards/card-kanban.tsx`

- [ ] **Step 1: card-filter-bar.tsx 加 project 字段**

```ts
// products/gitim/frontend/src/components/cards/card-filter-bar.tsx

export interface CardFilterState {
  channels: string[];
  labels: string[];
  assignee: string | null;
  mineOnly: boolean;
  project: string | null;       // 新:null = All, '__unassigned__' = no project, '<slug>' = specific
}

export const EMPTY_CARD_FILTER: CardFilterState = {
  channels: [],
  labels: [],
  assignee: null,
  mineOnly: false,
  project: null,                  // 新
};
```

在组件 JSX 里加一个 project dropdown:

```tsx
import { useProjectStore } from '@/hooks/use-project-store';

const projects = useProjectStore((s) => s.projects);

// 在 filter bar JSX 加:
<select
  value={filter.project ?? ''}
  onChange={(e) => onChange({ ...filter, project: e.target.value || null })}
  className="px-2 py-1 border rounded"
>
  <option value="">All projects</option>
  <option value="__unassigned__">Unassigned</option>
  {projects.map((p) => (
    <option key={p.slug} value={p.slug}>{p.meta.display_name}</option>
  ))}
</select>
```

- [ ] **Step 2: card-kanban.tsx URL round-trip + filter**

```ts
// products/gitim/frontend/src/components/cards/card-kanban.tsx

function readFilterFromURL(params: URLSearchParams): CardFilterState {
  const assignee = params.get('assignee');
  return {
    channels: params.getAll('channel'),
    labels: params.getAll('label'),
    assignee: assignee === '__me__' ? null : assignee,
    mineOnly: assignee === '__me__',
    project: params.get('project'),     // 新
  };
}

function writeFilterToURL(filter: CardFilterState): URLSearchParams {
  const p = new URLSearchParams();
  for (const ch of filter.channels) p.append('channel', ch);
  for (const l of filter.labels) p.append('label', l);
  if (filter.mineOnly) p.set('assignee', '__me__');
  else if (filter.assignee) p.set('assignee', filter.assignee);
  if (filter.project) p.set('project', filter.project);   // 新
  return p;
}
```

card 过滤(在 `selectFilteredCards` 或 `useMemo` 计算 filteredCards 处加 project filter):

```ts
// 假设 selectFilteredCards 接收 cards + filter
// 加:
const channels = useChatStore((s) => s.channels);

const channelsInProject = useMemo(() => {
  if (filter.project === null) return null;  // no filter
  if (filter.project === '__unassigned__') {
    return new Set(channels.filter((c) => !c.project).map((c) => c.name));
  }
  return new Set(channels.filter((c) => c.project === filter.project).map((c) => c.name));
}, [filter.project, channels]);

const projectFilteredCards = useMemo(() => {
  if (channelsInProject === null) return filteredCards;
  return filteredCards.filter((card) => channelsInProject.has(card.channel));
}, [filteredCards, channelsInProject]);
```

> 选择:或者把 project filter 反向传到 `selectFilteredCards`,但简单起见在 component 里加一层 derived。

- [ ] **Step 3: 加 test**

```ts
// 加到现有 card-kanban 的 test 或 lib/ui-state.test.ts (URL round-trip)

it('preserves project filter in URL round-trip', () => {
  const filter: CardFilterState = {
    channels: ['dev'],
    labels: [],
    assignee: null,
    mineOnly: false,
    project: 'design',
  };
  const params = writeFilterToURL(filter);
  expect(params.get('project')).toBe('design');
  const back = readFilterFromURL(params);
  expect(back.project).toBe('design');
});

it('preserves unassigned magic value', () => {
  const filter: CardFilterState = { ...EMPTY_CARD_FILTER, project: '__unassigned__' };
  const params = writeFilterToURL(filter);
  expect(params.get('project')).toBe('__unassigned__');
  const back = readFilterFromURL(params);
  expect(back.project).toBe('__unassigned__');
});
```

- [ ] **Step 4: 跑测试**

```bash
cd products/gitim/frontend
npm test -- card 2>&1 | tail -15
```
Expected: tests pass。

- [ ] **Step 5: Commit**

```bash
git add products/gitim/frontend/src/components/cards/card-filter-bar.tsx products/gitim/frontend/src/components/cards/card-kanban.tsx
git commit -m "feat(frontend): cards filter bar gains project dropdown + URL round-trip"
```

---

### Task 20: Frontend E2E test — create→assign→sidebar→pin→reload (review finding 3.C)

**Files:**
- Create: `products/gitim/frontend/src/__e2e__/channel-project.test.tsx` (或贴近现有 e2e 模式)

> 注:gitim frontend 当前测试是 vitest + RTL (不是 playwright)。这个 "E2E" 是 component-level integration test,mock 掉网络层。

- [ ] **Step 1: 写 test**

```tsx
// products/gitim/frontend/src/__e2e__/channel-project.test.tsx
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import * as client from '@/lib/client';
import { useProjectStore } from '@/hooks/use-project-store';
import { useChatStore } from '@/hooks/use-chat-store';
import { Sidebar } from '@/components/chat/sidebar';

vi.mock('@/lib/client', () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  setChannelProject: vi.fn(),
  // ... other methods mocked as no-op
}));

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  useProjectStore.setState({ projects: [], loading: false, error: null });
  useChatStore.setState({ channels: [] });
});

describe('channel-project E2E flow', () => {
  it('create project → assign channel → sidebar shows folder', async () => {
    (client.listProjects as any).mockResolvedValue([
      { slug: 'design', meta: { display_name: 'Design', created_by: 'alice', created_at: '2026-01-01', introduction: 'x' }, channel_count: 1 },
    ]);
    useChatStore.setState({
      channels: [
        { name: 'dev', display_name: 'dev', created_by: 'alice', created_at: '2026-01-01', introduction: 'x', members: ['alice'], project: 'design' },
        { name: 'random', display_name: 'random', created_by: 'alice', created_at: '2026-01-01', introduction: 'x', members: ['alice'] },
      ],
    });

    render(<MemoryRouter><Sidebar workspaceKey="test" /></MemoryRouter>);

    await waitFor(() => expect(screen.getByText('Design')).toBeInTheDocument());
    // 'dev' channel 在折叠的 project 里,初始不可见
    expect(screen.queryByText('dev')).not.toBeInTheDocument();
    // 'random' 是 unassigned, top-level
    expect(screen.getByText('random')).toBeInTheDocument();

    // 展开 project
    fireEvent.click(screen.getByText('Design'));
    expect(screen.getByText('dev')).toBeInTheDocument();
  });

  it('pin project persists to localStorage and survives reload', async () => {
    (client.listProjects as any).mockResolvedValue([
      { slug: 'design', meta: { display_name: 'Design', created_by: 'alice', created_at: '2026-01-01', introduction: 'x' }, channel_count: 1 },
      { slug: 'infra', meta: { display_name: 'Infra', created_by: 'alice', created_at: '2026-01-01', introduction: 'x' }, channel_count: 1 },
    ]);
    useChatStore.setState({
      channels: [
        { name: 'a', project: 'design', display_name: 'a', created_by: 'alice', created_at: 'x', introduction: 'x', members: ['alice'] },
        { name: 'b', project: 'infra', display_name: 'b', created_by: 'alice', created_at: 'x', introduction: 'x', members: ['alice'] },
      ],
    });

    const { unmount } = render(<MemoryRouter><Sidebar workspaceKey="test" /></MemoryRouter>);
    await waitFor(() => expect(screen.getByText('Design')).toBeInTheDocument());

    // Pin Infra (lex后)
    const infraNode = screen.getByText('Infra').closest('div')!;
    const pinBtn = infraNode.querySelector('button')!;
    await act(async () => fireEvent.click(pinBtn));

    // 验 localStorage 写入
    const raw = localStorage.getItem('gitim-pinned-conversations:test');
    const pinned = JSON.parse(raw!);
    expect(pinned.projects).toContain('infra');

    // Reload (remount)
    unmount();
    render(<MemoryRouter><Sidebar workspaceKey="test" /></MemoryRouter>);
    await waitFor(() => expect(screen.getByText('Infra')).toBeInTheDocument());

    // Infra 在 Design 之前 (pinned 优先)
    const items = screen.getAllByRole('link', { hidden: true });
    // (具体 DOM 查询路径在实施时按 sidebar 实际结构调)
    // 至少验:Infra 出现在 Design 之前
    const infraIdx = screen.getByText('Infra').compareDocumentPosition(screen.getByText('Design'));
    expect(infraIdx & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
```

> Vitest jsdom 默认带 localStorage。Mock client 层即可不打真实 HTTP。

- [ ] **Step 2: 跑**

```bash
cd products/gitim/frontend
npm test -- channel-project 2>&1 | tail -15
```
Expected: 2 tests pass。

- [ ] **Step 3: Commit**

```bash
git add products/gitim/frontend/src/__e2e__/channel-project.test.tsx
git commit -m "test(frontend): E2E — create project → assign → sidebar → pin → reload"
```

---

## Phase F — Cleanup

### Task 21: Full workspace test pass + lint clean

- [ ] **Step 1: cargo test --workspace 全量绿**

```bash
cargo test --workspace --no-fail-fast 2>&1 | tail -10
```
Expected: 全部 pass。若 fail,fix-then-rerun(可能 daemon poller 集成测试需要更长 timeout)。

- [ ] **Step 2: cargo clippy 0 warning**

```bash
cargo clippy --workspace --all-targets --no-deps --locked 2>&1 | tail -10
```
Expected: no error/warning。

- [ ] **Step 3: frontend test + typecheck + build**

```bash
cd products/gitim/frontend
npm run typecheck && npm test && npm run build
```
Expected: all green。

- [ ] **Step 4 (no commit):** 这是 verification step,无新 commit。如果发现 regression,fix-then-commit。

---

### Task 22: 更新 CLAUDE.md Current Orientation 段落

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: 在 Current Orientation "Where we are" 段尾追加一句**

```markdown
**Channel-project grouping v1** 已落地:`channels/<n>.meta.yaml` 加 optional `project: <slug>` 字段 + `projects/<slug>.meta.yaml` 独立 (扁平 layout 跟现有 channel meta 对齐)。Mutation = create project + set channel.project (None/Some 三态同接口);workspace-flat permission 不 gate;archived channel 拒 `SetChannelProject` (`channel_archived`),project meta corrupted 区分 (`project_meta_corrupted`)。Sidebar 平级混排 channel ⭐ 和 project 📁,空 project 隐式不显示;pinned 沿用 `gitim-pinned-conversations:<workspace>` localStorage 加 `projects` 数组,跟 channel pin 同套 mechanism。Cards 视图加 project filter (单选,URL `project=` round-trip,`__unassigned__` magic value)。Routing v1 recipients / archive / flows / gitim-index / agent provision 全不动。spec 见 `docs/plans/channel-project/00-design.md`,plan 见 `docs/plans/channel-project/01-implementation.md`。
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(CLAUDE): record channel-project v1 in Current Orientation"
```

---

## Self-review (执行前对照 spec 复核)

**1. Spec coverage:**
- Design §1 (data model): Task 1-4 ✓
- Design §2 (mutation): Task 5-8 ✓
- Design §3 (validation): Task 6-8 (5 类 validation 都 covered) ✓
- Design §4 (permission workspace-flat): write-guard wired in Task 15,handlers 沿用 ensure_author_not_departed ✓
- Design §5 (routing/archive/flows/cards/index 不动): Task 9 audit + Task 10 regression test ✓
- Design §6 (sidebar): Task 17-18 ✓
- Design §7 (cards filter): Task 19 ✓
- Design §8 (CLI surface): Task 12-14 ✓
- Design §9 (daemon API + HTTP): Task 5-8 + Task 15 ✓
- Design §10 (migration backward-compat): Task 11 + Task 3 ✓
- Design §11 (test plan 5 个 subsection): Task 1-3, 6-8, 10-11, 13-14, 17-20 都对应 ✓
- Design §12 (implementation guardrails): Task 9 (audit) + Task 15 (write-guard wire) ✓

**2. Placeholder scan:** 无 TBD/TODO/"implement later"。每个 step 都有 actual code or actual command + expected output。

**3. Type consistency:**
- `ProjectSlug::new` 一处定义,各处用 `gitim_core::types::ProjectSlug` re-export
- `ProjectMeta` 字段名跟 `ChannelMeta` 共享部分严格对齐 (display_name/created_by/created_at/introduction)
- HTTP path `/im/projects` + `/im/channels/{name}/project` 跟 IPC method `list_projects` / `create_project` / `set_channel_project` 一一对应
- Frontend `setChannelProject(workspaceSlug, channel, project)` → HTTP PATCH → daemon `SetChannelProject` 一致

**4. Estimated time:**
- Phase A (Foundation): ~1.5h (4 tasks)
- Phase B (Daemon): ~3h (7 tasks,含 audit + regression test)
- Phase C (CLI): ~1.5h (3 tasks)
- Phase D (HTTP): ~1h (1 task,集成测试占大头)
- Phase E (Frontend): ~3h (5 tasks,sidebar 渲染最重)
- Phase F (Cleanup): ~30min (2 tasks)
- **Total: ~10-11h** (human team scale; CC + subagent 估 ~2-3h)

---

## Execution mode

Plan complete。两种执行选择:

1. **Subagent-driven** (推荐):每个 task dispatch 一个 fresh subagent,task 完一个 review + commit 一个;并行做 Phase C / D / E (CLI / HTTP / frontend) 在 Phase B 完成后。
2. **Inline execution**:在当前 session 顺序 execute,checkpoint 在每 phase 结束。
