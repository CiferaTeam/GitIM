# Agent Config Editable Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** WebUI Agent detail 页加 Provider 显示、修掉 Sonnet 硬编码 fallback、加 Edit 模式（可改 system_prompt / env / `.env` 文件）；仓库 `/git/init` 阶段自动把 `.env` 加入 `.gitignore`。

**Architecture:**
- 后端：新增 `PATCH /workspaces/{slug}/agents/{id}`。me.json 为 system_prompt/env 的 source of truth；`<agent-clone>/.env` 为 dotenv secrets 的 source of truth（chmod 0600，由 agent CLI 自行读取，不注入进程 env）。
- `.gitignore` 管理：在 `provision_local_workspace` / `provision_github_workspace` 的 `provision_human` 之后，向 human clone 根追加 `.env` 规则（幂等）+ commit。
- 前端：WebUI v2 里 AgentDetail 加 `mode: "view" | "edit" | "saving"` 状态机；抽 `<EnvVarsEditor>` 共享组件给 AddAgentDialog 和 AgentDetail 复用；`<ProviderBadge>` 展示 provider。

**Tech Stack:** Rust (axum, serde_json), React 19 + Zustand + Radix UI + Tailwind, Vitest for frontend tests, `cargo test -p gitim-runtime` for backend.

**Design doc:** [`design.md`](./design.md)

---

## Phase 1：后端 — PATCH endpoint

### Task 1：添加 `AgentUpdateRequest` 类型 + route 注册 + stub handler

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:949-962` (在 `AgentAddRequest` 附近加新类型)
- Modify: `crates/gitim-runtime/src/http.rs:2447-2453` (agent routes 块里加 PATCH route)

- [ ] **Step 1: 写失败测试 — 路由存在且 404 未知 agent**

在 `crates/gitim-runtime/tests/` 下创建新文件 `agent_patch.rs`：

```rust
//! Integration tests for PATCH /workspaces/{slug}/agents/{id}.

mod common;
use common::TestRuntime;

#[tokio::test]
async fn patch_unknown_agent_returns_404() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    let res = rt
        .client
        .patch(rt.url("/workspaces/ws1/agents/nonexistent"))
        .json(&serde_json::json!({ "system_prompt": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}
```

（如果 `common` 模块不存在，先看其它集成测试文件如 `crates/gitim-runtime/tests/workspace_lifecycle.rs` 的辅助模式复用同样 pattern。）

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p gitim-runtime --test agent_patch -- --nocapture
```

期望：编译错误（common 不存在）或路由不存在的 404 → 断言失败。

- [ ] **Step 3: 实现最小 stub handler + route**

在 `crates/gitim-runtime/src/http.rs` 的 agents_add 附近（~line 1233 之后）加：

```rust
// -- /agents PATCH --

#[derive(Deserialize, Default)]
struct AgentUpdateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_prompt: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dotenv: Option<String>,
}

async fn agents_patch(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Path((_slug_path, agent_id)): axum::extract::Path<(String, String)>,
    Json(req): Json<AgentUpdateRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let _ = (state, slug, agent_id, req); // stub
    (StatusCode::NOT_FOUND, Json(serde_json::json!({
        "ok": false, "error": "agent not found"
    }))).into_response()
}
```

Route 注册（在 line 2453 附近的 agent routes 块）：

```rust
        .route("/agents/{id}", get(agents_get).patch(agents_patch));
```

注：serde 的 `Option<Option<String>>` 三态区分"字段缺省"和"传 null"；字段缺省 → 外层 None，传 null → `Some(None)`，传字符串 → `Some(Some(s))`。

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p gitim-runtime --test agent_patch patch_unknown_agent_returns_404 -- --nocapture
```

期望：PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gitim-runtime/tests/agent_patch.rs crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): add PATCH /workspaces/{slug}/agents/{id} stub

Routes PATCH on existing agents_get path; returns 404 for unknown
agent. Implementation lands in subsequent commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2：实现 system_prompt merge 到 me.json

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:agents_patch` (本 task 实现核心逻辑)
- Modify: `crates/gitim-runtime/tests/agent_patch.rs` (新测试)

- [ ] **Step 1: 写失败测试**

在 `crates/gitim-runtime/tests/agent_patch.rs` 追加：

```rust
#[tokio::test]
async fn patch_system_prompt_writes_me_json() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, Some("old prompt")).await;

    let res = rt
        .client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "system_prompt": "new prompt" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["agent"]["system_prompt"], "new prompt");

    // Verify me.json on disk
    let me_path = rt.workspace_path("ws1").join("alice/.gitim/me.json");
    let me: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&me_path).unwrap()
    ).unwrap();
    assert_eq!(me["system_prompt"], "new prompt");
    // provider still there — merge semantics, not overwrite
    assert_eq!(me["provider"], "claude");
}

#[tokio::test]
async fn patch_system_prompt_null_clears_field() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, Some("old")).await;

    let res = rt
        .client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "system_prompt": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let me_path = rt.workspace_path("ws1").join("alice/.gitim/me.json");
    let me: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&me_path).unwrap()
    ).unwrap();
    assert!(me.get("system_prompt").is_none() || me["system_prompt"].is_null());
}

#[tokio::test]
async fn patch_missing_field_does_not_touch_it() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, Some("keep me")).await;

    rt.client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();

    let me_path = rt.workspace_path("ws1").join("alice/.gitim/me.json");
    let me: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&me_path).unwrap()
    ).unwrap();
    assert_eq!(me["system_prompt"], "keep me");
}
```

如 `add_agent` helper 不存在，在 `tests/common` 里实现（参考 `agents_add` 的 HTTP 调用），或直接在测试里手动发 POST `/workspaces/ws1/agents/add`。

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p gitim-runtime --test agent_patch patch_system_prompt -- --nocapture
```

期望：都 FAIL（stub 返回 404）。

- [ ] **Step 3: 实现 handler**

替换 stub：

```rust
async fn agents_patch(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    axum::extract::Path((_slug_path, agent_id)): axum::extract::Path<(String, String)>,
    Json(req): Json<AgentUpdateRequest>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    // 1. Look up agent; clone repo_path so we can release the lock before I/O.
    let repo_root = {
        let s = state.lock().unwrap();
        let ctx = match s.workspaces.get(&slug) {
            Some(c) => c,
            None => return not_found_workspace(),
        };
        match ctx.agents.get(&agent_id) {
            Some(info) => PathBuf::from(&info.repo_path),
            None => {
                return (StatusCode::NOT_FOUND, Json(serde_json::json!({
                    "ok": false, "error": format!("agent not found: {agent_id}")
                }))).into_response();
            }
        }
    };

    // 2. Read + merge me.json (preserves untouched fields like github_email).
    let me_path = repo_root.join(".gitim/me.json");
    let me_content = match std::fs::read_to_string(&me_path) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "ok": false, "error": format!("read me.json failed: {e}")
            }))).into_response();
        }
    };
    let mut me: serde_json::Value = match serde_json::from_str(&me_content) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "ok": false, "error": format!("parse me.json failed: {e}")
            }))).into_response();
        }
    };

    if let Some(sp_opt) = &req.system_prompt {
        match sp_opt {
            Some(s) if !s.is_empty() => {
                me["system_prompt"] = serde_json::Value::String(s.clone());
            }
            _ => {
                if let Some(obj) = me.as_object_mut() {
                    obj.remove("system_prompt");
                }
            }
        }
    }

    if let Err(e) = std::fs::write(&me_path, serde_json::to_string_pretty(&me).unwrap()) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "ok": false, "error": format!("write me.json failed: {e}")
        }))).into_response();
    }

    // 3. Update in-memory AgentInfo.
    {
        let mut s = state.lock().unwrap();
        if let Some(ctx) = s.workspaces.get_mut(&slug) {
            if let Some(info) = ctx.agents.get_mut(&agent_id) {
                if let Some(sp_opt) = &req.system_prompt {
                    info.system_prompt = match sp_opt {
                        Some(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    };
                }
            }
        }
    }

    // 4. Return fresh snapshot.
    let s = state.lock().unwrap();
    let ctx = s.workspaces.get(&slug).unwrap();
    let info = ctx.agents.get(&agent_id).unwrap().clone();
    Json(serde_json::json!({ "ok": true, "agent": info })).into_response()
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p gitim-runtime --test agent_patch patch_system_prompt -- --nocapture
cargo test -p gitim-runtime --test agent_patch patch_missing_field -- --nocapture
```

期望：三个测试都 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/agent_patch.rs
git commit -m "feat(runtime): PATCH agent updates system_prompt with merge semantics

me.json merge preserves untouched fields (github_email, provider, etc).
Three-state system_prompt: absent = no-op, null/\"\" = delete field,
non-empty string = set.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3：实现 env 整体替换 + 校验

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:agents_patch`
- Modify: `crates/gitim-runtime/tests/agent_patch.rs`

- [ ] **Step 1: 写失败测试**

追加到 `tests/agent_patch.rs`：

```rust
#[tokio::test]
async fn patch_env_replaces_full_map() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent_with_env("ws1", "alice", "claude",
        [("A", "1"), ("B", "2")].into()).await;

    let res = rt
        .client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "env": { "C": "3" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["agent"]["env"]["C"], "3");
    assert!(body["agent"]["env"].get("A").is_none());

    let me_path = rt.workspace_path("ws1").join("alice/.gitim/me.json");
    let me: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&me_path).unwrap()
    ).unwrap();
    assert_eq!(me["env"]["C"], "3");
    assert!(me["env"].get("A").is_none());
}

#[tokio::test]
async fn patch_env_empty_clears_all() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent_with_env("ws1", "alice", "claude",
        [("A", "1")].into()).await;

    rt.client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "env": {} }))
        .send()
        .await
        .unwrap();

    let me_path = rt.workspace_path("ws1").join("alice/.gitim/me.json");
    let me: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&me_path).unwrap()
    ).unwrap();
    assert!(me["env"].as_object().unwrap().is_empty()
        || me.get("env").map(|v| v.is_null()).unwrap_or(true));
}

#[tokio::test]
async fn patch_env_rejects_illegal_key() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, None).await;

    let res = rt
        .client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "env": { "1bad": "x" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("invalid env var"));
}
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p gitim-runtime --test agent_patch patch_env -- --nocapture
```

期望：三个都 FAIL。

- [ ] **Step 3: 在 handler 里加 env 处理 + 校验**

在 agents_patch 的 "Read + merge me.json" 块的 `if let Some(sp_opt) ...` 之后加：

```rust
    // env validation + whole-map replacement.
    if let Some(env_map) = &req.env {
        for key in env_map.keys() {
            if !is_valid_env_key(key) {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "ok": false,
                    "error": format!("invalid env var name: {key}")
                }))).into_response();
            }
        }
        if env_map.is_empty() {
            if let Some(obj) = me.as_object_mut() {
                obj.remove("env");
            }
        } else {
            me["env"] = serde_json::to_value(env_map).unwrap();
        }
    }
```

在 http.rs 文件底部（或靠近其它小 helper 的位置）加：

```rust
fn is_valid_env_key(k: &str) -> bool {
    if k.is_empty() { return false; }
    let bytes = k.as_bytes();
    let first = bytes[0];
    if !(first.is_ascii_alphabetic() || first == b'_') { return false; }
    bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}
```

在 "Update in-memory AgentInfo" 块里同步 info.env：

```rust
                if let Some(env_map) = &req.env {
                    info.env = env_map.clone();
                }
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p gitim-runtime --test agent_patch patch_env -- --nocapture
```

期望：三个都 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/agent_patch.rs
git commit -m "feat(runtime): PATCH agent env with whole-map replacement + key validation

env var keys must match [A-Za-z_][A-Za-z0-9_]*. Empty map clears all
env vars. Replacement semantics (not merge) — so frontend can delete
individual vars by omitting them from the new map.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4：实现 dotenv 文件落盘

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:agents_patch`
- Modify: `crates/gitim-runtime/tests/agent_patch.rs`

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn patch_dotenv_writes_file_with_mode_600() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, None).await;

    rt.client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "dotenv": "OPENAI_KEY=sk-xxx\nDB=postgres://..." }))
        .send()
        .await
        .unwrap();

    let env_path = rt.workspace_path("ws1").join("alice/.env");
    let contents = std::fs::read_to_string(&env_path).unwrap();
    assert!(contents.contains("OPENAI_KEY=sk-xxx"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&env_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode = {:o}", mode & 0o777);
    }
}

#[tokio::test]
async fn patch_dotenv_empty_deletes_file() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, None).await;
    rt.client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "dotenv": "FOO=bar" }))
        .send().await.unwrap();

    rt.client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "dotenv": "" }))
        .send().await.unwrap();

    let env_path = rt.workspace_path("ws1").join("alice/.env");
    assert!(!env_path.exists(), "expected .env deleted");
}

#[tokio::test]
async fn patch_dotenv_rejects_oversize() {
    let rt = TestRuntime::spawn_with_local_workspace("ws1").await;
    rt.add_agent("ws1", "alice", "claude", None, None).await;

    let big = "A".repeat(65 * 1024); // 65 KB > 64 KB cap
    let res = rt.client
        .patch(rt.url("/workspaces/ws1/agents/alice"))
        .json(&serde_json::json!({ "dotenv": big }))
        .send().await.unwrap();
    assert_eq!(res.status(), 400);
}
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p gitim-runtime --test agent_patch patch_dotenv -- --nocapture
```

期望：三个都 FAIL。

- [ ] **Step 3: 在 handler 里加 dotenv 处理**

在 handler 的 "Read + merge me.json" 块之后、"write me.json" 之前插入大小校验，并在内存 AgentInfo 更新之后写 `.env`：

在 `if let Some(env_map) = &req.env {` 之后加：

```rust
    // dotenv size cap (64 KB).
    if let Some(contents) = &req.dotenv {
        if contents.len() > 64 * 1024 {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "ok": false,
                "error": "dotenv exceeds 64 KB limit"
            }))).into_response();
        }
    }
```

在 "write me.json" 之后、"Update in-memory AgentInfo" 之前加 dotenv 落盘：

```rust
    if let Some(contents) = &req.dotenv {
        let env_path = repo_root.join(".env");
        if contents.is_empty() {
            if env_path.exists() {
                if let Err(e) = std::fs::remove_file(&env_path) {
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                        "ok": false, "error": format!("delete .env failed: {e}")
                    }))).into_response();
                }
            }
        } else {
            if let Err(e) = std::fs::write(&env_path, contents) {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                    "ok": false, "error": format!("write .env failed: {e}")
                }))).into_response();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = std::fs::metadata(&env_path).unwrap().permissions();
                perm.set_mode(0o600);
                let _ = std::fs::set_permissions(&env_path, perm);
            }
        }
    }
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p gitim-runtime --test agent_patch patch_dotenv -- --nocapture
```

期望：三个都 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/agent_patch.rs
git commit -m "feat(runtime): PATCH agent writes .env file with 0600 + 64KB cap

Empty string deletes the file. File path is <agent-clone>/.env —
picked up naturally by agent CLIs at their cwd. Not injected into
process env (that's the env field's job).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 2：后端 — `.gitignore` 管理

### Task 5：实现 `ensure_env_gitignored` helper

**Files:**
- Create: `crates/gitim-runtime/src/gitignore.rs`
- Modify: `crates/gitim-runtime/src/lib.rs` (声明模块)

- [ ] **Step 1: 写失败测试**

创建 `crates/gitim-runtime/src/gitignore.rs`：

```rust
//! Idempotent .gitignore management for the `.env` secrets convention.
//!
//! Called from workspace provisioning so every agent clone inherits the rule
//! via its shared remote. Separate module to keep workspace.rs / http.rs
//! focused on orchestration.

use std::path::Path;

/// Append `.env` to the repo's `.gitignore` if not already matched. Returns
/// `Ok(true)` if a change was made (caller should commit), `Ok(false)` if
/// already present.
pub fn ensure_env_gitignored(clone_root: &Path) -> std::io::Result<bool> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn appends_to_empty_gitignore() {
        let dir = tmpdir();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".env"));
    }

    #[test]
    fn idempotent_when_already_present() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn recognizes_slash_dot_env_form() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), "/.env\n").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn recognizes_dot_env_star_form() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), ".env*\n").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(!changed);
    }

    #[test]
    fn appends_with_trailing_newline_to_existing() {
        let dir = tmpdir();
        fs::write(dir.path().join(".gitignore"), "node_modules\ntarget").unwrap();
        let changed = ensure_env_gitignored(dir.path()).unwrap();
        assert!(changed);
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains("node_modules"));
        assert!(content.contains("target"));
        assert!(content.ends_with("\n"));
        assert!(content.contains(".env"));
    }
}
```

在 `crates/gitim-runtime/src/lib.rs` 加：

```rust
pub mod gitignore;
```

确保 `tempfile` 已在 `Cargo.toml` 的 `[dev-dependencies]` 里（如果没有，添加 `tempfile = "3"`）。

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p gitim-runtime --lib gitignore
```

期望：panic at `unimplemented!()`。

- [ ] **Step 3: 实现 helper**

替换 `ensure_env_gitignored`：

```rust
pub fn ensure_env_gitignored(clone_root: &Path) -> std::io::Result<bool> {
    let path = clone_root.join(".gitignore");
    let current = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    // Match any of: ".env", "/.env", ".env*" as standalone lines (ignoring
    // trailing whitespace and leading `!` negation — conservative: bail if
    // there's any negation since untangling semantics is not our job).
    for line in current.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() { continue; }
        if matches!(trimmed, ".env" | "/.env" | ".env*" | "/.env*") {
            return Ok(false);
        }
    }

    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(".env\n");
    std::fs::write(&path, next)?;
    Ok(true)
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p gitim-runtime --lib gitignore
```

期望：五个测试全 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gitim-runtime/src/gitignore.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/Cargo.toml
git commit -m "feat(runtime): idempotent .gitignore helper for .env rule

Recognizes .env / /.env / .env* forms as already-present. Appends the
rule when absent, handling missing trailing newline.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 6：集成到 workspace provisioning + 提交到 git

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:provision_local_workspace` (~line 2009)
- Modify: `crates/gitim-runtime/src/http.rs:provision_github_workspace` (~line 2128)
- Modify: `crates/gitim-runtime/tests/agent_patch.rs` 或新建 `tests/gitignore_init.rs`

- [ ] **Step 1: 写失败测试**

创建 `crates/gitim-runtime/tests/gitignore_init.rs`：

```rust
//! Verifies .gitignore contains .env after workspace init (local mode).

mod common;
use common::TestRuntime;

#[tokio::test]
async fn local_git_init_adds_env_to_gitignore_and_commits() {
    let rt = TestRuntime::spawn_empty().await;
    rt.git_init_local("ws1").await;

    let human_root = rt.workspace_path("ws1").join(".gitim-runtime/human");
    let gi = human_root.join(".gitignore");
    assert!(gi.exists(), ".gitignore missing at {:?}", gi);
    let content = std::fs::read_to_string(&gi).unwrap();
    assert!(content.contains(".env"), "content: {}", content);

    // Verify it's committed (git log --oneline -- .gitignore should have entry)
    let out = std::process::Command::new("git")
        .args(["log", "--oneline", "--", ".gitignore"])
        .current_dir(&human_root)
        .output()
        .unwrap();
    assert!(out.status.success());
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(!log.is_empty(), ".gitignore not committed");
}

#[tokio::test]
async fn local_git_init_idempotent_on_preexisting_env_rule() {
    let rt = TestRuntime::spawn_empty().await;
    rt.git_init_local("ws1").await;

    // Second init would re-provision; simulate repeated call by re-invoking
    // the guard directly. (Or: count commits touching .gitignore — expect 1.)
    let human_root = rt.workspace_path("ws1").join(".gitim-runtime/human");
    let out = std::process::Command::new("git")
        .args(["log", "--oneline", "--", ".gitignore"])
        .current_dir(&human_root)
        .output()
        .unwrap();
    let commits = String::from_utf8_lossy(&out.stdout).lines().count();
    assert_eq!(commits, 1, "expected exactly one .gitignore commit");
}
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p gitim-runtime --test gitignore_init -- --nocapture
```

期望：FAIL。

- [ ] **Step 3: 在 provision_local_workspace 和 provision_github_workspace 里加钩子**

在 `crates/gitim-runtime/src/http.rs` 文件顶部 use 列表里加：

```rust
use crate::gitignore::ensure_env_gitignored;
```

在 `provision_local_workspace` 里 `provision_human` 调用返回 `human_dir` 之后、`config.write` 之前加：

```rust
    apply_dotenv_gitignore(&human_dir);
```

在 `provision_github_workspace` 里 `provision_human` 返回 `final_human` 之后加相同调用（传 `&final_human`）。

在文件底部加 helper（保持 provisioning 函数简洁）：

```rust
/// Ensure the human clone's .gitignore excludes .env and commit if we added it.
/// Best-effort — failures are logged, not propagated; a missing rule is cosmetic,
/// the secret file is per-clone anyway.
fn apply_dotenv_gitignore(human_clone: &Path) {
    match ensure_env_gitignored(human_clone) {
        Ok(false) => {}
        Ok(true) => {
            let add = std::process::Command::new("git")
                .args(["add", ".gitignore"])
                .current_dir(human_clone)
                .output();
            if let Ok(o) = &add {
                if !o.status.success() {
                    tracing::warn!(stderr=%String::from_utf8_lossy(&o.stderr),
                        "git add .gitignore failed");
                    return;
                }
            }
            let commit = std::process::Command::new("git")
                .args(["commit", "-m", "chore: gitignore .env (runtime init)"])
                .current_dir(human_clone)
                .output();
            if let Ok(o) = &commit {
                if !o.status.success() {
                    tracing::warn!(stderr=%String::from_utf8_lossy(&o.stderr),
                        "git commit .gitignore failed");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error=%e, "ensure_env_gitignored failed");
        }
    }
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p gitim-runtime --test gitignore_init -- --nocapture
```

期望：两个都 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/gitignore_init.rs
git commit -m "feat(runtime): auto-add .env to .gitignore on workspace init

Applied in both local and github provisioning paths after provision_human.
Idempotent — re-init won't double-commit. Failure is logged, not fatal:
a missing rule is cosmetic since .env lives per-clone.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 3：前端 — Display 修复

### Task 7：`<ProviderBadge>` 组件

**Files:**
- Create: `webui-v2/src/components/management/provider-badge.tsx`
- Create: `webui-v2/src/components/management/provider-badge.test.tsx`

- [ ] **Step 1: 写失败测试**

创建 `webui-v2/src/components/management/provider-badge.test.tsx`：

```tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProviderBadge } from "./provider-badge";

describe("ProviderBadge", () => {
  it("renders claude label", () => {
    render(<ProviderBadge provider="claude" />);
    expect(screen.getByText("Claude")).toBeInTheDocument();
  });

  it("renders codex label", () => {
    render(<ProviderBadge provider="codex" />);
    expect(screen.getByText("Codex")).toBeInTheDocument();
  });

  it("renders opencode label", () => {
    render(<ProviderBadge provider="opencode" />);
    expect(screen.getByText("OpenCode")).toBeInTheDocument();
  });

  it("renders em-dash when undefined", () => {
    render(<ProviderBadge provider={undefined} />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cd webui-v2 && npx vitest run src/components/management/provider-badge.test.tsx
```

期望：module not found。

- [ ] **Step 3: 实现组件**

创建 `webui-v2/src/components/management/provider-badge.tsx`：

```tsx
import type { ProviderId } from "@/lib/providers";
import { PROVIDERS } from "@/lib/providers";

interface ProviderBadgeProps {
  provider: ProviderId | undefined;
}

const COLORS: Record<ProviderId, string> = {
  claude: "bg-orange-500/15 text-orange-400 border-orange-500/30",
  codex: "bg-purple-500/15 text-purple-400 border-purple-500/30",
  opencode: "bg-green-500/15 text-green-400 border-green-500/30",
};

export function ProviderBadge({ provider }: ProviderBadgeProps) {
  if (!provider) {
    return <span className="text-text-muted">—</span>;
  }
  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 rounded border text-xs font-medium ${COLORS[provider]}`}
    >
      {PROVIDERS[provider].label}
    </span>
  );
}
```

- [ ] **Step 4: 运行测试**

```bash
cd webui-v2 && npx vitest run src/components/management/provider-badge.test.tsx
```

期望：四个测试全 PASS。

- [ ] **Step 5: 提交**

```bash
git add webui-v2/src/components/management/provider-badge.tsx webui-v2/src/components/management/provider-badge.test.tsx
git commit -m "feat(webui): ProviderBadge component

Orange/purple/green per provider; em-dash placeholder for undefined.
Label text comes from PROVIDERS registry — single source of truth.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 8：AgentDetail — 加 Provider 字段 + 修 Model fallback

**Files:**
- Modify: `webui-v2/src/components/management/agent-detail.tsx:120-144`

- [ ] **Step 1: 手动验证现状**

跑 dev server：
```bash
cd webui-v2 && pnpm dev
```

打开 detail 页，确认：
- 没有 Provider 字段
- opencode agent 的 model 显示为 "claude-sonnet-4-6"（bug）

- [ ] **Step 2: 改 agent-detail.tsx**

Edit `webui-v2/src/components/management/agent-detail.tsx`，替换 info grid 块（line 122-144）：

```tsx
      {/* Info grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-5 mb-8 p-5 rounded-xl border border-border bg-card/50">
        <Field label="Repo Path">
          <code className="text-sm font-mono text-text-secondary bg-background/60 px-2 py-1 rounded">
            {agent.repoPath}
          </code>
        </Field>

        <Field label="Provider">
          <ProviderBadge provider={agent.provider} />
        </Field>

        <Field label="Model">
          {agent.model ? (
            <span className="inline-flex items-center px-2 py-0.5 rounded bg-background/60 border border-border text-sm font-mono">
              {agent.model}
            </span>
          ) : agent.provider === "opencode" ? (
            <span className="text-text-muted italic text-sm">
              Default (from opencode auth login)
            </span>
          ) : (
            <span className="text-text-muted">—</span>
          )}
        </Field>

        <Field label="Messages Processed">
          <span className="text-lg font-semibold">{agent.messagesProcessed}</span>
        </Field>

        <Field label="Last Activity">
          <span className="text-text-secondary">
            {agent.lastActivity ? relativeTime(agent.lastActivity) : "—"}
          </span>
        </Field>
      </div>
```

加 import（文件顶部）：

```tsx
import { ProviderBadge } from "./provider-badge";
```

- [ ] **Step 3: 手动回归**

```bash
cd webui-v2 && pnpm dev
```

打开三个 agent（claude / codex / opencode）的 detail 页，确认：
- Provider 字段显示正确 badge
- Claude agent 的 model 显示具体 model id
- OpenCode agent 的 model 显示 "Default (from opencode auth login)"
- 没有 agent 显示 "claude-sonnet-4-6" 错误 fallback

- [ ] **Step 4: 跑类型检查和 lint**

```bash
cd webui-v2 && pnpm tsc --noEmit && pnpm lint
```

期望：无错误。

- [ ] **Step 5: 提交**

```bash
git add webui-v2/src/components/management/agent-detail.tsx
git commit -m "fix(webui): show Provider on agent detail + remove Sonnet fallback lie

Previous: \`agent.model ?? \"claude-sonnet-4-6\"\` claimed every
OpenCode agent ran Sonnet. Now: three-way render — model id, opencode
default hint, or em-dash.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 9：AgentCard — 加 provider · model 小字行

**Files:**
- Modify: `webui-v2/src/components/management/agent-card.tsx:109-129`

- [ ] **Step 1: 改 agent-card.tsx**

Edit `webui-v2/src/components/management/agent-card.tsx` 的 CardHeader（line 109-129）：

替换 name 容器，在 name span 下方加 provider/model 小字行：

```tsx
              <div className="min-w-0">
                <span className="font-semibold text-lg truncate block">{agent.name}</span>
                <span className="text-xs text-text-muted truncate block">
                  {agent.provider ?? "—"} ·{" "}
                  {agent.model ??
                    (agent.provider === "opencode" ? "default" : "—")}
                </span>
                {agent.status === "error" && (
                  <p className="text-xs text-destructive truncate">
                    {agent.errorMessage ?? "unknown error"}
                  </p>
                )}
              </div>
```

- [ ] **Step 2: 手动回归**

```bash
cd webui-v2 && pnpm dev
```

/management 页，确认每张卡片的名字下方都有一行小字显示 `provider · model`（opencode 显示为 `opencode · default`）。

- [ ] **Step 3: 跑类型检查**

```bash
cd webui-v2 && pnpm tsc --noEmit
```

- [ ] **Step 4: 提交**

```bash
git add webui-v2/src/components/management/agent-card.tsx
git commit -m "feat(webui): AgentCard shows provider · model subline

Matches detail page fix — users can now see the provider stack at a
glance without drilling in.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 4：前端 — 共享组件 + API client

### Task 10：抽 `<EnvVarsEditor>` 共享组件

**Files:**
- Create: `webui-v2/src/components/management/env-vars-editor.tsx`
- Modify: `webui-v2/src/components/management/add-agent-dialog.tsx` (迁移)

- [ ] **Step 1: 创建共享组件**

创建 `webui-v2/src/components/management/env-vars-editor.tsx`：

```tsx
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

export interface EnvVar {
  key: string;
  value: string;
}

interface EnvVarsEditorProps {
  value: EnvVar[];
  onChange: (vars: EnvVar[]) => void;
}

export function EnvVarsEditor({ value, onChange }: EnvVarsEditorProps) {
  return (
    <div className="space-y-2">
      {value.map((pair, i) => (
        <div key={i} className="flex gap-2">
          <Input
            placeholder="KEY"
            value={pair.key}
            onChange={(e) => {
              const updated = [...value];
              updated[i] = { ...updated[i], key: e.target.value };
              onChange(updated);
            }}
            className="flex-1 font-mono text-xs"
          />
          <Input
            placeholder="value"
            value={pair.value}
            onChange={(e) => {
              const updated = [...value];
              updated[i] = { ...updated[i], value: e.target.value };
              onChange(updated);
            }}
            className="flex-1 font-mono text-xs"
          />
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => onChange(value.filter((_, j) => j !== i))}
            className="px-2 text-muted-foreground hover:text-destructive"
          >
            ×
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...value, { key: "", value: "" }])}
      >
        + Add Variable
      </Button>
    </div>
  );
}
```

- [ ] **Step 2: 迁移 AddAgentDialog 使用共享组件**

修改 `webui-v2/src/components/management/add-agent-dialog.tsx`：

- 删除内联的 env KV UI（line ~285-335 块）
- import `EnvVarsEditor`
- 替换为 `<EnvVarsEditor value={envVars} onChange={setEnvVars} />`

具体：删除 line 289-335（`<div className="space-y-2">...</div>` 整块），替换为：

```tsx
              <EnvVarsEditor value={envVars} onChange={setEnvVars} />
```

顶部 import 增加：

```tsx
import { EnvVarsEditor } from "./env-vars-editor";
```

- [ ] **Step 3: 手动回归 AddAgent**

```bash
cd webui-v2 && pnpm dev
```

/management → Add Agent 弹窗 → 增删 env vars → 行为与之前一致。

- [ ] **Step 4: 跑类型检查**

```bash
cd webui-v2 && pnpm tsc --noEmit
```

- [ ] **Step 5: 提交**

```bash
git add webui-v2/src/components/management/env-vars-editor.tsx webui-v2/src/components/management/add-agent-dialog.tsx
git commit -m "refactor(webui): extract EnvVarsEditor shared component

Both AddAgentDialog and upcoming AgentDetail edit mode need KV list
UI. Extract now to avoid copy-paste later.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 11：`client.updateAgent` + store action

**Files:**
- Modify: `webui-v2/src/lib/client.ts`
- Modify: `webui-v2/src/hooks/use-agent-store.ts` (如需新增 action)

- [ ] **Step 1: 检查 store 是否已有 updateAgent**

```bash
grep -n "updateAgent" webui-v2/src/hooks/use-agent-store.ts
```

Design 里标注 "已有，复用"。如果确实有 `updateAgent(id, patch)` 接受 `Partial<Agent>`，跳到 Step 2。如果没有，先加：

```ts
updateAgent: (id: string, patch: Partial<Agent>) => void;

updateAgent: (id, patch) =>
  set((state) => ({
    agents: state.agents.map((a) =>
      a.id === id ? { ...a, ...patch } : a
    ),
  })),
```

- [ ] **Step 2: 在 client.ts 加 `updateAgent`**

在 `webui-v2/src/lib/client.ts` 的 `addAgent` 函数之后（~line 568）加：

```ts
export async function updateAgent(
  slug: string,
  agentId: string,
  patch: {
    system_prompt?: string | null;
    env?: Record<string, string>;
    dotenv?: string;
  },
): Promise<ApiResponse<{ agent: Agent }>> {
  try {
    const res = await fetch(`${wsBase(slug)}/agents/${agentId}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    });
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: mapBackendAgent(data.agent) } };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}
```

- [ ] **Step 3: 跑类型检查**

```bash
cd webui-v2 && pnpm tsc --noEmit
```

- [ ] **Step 4: 提交**

```bash
git add webui-v2/src/lib/client.ts webui-v2/src/hooks/use-agent-store.ts
git commit -m "feat(webui): client.updateAgent for PATCH endpoint

Thin wrapper over fetch; shares mapBackendAgent with other agent APIs
to keep Agent shape consistent. No fallback to mock — PATCH is
runtime-only (creation can fall back; updates can't meaningfully).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 5：前端 — Edit 模式

### Task 12：AgentDetail Edit 状态机 + 表单（不含 Save）

**Files:**
- Modify: `webui-v2/src/components/management/agent-detail.tsx`

- [ ] **Step 1: 加 `mode` state + Edit 按钮**

Edit `agent-detail.tsx` 顶部 imports 加：

```tsx
import { Textarea } from "@/components/ui/textarea";
import { EnvVarsEditor, type EnvVar } from "./env-vars-editor";
import { Pencil } from "lucide-react";
```

在组件内现有 useState 后面加：

```tsx
  type Mode = "view" | "edit" | "saving";
  const [mode, setMode] = useState<Mode>("view");
  const [draftPrompt, setDraftPrompt] = useState("");
  const [draftEnv, setDraftEnv] = useState<EnvVar[]>([]);
  const [draftDotenv, setDraftDotenv] = useState("");
  const [editError, setEditError] = useState<string | null>(null);

  function enterEditMode() {
    if (!agent) return;
    setDraftPrompt(agent.systemPrompt ?? "");
    setDraftEnv(
      Object.entries(agent.env ?? {}).map(([key, value]) => ({ key, value })),
    );
    setDraftDotenv(""); // We never read .env back — users type fresh content
    setEditError(null);
    setMode("edit");
  }

  function cancelEdit() {
    setMode("view");
    setEditError(null);
  }
```

注：dotenv 故意不回显——后端也不返回 `.env` 文件内容（secrets），UI 里进入编辑即空白，避免秘钥泄露到前端 state。用户要完整重写内容才能保存。这点会在说明文案里写明。

- [ ] **Step 2: 在 header 右侧加 Edit 按钮（mode === "view" 时）**

替换 Header 块（line 102-119 附近的 `{/* Header */}`）的结构，把 status badge 所在 div 改成 flex 容器：

```tsx
      {/* Header */}
      <div className="flex items-start gap-4 mb-8">
        <div
          className="w-16 h-16 rounded-2xl flex items-center justify-center text-xl font-bold text-white shadow-lg"
          style={{ backgroundColor: avatarColor(agent.name || agent.id) }}
        >
          {initials(agent.name || agent.id)}
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-3 flex-wrap">
            <h1 className="text-2xl font-semibold tracking-tight">{agent.name}</h1>
            {statusBadge(agent.status)}
          </div>
          <p className="text-sm text-text-muted mt-1 font-mono truncate">
            {agent.id}
          </p>
        </div>
        {mode === "view" && (
          <Button
            variant="outline"
            size="sm"
            onClick={enterEditMode}
            className="border-border-strong hover:bg-surface-hover"
          >
            <Pencil className="size-4 mr-1.5" />
            Edit
          </Button>
        )}
      </div>
```

- [ ] **Step 3: 加 edit 模式下的表单区**

替换 System Prompt block（line ~146-155）和 Environment Variables block（line ~157-172），成条件渲染：

```tsx
      {/* System Prompt */}
      <div className="mb-8">
        <Field label="System Prompt">
          {mode === "view" ? (
            <div className="mt-2 rounded-xl border border-border bg-card/50 p-4">
              <pre className="text-sm whitespace-pre-wrap font-mono break-words text-text-secondary leading-relaxed">
                {agent.systemPrompt || "(none)"}
              </pre>
            </div>
          ) : (
            <Textarea
              value={draftPrompt}
              onChange={(e) => setDraftPrompt(e.target.value)}
              rows={4}
              className="mt-2 font-mono text-sm"
              placeholder="Describe the agent's role and behavior…"
            />
          )}
        </Field>
      </div>

      {/* Environment Variables */}
      <div className="mb-8">
        <Field label="Environment Variables">
          <p className="text-xs text-text-muted mt-1 mb-2">
            Injected as process env vars to the agent CLI. Flat key-value.
          </p>
          {mode === "view" ? (
            agent.env && Object.keys(agent.env).length > 0 ? (
              <div className="mt-2 rounded-xl border border-border bg-card/50 p-4 space-y-2">
                {Object.entries(agent.env).map(([key, value]) => (
                  <div key={key} className="text-sm font-mono flex items-center gap-2">
                    <span className="text-primary font-medium">{key}</span>
                    <span className="text-text-muted">=</span>
                    <span className="text-text-secondary">{value}</span>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-sm text-text-muted mt-2">(none)</p>
            )
          ) : (
            <EnvVarsEditor value={draftEnv} onChange={setDraftEnv} />
          )}
        </Field>
      </div>

      {/* Secrets (.env file) — only shown in edit mode */}
      {mode !== "view" && (
        <div className="mb-8">
          <Field label="Secrets (.env file)">
            <p className="text-xs text-text-muted mt-1 mb-2">
              Written to <code>&lt;agent-clone&gt;/.env</code> (gitignored).
              Agent reads via <code>source .env</code>, dotenv libraries, or{" "}
              <code>cat</code> at runtime. Use for API keys and multi-line
              secrets. Contents are <strong>not</strong> shown here — leave
              empty to keep the existing file; type to replace; submit empty
              after editing to delete.
            </p>
            <Textarea
              value={draftDotenv}
              onChange={(e) => setDraftDotenv(e.target.value)}
              rows={8}
              className="mt-2 font-mono text-xs"
              placeholder="OPENAI_API_KEY=sk-..."
            />
          </Field>
        </div>
      )}
```

（注意：旧的 `{agent.env && Object.keys(agent.env).length > 0 && (...)}` 条件渲染整块被新的 Environment Variables block 替换。）

- [ ] **Step 4: 手动回归**

```bash
cd webui-v2 && pnpm dev
```

打开 detail 页 → 点 Edit → 确认：
- System Prompt 变 textarea
- Env Vars 变 EnvVarsEditor
- Secrets (.env file) 出现 textarea，说明文案显示

- [ ] **Step 5: 类型检查 + 提交**

```bash
cd webui-v2 && pnpm tsc --noEmit
```

```bash
git add webui-v2/src/components/management/agent-detail.tsx
git commit -m "feat(webui): AgentDetail edit mode state machine (no save yet)

Edit button enters edit mode; textareas for system prompt + .env;
EnvVarsEditor for env vars. Save action lands in next task.
.env content intentionally never read back — enforces secrets
stay on disk only.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 13：AgentDetail — Save + 生效提示 + Cancel

**Files:**
- Modify: `webui-v2/src/components/management/agent-detail.tsx`

- [ ] **Step 1: 加 Save/Cancel 按钮 + handleSave 函数**

在 `cancelEdit` 之后加：

```tsx
  async function handleSave() {
    if (!activeSlug || !agent) return;

    // Build patch with only changed fields (so backend merge is minimal).
    const patch: {
      system_prompt?: string | null;
      env?: Record<string, string>;
      dotenv?: string;
    } = {};

    const newPrompt = draftPrompt.trim();
    const oldPrompt = (agent.systemPrompt ?? "").trim();
    const promptChanged = newPrompt !== oldPrompt;
    if (promptChanged) {
      patch.system_prompt = newPrompt === "" ? null : newPrompt;
    }

    const newEnv: Record<string, string> = {};
    for (const { key, value } of draftEnv) {
      const k = key.trim();
      if (k) newEnv[k] = value;
    }
    const oldEnv = agent.env ?? {};
    const envChanged =
      Object.keys(newEnv).length !== Object.keys(oldEnv).length ||
      Object.entries(newEnv).some(([k, v]) => oldEnv[k] !== v);
    if (envChanged) patch.env = newEnv;

    const dotenvChanged = draftDotenv.length > 0;
    if (dotenvChanged) patch.dotenv = draftDotenv;

    if (Object.keys(patch).length === 0) {
      setMode("view");
      return;
    }

    setMode("saving");
    setEditError(null);
    const res = await client.updateAgent(activeSlug, agent.id, patch);
    if (res.ok && res.data?.agent) {
      updateAgent(agent.id, res.data.agent as Partial<Agent>);
      setMode("view");

      // Generation-aware toast lines.
      const lines: string[] = [];
      if (envChanged || dotenvChanged) {
        lines.push("Environment & .env → take effect on next message");
      }
      if (promptChanged) {
        lines.push(
          "System prompt → takes effect on next session (auto-rolls when current session fills)",
        );
      }
      toast.success("Saved", { description: lines.join("\n") });
    } else {
      setEditError(res.error ?? "Save failed");
      setMode("edit");
    }
  }
```

- [ ] **Step 2: 在 Actions 区加 Edit-mode 按钮组**

改 Actions 区（line ~202-225），整个块替换：

```tsx
      {/* Actions */}
      <div className="flex gap-3">
        {mode === "view" ? (
          <>
            <Button
              variant={isRunning ? "outline" : "default"}
              size="default"
              onClick={handleToggle}
              className={isRunning ? "border-border-strong hover:bg-surface-hover" : ""}
            >
              {isRunning ? (
                <><Pause className="size-4 mr-1.5" /> Stop</>
              ) : (
                <><Play className="size-4 mr-1.5" /> Start</>
              )}
            </Button>
            <Button
              variant="ghost"
              size="default"
              onClick={() => setRemoveOpen(true)}
              className="text-destructive hover:text-destructive hover:bg-destructive/10"
            >
              <Trash2 className="size-4 mr-1.5" />
              Remove
            </Button>
          </>
        ) : (
          <>
            <Button
              variant="default"
              size="default"
              onClick={handleSave}
              disabled={mode === "saving"}
            >
              {mode === "saving" ? "Saving…" : "Save"}
            </Button>
            <Button
              variant="outline"
              size="default"
              onClick={cancelEdit}
              disabled={mode === "saving"}
            >
              Cancel
            </Button>
          </>
        )}
      </div>

      {editError && (
        <div className="mt-3 p-3 rounded-lg border border-destructive/30 bg-destructive/10 text-sm text-destructive">
          {editError}
        </div>
      )}
```

- [ ] **Step 3: 端到端回归**

启动 runtime + WebUI：

```bash
# terminal 1
cargo run -p gitim-runtime --bin runtime
# terminal 2
cd webui-v2 && pnpm dev
```

创建一个 agent → detail 页 Edit → 改 system prompt → Save → 看 toast 显示 "System prompt → takes effect on next session"；刷新页面看新值保留。

再测 env → Save → toast 显示 "Environment & .env → take effect on next message"。

再测 .env 空白 → 不在 patch 里（dotenv 留空代表不动，这是故意行为，避免误删）。

- [ ] **Step 4: 类型检查**

```bash
cd webui-v2 && pnpm tsc --noEmit
```

- [ ] **Step 5: 提交**

```bash
git add webui-v2/src/components/management/agent-detail.tsx
git commit -m "feat(webui): AgentDetail save + generation-aware toast

Only changed fields are sent in PATCH. Toast distinguishes env/.env
(next message) from system_prompt (next session) semantics so users
don't expect mid-session reconfiguration.

Empty .env textarea = keep existing file (not delete), so users
editing only system_prompt can't accidentally wipe secrets.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 14：Unsaved changes 守卫

**Files:**
- Modify: `webui-v2/src/components/management/agent-detail.tsx`

- [ ] **Step 1: 加 isDirty 检测 + beforeunload 绑定**

在现有 useState 块之后加：

```tsx
  const isDirty =
    mode === "edit" &&
    agent !== undefined &&
    (draftPrompt.trim() !== (agent.systemPrompt ?? "").trim() ||
      draftDotenv.length > 0 ||
      (() => {
        const newEnv: Record<string, string> = {};
        for (const { key, value } of draftEnv) {
          const k = key.trim();
          if (k) newEnv[k] = value;
        }
        const oldEnv = agent.env ?? {};
        return (
          Object.keys(newEnv).length !== Object.keys(oldEnv).length ||
          Object.entries(newEnv).some(([k, v]) => oldEnv[k] !== v)
        );
      })());

  React.useEffect(() => {
    if (!isDirty) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [isDirty]);
```

顶部 import 加：

```tsx
import * as React from "react";
```

- [ ] **Step 2: Cancel 和 Back 加二次确认**

修改 `cancelEdit` 函数：

```tsx
  function cancelEdit() {
    if (isDirty) {
      const confirmed = window.confirm(
        "Discard unsaved changes?",
      );
      if (!confirmed) return;
    }
    setMode("view");
    setEditError(null);
  }
```

修改顶部 Back 按钮的 onClick：

```tsx
        onClick={() => {
          if (isDirty) {
            const confirmed = window.confirm("Discard unsaved changes?");
            if (!confirmed) return;
          }
          navigate("/management");
        }}
```

- [ ] **Step 3: 手动回归**

dev server → Edit → 改内容 → 点 Cancel → 弹 confirm 确认。  
Edit → 改内容 → 点 Back → 弹 confirm 确认。  
Edit → 改内容 → 刷新页面 → 浏览器原生 beforeunload 确认。

- [ ] **Step 4: 类型检查**

```bash
cd webui-v2 && pnpm tsc --noEmit
```

- [ ] **Step 5: 提交**

```bash
git add webui-v2/src/components/management/agent-detail.tsx
git commit -m "feat(webui): unsaved changes guard on AgentDetail edit

window.confirm on Cancel / Back nav; native beforeunload on page
close / reload. Dirty check computes same diff as the Save patch
logic to keep semantics consistent.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 6：收尾

### Task 15：全量测试 + 更新 orientation

**Files:**
- Modify: `CLAUDE.md` (Current Orientation 块)

- [ ] **Step 1: 跑全量 backend 测试**

```bash
cargo test -p gitim-runtime
```

期望：全 PASS，无 regression。如果有挂掉的测试，先修再继续。

- [ ] **Step 2: 跑全量 core + daemon 测试（确认未影响其它 crate）**

```bash
cargo test -p gitim-core
cargo test -p gitim-daemon
```

- [ ] **Step 3: WebUI 类型检查 + lint + 测试**

```bash
cd webui-v2 && pnpm tsc --noEmit && pnpm lint && pnpm vitest run
```

- [ ] **Step 4: 更新 CLAUDE.md 的 Current Orientation**

在 `CLAUDE.md` 最底部 `## Current Orientation` 块里追加一行到 "Learnings"（保持现有其它字段不变）：

```markdown
- Agent 配置（system_prompt / env / .env secrets）创建后可通过 detail 页 Edit 模式修改；provider/model 仍然 immutable（session 迁移未解决）
```

"Where we are" 那行末尾加：

```
Agent 配置可编辑 + .env secrets 落盘落地。
```

- [ ] **Step 5: 最终提交**

```bash
git add CLAUDE.md
git commit -m "docs(orientation): agent config editable landed

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Checklist

- [x] **Spec coverage**：设计文档的每个 Section 都有对应 task
  - Section 1（PATCH endpoint）→ Tasks 1-4
  - Section 2（.env + .gitignore）→ Tasks 5-6（加上 Task 4 的 .env 落盘）
  - Section 3（detail + card display fix）→ Tasks 7-9
  - Section 4（Edit 模式）→ Tasks 10-14
  - Testing plan（backend / frontend）→ 分散在各 task 的 TDD 步骤

- [x] **No placeholders**：无 TBD / TODO / "similar to" / 空函数签名

- [x] **Type consistency**：
  - `AgentUpdateRequest` 字段 (`system_prompt: Option<Option<String>>`, `env: Option<HashMap>`, `dotenv: Option<String>`) 在 Tasks 1-4 保持一致
  - 前端 `updateAgent(slug, agentId, patch)` 签名在 Tasks 11, 13 一致
  - `EnvVar { key, value }` 在 Tasks 10, 12-14 一致

- [x] **生效语义**：每条 toast 文案 / 说明文案明确区分 env/.env（下条消息）和 system_prompt（下个 session）

- [x] **幂等性**：`ensure_env_gitignored` 是幂等的（Task 5），PATCH 空 body 无副作用（Task 2）

- [x] **CLAUDE.md 节奏**：任务开头（Task 1 Step 2）/ 结尾（Task 15）各跑一次全量，中间只跑相关 crate 或 test 文件

---

## Execution Handoff

**Plan complete and saved to [`docs/plans/2026-04-21-agent-config-editable/plan.md`](./plan.md).**

**Two execution options:**

1. **Subagent-Driven (recommended)** — 主 agent 每 task 派 fresh subagent；task 之间 review；快速迭代。
2. **Inline Execution** — 当前 session 顺序跑所有 task，按 Phase 设 checkpoint。

哪种？
