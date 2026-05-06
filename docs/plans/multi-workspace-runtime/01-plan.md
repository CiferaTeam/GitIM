# 单 Runtime 多 Workspace — 实施计划

> **Goal:** 把 `gitim-runtime` 从"单 workspace"改造为"一个进程 + 一个端口服务多个 workspace"。
>
> **Architecture:** 顶层 `RuntimeState` 持有 `HashMap<Slug, WorkspaceContext>`;HTTP 路由用 axum `.nest("/workspaces/:slug", ws_router)` 切分全局 vs workspace-scoped;SSE per-ws broadcast;前端通过 path 前缀定位 workspace。
>
> **Tech Stack:** Rust 2021 / axum / tokio / serde / React 19 / zustand / Vite。

---

## File Structure

**新增**:
- `crates/gitim-runtime/src/slug.rs` — slug 生成 / 规范化 / 冲突解决 / 校验
- `crates/gitim-runtime/src/workspace.rs` — `WorkspaceContext` 定义 + 构造 + 关闭
- `crates/gitim-runtime/src/user_config.rs` — `~/.gitim/runtime.json` schema v2 读写(`{ workspaces: [...] }`)
- `crates/gitim-runtime/tests/http_workspaces.rs` — HTTP integration test 骨架
- `crates/gitim-runtime/tests/multi_workspace.rs` — 多 ws recover / delete / isolation
- `webui-v2/src/components/workspace-switcher.tsx` — 左上角切换器
- `webui-v2/src/hooks/use-workspace-store.ts` — zustand store 存 workspaces 列表 + active slug

**修改**:
- `crates/gitim-runtime/src/lib.rs` — `pub mod slug / workspace / user_config`
- `crates/gitim-runtime/src/http.rs` — 大改:`RuntimeState` 重构、路由 nest、所有 handler 重签名
- `crates/gitim-runtime/src/agent_loop.rs` — `AgentActivityEvent.workspace_id` 填充
- `crates/gitim-runtime/src/token_propagation.rs` — 签名改 `fn propagate_all(workspaces: &[PathBuf])` 或调用方迭代
- `webui-v2/src/lib/client.ts` — 所有 API 方法按签名加 `slug`
- `webui-v2/src/hooks/use-connection-store.ts` — 去掉 workspace path(workspace 列表走新 store)
- `webui-v2/src/hooks/use-agent-activity.ts` — SSE URL 加 slug
- `webui-v2/src/app.tsx` — 接入 workspace switcher

---

## Task 1: Slug 模块

**Files:**
- Create: `crates/gitim-runtime/src/slug.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`(`pub mod slug;`)

**Tests(在同文件 `#[cfg(test)] mod tests`):**

```rust
#[test] fn normalize_basic() { assert_eq!(normalize("Frontend"), "frontend"); }
#[test] fn normalize_spaces() { assert_eq!(normalize("My workspace"), "my-workspace"); }
#[test] fn normalize_unicode() { assert_eq!(normalize("前端"), "workspace"); } // 非 ASCII 全变 `-` → 折叠 → 空 → fallback
#[test] fn normalize_collapses() { assert_eq!(normalize("a---b"), "a-b"); }
#[test] fn normalize_trims() { assert_eq!(normalize("-foo-"), "foo"); }
#[test] fn normalize_truncates() { assert_eq!(normalize(&"x".repeat(100)).len(), 32); }
#[test] fn normalize_empty_fallback() { assert_eq!(normalize(""), "workspace"); }

#[test] fn resolve_conflict_no_conflict() {
    let existing: HashSet<String> = HashSet::new();
    assert_eq!(resolve("foo", &existing), "foo");
}
#[test] fn resolve_conflict_appends_2() {
    let existing: HashSet<String> = ["foo"].into_iter().map(String::from).collect();
    assert_eq!(resolve("foo", &existing), "foo-2");
}
#[test] fn resolve_conflict_skips_taken_suffixes() {
    let existing = ["foo", "foo-2", "foo-3"].into_iter().map(String::from).collect();
    assert_eq!(resolve("foo", &existing), "foo-4");
}
#[test] fn resolve_reserved_keyword() {
    let existing: HashSet<String> = HashSet::new();
    assert_eq!(resolve("default", &existing), "default-2"); // reserved → 直接跳到 -2
}

#[test] fn validate_accepts() { assert!(validate("foo-bar").is_ok()); }
#[test] fn validate_rejects_uppercase() { assert!(validate("Foo").is_err()); }
#[test] fn validate_rejects_slash() { assert!(validate("foo/bar").is_err()); }
#[test] fn validate_rejects_dotdot() { assert!(validate("..").is_err()); }
#[test] fn validate_rejects_empty() { assert!(validate("").is_err()); }
#[test] fn validate_rejects_over_32() { assert!(validate(&"x".repeat(33)).is_err()); }
```

**Implementation:**

```rust
// crates/gitim-runtime/src/slug.rs
use std::collections::HashSet;

const RESERVED: &[&str] = &["default", "system", "active", "current"];
const MAX_LEN: usize = 32;
const FALLBACK: &str = "workspace";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SlugError {
    #[error("slug empty")]
    Empty,
    #[error("slug too long (max {MAX_LEN})")]
    TooLong,
    #[error("slug contains invalid characters (allowed: a-z 0-9 -)")]
    InvalidChars,
}

/// Normalize a raw string (e.g. directory basename) into slug form.
/// Rules: lowercase → replace non-[a-z0-9-] with `-` → collapse repeats →
/// trim `-` → truncate to 32 → empty falls back to "workspace".
pub fn normalize(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let replaced: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    // Collapse consecutive `-`
    let mut collapsed = String::with_capacity(replaced.len());
    let mut prev_dash = false;
    for c in replaced.chars() {
        if c == '-' {
            if !prev_dash { collapsed.push(c); }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    let truncated: String = trimmed.chars().take(MAX_LEN).collect();
    // Ensure truncated didn't leave a trailing `-`
    let truncated = truncated.trim_end_matches('-').to_string();
    if truncated.is_empty() {
        FALLBACK.to_string()
    } else {
        truncated
    }
}

/// Resolve a slug collision by appending `-2`, `-3`, ... Reserved keywords
/// always collide — so `default` becomes `default-2`.
pub fn resolve(candidate: &str, existing: &HashSet<String>) -> String {
    let reserved = RESERVED.contains(&candidate);
    if !reserved && !existing.contains(candidate) {
        return candidate.to_string();
    }
    let mut n = 2u32;
    loop {
        let suffixed = format!("{candidate}-{n}");
        if !existing.contains(&suffixed) && !RESERVED.contains(&suffixed.as_str()) {
            return suffixed;
        }
        n += 1;
    }
}

pub fn validate(slug: &str) -> Result<(), SlugError> {
    if slug.is_empty() { return Err(SlugError::Empty); }
    if slug.len() > MAX_LEN { return Err(SlugError::TooLong); }
    if !slug.chars().all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase()) || c == '-') {
        return Err(SlugError::InvalidChars);
    }
    Ok(())
}
```

**Verify:** `cargo test -p gitim-runtime slug::tests` — all pass.

**Commit:** `feat(runtime): slug module for multi-workspace support`

---

## Task 2: UserConfig schema v2(`~/.gitim/runtime.json`)

**Files:**
- Create: `crates/gitim-runtime/src/user_config.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`(`pub mod user_config;`)

**New schema:**

```json
{
  "workspaces": [
    { "slug": "frontend", "workspace_name": "Frontend 主线", "path": "/abs/path" }
  ]
}
```

无兼容旧 `{ workspace: "..." }` 的代码 — 按决策摘要 "从 0 原生支持"。

**Tests (`#[cfg(test)] mod tests`)** — 用 `tempfile::TempDir` 隔离 home:

```rust
#[test] fn read_missing_returns_empty()
#[test] fn read_parse_error_returns_empty()
#[test] fn write_then_read_roundtrip()
#[test] fn upsert_adds_entry()
#[test] fn upsert_updates_existing_by_slug()
#[test] fn remove_by_slug()
#[test] fn write_creates_parent_dir()
```

**Implementation:**

```rust
// crates/gitim-runtime/src/user_config.rs
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub slug: String,
    pub workspace_name: String,
    pub path: String,  // absolute
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}

/// `~/.gitim/runtime.json`
pub fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gitim/runtime.json"))
}

/// Read config, returning `UserConfig::default()` if missing or malformed.
pub fn read() -> UserConfig { read_from(config_path().as_deref()) }

pub fn read_from(path: Option<&Path>) -> UserConfig {
    let Some(p) = path else { return UserConfig::default(); };
    let Ok(content) = std::fs::read_to_string(p) else { return UserConfig::default(); };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn write(cfg: &UserConfig) -> std::io::Result<()> {
    if let Some(p) = config_path() { write_to(cfg, &p) } else { Ok(()) }
}

pub fn write_to(cfg: &UserConfig, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let json = serde_json::to_string_pretty(cfg).unwrap();
    std::fs::write(path, json)
}

impl UserConfig {
    pub fn upsert(&mut self, entry: WorkspaceEntry) {
        if let Some(existing) = self.workspaces.iter_mut().find(|e| e.slug == entry.slug) {
            *existing = entry;
        } else {
            self.workspaces.push(entry);
        }
    }
    pub fn remove(&mut self, slug: &str) -> bool {
        let before = self.workspaces.len();
        self.workspaces.retain(|e| e.slug != slug);
        self.workspaces.len() != before
    }
}
```

**Verify:** `cargo test -p gitim-runtime user_config::tests` — all pass.

**Commit:** `feat(runtime): user config schema v2 with workspace list`

---

## Task 3: WorkspaceContext + RuntimeState 重构

**核心改造 — 是所有后续 task 的前置依赖。**

**Files:**
- Create: `crates/gitim-runtime/src/workspace.rs`
- Modify: `crates/gitim-runtime/src/http.rs`(`RuntimeState` 重构)
- Modify: `crates/gitim-runtime/src/lib.rs`(`pub mod workspace;`)

**New `WorkspaceContext`:**

```rust
// crates/gitim-runtime/src/workspace.rs
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use tokio::sync::broadcast;

use crate::http::{AgentActivityEvent, AgentInfo};
use crate::git_config::WorkspaceConfig;

pub struct WorkspaceContext {
    pub slug: String,
    pub workspace_name: String,
    pub path: PathBuf,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
    pub agents: HashMap<String, AgentInfo>,
    pub activity_tx: broadcast::Sender<AgentActivityEvent>,
    /// Flipped by sync_loop after 3 consecutive auth failures; per-workspace so
    /// one broken PAT doesn't mute sync for other workspaces.
    pub auth_failed: Arc<AtomicBool>,
    pub git_config: Option<WorkspaceConfig>,
}

impl WorkspaceContext {
    pub fn new(slug: String, workspace_name: String, path: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(128);
        Self {
            slug, workspace_name, path,
            human_repo: None,
            poll_cursor: None,
            agents: HashMap::new(),
            activity_tx: tx,
            auth_failed: Arc::new(AtomicBool::new(false)),
            git_config: None,
        }
    }
}
```

**Modified `RuntimeState`** (`crates/gitim-runtime/src/http.rs:104-146`):

```rust
pub struct RuntimeState {
    pub workspaces: HashMap<String, WorkspaceContext>,
    /// Global idle watchdog — any workspace's activity bumps this.
    pub last_activity: std::sync::atomic::AtomicU64,
    pub github_api: Arc<dyn GithubApiClient>,
    pub clone_url_override: Option<String>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        // ... base_url + clone_url_override unchanged ...
        Self {
            workspaces: HashMap::new(),
            last_activity: std::sync::atomic::AtomicU64::new(/* now */),
            github_api: Arc::new(DefaultGithubApi { base_url }),
            clone_url_override,
        }
    }
}
```

**AgentInfo 字段不变**(保持 provider/model/system_prompt/env/loop_handle 等)。

**HealthResponse 改写** — `initialized: bool` 改为"有至少一个 workspace",返回 `workspaces_count: usize` 和 `workspaces: Vec<WorkspaceSummary>`。

**Helper methods:**

```rust
impl RuntimeState {
    pub fn get(&self, slug: &str) -> Option<&WorkspaceContext> { self.workspaces.get(slug) }
    pub fn get_mut(&mut self, slug: &str) -> Option<&mut WorkspaceContext> { self.workspaces.get_mut(slug) }
    pub fn slugs(&self) -> std::collections::HashSet<String> {
        self.workspaces.keys().cloned().collect()
    }
}
```

**Tests** — 在 `workspace.rs` 或 `http.rs #[cfg(test)]`:

```rust
#[test] fn workspace_context_new_default_fields()
#[test] fn runtime_state_get_returns_none_for_unknown()
#[test] fn runtime_state_slugs_roundtrip()
```

**Verify:** `cargo build -p gitim-runtime` — 预期**大量 compile errors**(所有 handler 都访问旧字段)。这是预期的,Task 5/6 会修。**此 task 只做数据结构层改造 + 给下游留 TODO**。

作为此 task 的范围保护,临时给所有 handler 加一个过渡 shim:

```rust
// 在 http.rs 顶部加过渡 helper,Task 6 会删除
fn legacy_workspace(state: &SharedRuntimeState) -> Option<PathBuf> {
    state.lock().unwrap().workspaces.values().next().map(|w| w.path.clone())
}
fn legacy_human_repo(state: &SharedRuntimeState) -> Option<PathBuf> {
    state.lock().unwrap().workspaces.values().next().and_then(|w| w.human_repo.clone())
}
// 等等 — 把老 handler 改成调 legacy_* 临时跑通编译
```

> **执行者注意**:这个 shim 必须在 Task 6 删除。不能遗漏。

**Verify after shim:** `cargo build -p gitim-runtime` 通过;`cargo test -p gitim-runtime` 旧测试应当全过(shim 让行为等价于"第一个且唯一 workspace")。

**Commit:** `refactor(runtime): introduce WorkspaceContext, restructure RuntimeState`

---

## Task 4: Per-workspace broadcast + `AgentActivityEvent.workspace_id`

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(`AgentActivityEvent` 加字段,SSE 订阅改从 `WorkspaceContext.activity_tx` 拿)
- Modify: `crates/gitim-runtime/src/agent_loop.rs`(emit 时带 slug)

**AgentActivityEvent:**

```rust
#[derive(Clone, Debug, Serialize)]
pub struct AgentActivityEvent {
    pub agent_id: String,
    pub workspace_id: String,  // NEW — always populated
    pub event_type: String,
    pub detail: String,
    pub timestamp: String,
}
```

**AgentLoop** (`crates/gitim-runtime/src/agent_loop.rs`):

```rust
pub struct AgentLoop {
    // ... existing fields ...
    activity_tx: Option<broadcast::Sender<AgentActivityEvent>>,
    workspace_id: String,  // NEW
}

impl AgentLoop {
    pub fn set_activity_tx_with_workspace(&mut self, tx: broadcast::Sender<AgentActivityEvent>, workspace_id: String) {
        self.activity_tx = Some(tx);
        self.workspace_id = workspace_id;
    }
    // 构造 event 处填上 workspace_id
}
```

调用方(`http.rs:1318` `set_activity_tx` 改成 `set_activity_tx_with_workspace`)拿到 slug — 从 WorkspaceContext 拿。

**Tests (agent_loop.rs):**
```rust
#[test] fn agent_activity_event_serializes_with_workspace_id()
// 已有测试若不测 workspace_id,适配使之 assert 字段存在
```

**Tests (http.rs):**
```rust
#[tokio::test]
async fn sse_subscriber_gets_only_scoped_workspace_events() {
    // state 里两个 ws a,b
    // subscribe via /workspaces/a/agents/events
    // ws b emit event → 不应收到;ws a emit → 收到
}
```

> 如果 Task 6 还没完成 router,此测可 stub 掉 HTTP 层直接 subscribe `ctx.activity_tx.subscribe()`。

**Verify:** `cargo test -p gitim-runtime` 旧 + 新测过。

**Commit:** `feat(runtime): per-workspace broadcast channel with workspace_id tagging`

---

## Task 5: Global `/workspaces` routes

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(替换 `/workspace` 和 `/git/init`;新增 `GET /workspaces` / `DELETE /workspaces/:slug`)

**Endpoints & shapes:**

```
GET /workspaces
→ 200 { "workspaces": [ { slug, workspace_name, path, provider, initialized } ] }

POST /workspaces
  body: {
    "path": "/abs/path",
    "workspace_name": "optional — defaults to basename原样",
    "git": { "provider": "local" | "github", "remote_url": ?, "token": ? }
  }
→ 201 { "ok": true, "slug": "frontend", "workspace_name": "frontend", ... }
→ 400 { "ok": false, "error_code": "invalid_token" | "token_lacks_repo_access" | ... }
→ 409 { "ok": false, "error_code": "handler_conflict", ... }

GET /workspaces/:slug
→ 200 { slug, workspace_name, path, agents_count, human_repo, ... }
→ 404 { ok: false, error: "unknown workspace" }

DELETE /workspaces/:slug
→ 200 { ok: true }
→ 404 { ok: false, error: "unknown workspace" }
```

**POST 行为**(关键 — review 里 A1 TOCTOU 修复)**:

1. 入口校验 path(复用 `validate_workspace_path_from_env`)
2. **持锁**:
   - 计算 basename → `slug::normalize` → `slug::resolve(candidate, &state.slugs())`
   - 在 workspaces 里 insert 一个 placeholder `WorkspaceContext`
   - 释放锁(do NOT hold across await)
3. **异步 IO**(不持锁):`provision_human` / github token 校验 / clone
4. **持锁**:
   - 把 placeholder 填完整(human_repo / git_config)
   - `user_config::upsert` 写 `~/.gitim/runtime.json`
5. 失败时(任一步):持锁 remove placeholder + 删 `.gitim-runtime/` + 不写 runtime.json(carry 现有单 ws 失败清理语义)

**DELETE 行为**:
1. 持锁 `remove(slug)` 从 HashMap 拿到 `WorkspaceContext`
2. 释放锁(ctx 里的 daemon handle 在作用域末尾 drop)
3. 调用 daemon shutdown helper(`graceful_shutdown_daemon(&ctx)`,SIGTERM + 5s + SIGKILL — 实现见 Task 8)
4. `user_config::remove(&slug) + write`
5. 返回 200

**File hygiene:** **不删本地文件**(决策摘要)。仅停 daemon + 清 runtime.json。

**Tests** — 放到 Task 10 的 integration test 里(本 task 打桩实现即可)。

**Verify:** `cargo build -p gitim-runtime`;手动 `curl POST /workspaces`(见 Task 10 test)。

**Commit:** `feat(runtime): global /workspaces CRUD routes`

---

## Task 6: Slug extractor + router nest + handler rewiring

**这是第二大改造 task。删除 Task 3 里的 shim。**

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(extractor、router、所有 workspace-scoped handler)

**WorkspaceSlug extractor:**

```rust
pub struct WorkspaceSlug(pub String);

impl<S> axum::extract::FromRequestParts<S> for WorkspaceSlug
where S: Send + Sync
{
    type Rejection = axum::response::Response;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _state: &S)
        -> Result<Self, Self::Rejection>
    {
        use axum::extract::Path;
        let Path(slug): Path<String> = Path::from_request_parts(parts, _state).await
            .map_err(|e| e.into_response())?;
        crate::slug::validate(&slug).map_err(|e| (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": format!("invalid slug: {e}") })),
        ).into_response())?;
        Ok(WorkspaceSlug(slug))
    }
}
```

> 注意:axum 的 `Path` 在嵌套 router 里合并上层 path param 需要 `Path<(Slug,)>` 或自定义提取。用 `axum::extract::Path<String>` 在嵌套中可以直接拿第一个 param。验证点:实际编写时跑集成测试验证。

**with_workspace helper**(review C1 DRY 修复):

```rust
/// Locks state briefly, looks up workspace, clones the fields the caller needs.
/// The closure receives a snapshot of the ctx references (short borrow).
fn with_workspace_snapshot<F, R>(
    state: &SharedRuntimeState,
    slug: &str,
    f: F,
) -> Result<R, axum::response::Response>
where F: FnOnce(&WorkspaceContext) -> R,
{
    let s = state.lock().unwrap();
    let ctx = s.workspaces.get(slug).ok_or_else(|| (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "ok": false, "error": "unknown workspace" })),
    ).into_response())?;
    Ok(f(ctx))
}

// 写操作版本类似,with_workspace_mut
```

**Router 重构:**

```rust
pub fn create_router() -> (Router, SharedRuntimeState) {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    let ws_router = Router::new()
        .route("/im/me", get(im_me))
        .route("/im/channels", get(im_channels))
        .route("/im/create-channel", post(im_create))
        // ... all /im/* moved here ...
        .route("/agents", get(agents_list))
        .route("/agents/events", get(agents_events))
        .route("/agents/add", post(agents_add))
        // ... all /agents/* moved here ...
        ;

    let router = Router::new()
        .route("/health", get(health))
        .route("/workspaces", get(workspaces_list).post(workspaces_create))
        .route("/workspaces/{slug}", get(workspaces_get).delete(workspaces_delete))
        .nest("/workspaces/{slug}", ws_router)
        .route("/preflight/{provider}", get(preflight_handler))
        .layer(axum::middleware::from_fn_with_state(state.clone(), activity_middleware))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    (router, state)
}
```

**Handler 改造 pattern**(示例:`im_send`)— 所有 workspace-scoped handler 都按此改:

```rust
// Before
async fn im_send(State(state): State<SharedRuntimeState>, Json(req): Json<SendRequest>) -> ... {
    let human_repo = {
        let s = state.lock().unwrap();
        s.human_repo.clone().ok_or(...)?  // 旧
    };
    // ...
}

// After
async fn im_send(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
    Json(req): Json<SendRequest>,
) -> ... {
    let human_repo = with_workspace_snapshot(&state, &slug, |ctx| ctx.human_repo.clone())?
        .ok_or(...)?;
    // ...
}
```

**改造覆盖**(所有当前 workspace-scoped handler):
- `/im/*` 共 13 个 handler
- `/agents/*` 共 7 个 handler
- `agents_events` SSE(订阅源改 `ctx.activity_tx.subscribe()`)

**SSE endpoint 改造:**

```rust
async fn agents_events(
    State(state): State<SharedRuntimeState>,
    WorkspaceSlug(slug): WorkspaceSlug,
) -> Result<Sse<...>, axum::response::Response> {
    let rx = with_workspace_snapshot(&state, &slug, |ctx| ctx.activity_tx.subscribe())?;
    // stream unchanged
}
```

**删除 Task 3 shim:**
- `legacy_workspace`, `legacy_human_repo` 等全部删除
- 老 `/workspace` `/git/init` 路由删除(合并入 `POST /workspaces`)

**Tests** — 部分单元测试改签名;核心在 Task 10 integration test。

**Verify:**
```bash
cargo build -p gitim-runtime  # 必须通过
cargo test -p gitim-runtime   # 除 shim 依赖的老测试外应通过
```

**Commit:** `refactor(runtime): nest workspace-scoped routes under /workspaces/:slug`

---

## Task 7: Recover multi-workspace(并行)

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(`recover_from_config` 重写)

**Before** — 单 ws 串行;**After** — 读 `user_config` 列表,`FuturesUnordered` 并行 recover 每个 ws(各 ws 内部 daemon + agents 仍按现有顺序)。

**Implementation:**

```rust
pub async fn recover_from_config(state: SharedRuntimeState) {
    let cfg = crate::user_config::read();
    if cfg.workspaces.is_empty() { return; }

    tracing::info!("recovering {} workspace(s)", cfg.workspaces.len());

    // Parallel per-workspace recovery. Drop entries whose path no longer exists.
    let tasks = cfg.workspaces.into_iter().filter_map(|entry| {
        let workspace = PathBuf::from(&entry.path);
        if !workspace.exists() {
            tracing::warn!(slug=%entry.slug, path=%entry.path, "workspace path missing; skip");
            return None;
        }
        let state = state.clone();
        Some(tokio::spawn(async move {
            recover_single_workspace(state, entry.slug, entry.workspace_name, workspace).await
        }))
    });

    futures::future::join_all(tasks).await;
}

async fn recover_single_workspace(
    state: SharedRuntimeState,
    slug: String,
    workspace_name: String,
    workspace: PathBuf,
) {
    // 1. Insert fresh WorkspaceContext
    {
        let mut s = state.lock().unwrap();
        let mut ctx = WorkspaceContext::new(slug.clone(), workspace_name, workspace.clone());
        ctx.git_config = WorkspaceConfig::read(&workspace).ok();
        s.workspaces.insert(slug.clone(), ctx);
    }

    // 2. Provision human daemon (existing branch-on-github-vs-git logic,
    //    unchanged except operates on &workspace and writes to ctx.human_repo)
    let human_dir = workspace.join(".gitim-runtime/human");
    if human_dir.exists() {
        // ... current logic, setting result on ctx.human_repo ...
    }

    // 3. Recover agents for this workspace
    recover_agents_for_workspace(state, &slug, &workspace).await;
}
```

**`recover_agents_for_workspace`** — rename 现 `recover_agents_from_workspace`,接受 `&slug`,所有 `state.agents.insert` 改为 `state.workspaces.get_mut(&slug).agents.insert`;`activity_tx.send` 改为 `ctx.activity_tx.send`(带 `workspace_id: slug`)。

**Tests** — Task 11 integration test。

**Verify:** `cargo build -p gitim-runtime` + `cargo test -p gitim-runtime`。

**Commit:** `feat(runtime): parallel multi-workspace recovery from user config`

---

## Task 8: Daemon lifecycle + token propagation 多 ws

**Files:**
- Modify: `crates/gitim-runtime/src/workspace.rs`(加 `daemon_handle` 或 shutdown 接口)
- Modify: `crates/gitim-runtime/src/token_propagation.rs`(签名变或调用方迭代)
- Modify: `crates/gitim-runtime/src/bin/*.rs` 或 `http.rs`(signal handler)

**Daemon handle**:

查现有 daemon 生命周期管理代码(`gitim_client::ensure_daemon` / `kill_daemon`)。每个 WorkspaceContext 不一定需要持有 `Child` — daemon 由 workspace path 驱动,可通过 `{path}/.gitim/daemon.pid` 定位;关闭时 helper 根据 path 发信号。

**graceful_shutdown helper:**

```rust
// workspace.rs
pub async fn graceful_shutdown(workspace_path: &Path) {
    let pid_file = workspace_path.join(".gitim/daemon.pid");
    let Ok(pid_str) = std::fs::read_to_string(&pid_file) else { return; };
    let Ok(pid) = pid_str.trim().parse::<i32>() else { return; };

    // SIGTERM
    unsafe { libc::kill(pid, libc::SIGTERM); }
    // Wait up to 5s
    for _ in 0..50 {
        if !process_alive(pid) { return; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // SIGKILL
    unsafe { libc::kill(pid, libc::SIGKILL); }
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}
```

> 检查仓库里是否已有等效 helper(`gitim-client` 或 `gitim-daemon`);如有直接复用而非新建。

**Runtime shutdown** — 在 runtime binary 的 signal handler / Drop 场所迭代所有 ws:

```rust
// runtime bin main (或 http::shutdown_all)
pub async fn shutdown_all(state: SharedRuntimeState) {
    let paths: Vec<_> = { state.lock().unwrap().workspaces.values().map(|w| w.path.clone()).collect() };
    for p in paths { workspace::graceful_shutdown(&p).await; }
}
```

**DELETE handler** 调同一 helper。

**Token propagation** — 签名保持,但调用点从"一次 workspace"改为"迭代所有 workspace":

```rust
// http.rs,现 propagate 单 ws 的调用点全部改为:
let workspaces_paths: Vec<_> = state.lock().unwrap().workspaces.values().map(|w| w.path.clone()).collect();
for p in workspaces_paths {
    let _ = crate::token_propagation::propagate_token(&p);
}
```

或加一个 helper `propagate_all(state)`。

**Tests:**
```rust
#[tokio::test] fn graceful_shutdown_sends_sigterm_then_sigkill()
// 在 ci 下跳过(需要 real process);用 #[ignore] + 文档化
```

**Verify:** `cargo build -p gitim-runtime`。

**Commit:** `feat(runtime): workspace-scoped daemon shutdown and token propagation`

---

## Task 9: webui-v2 multi-workspace client + UI

**Files:**
- Modify: `webui-v2/src/lib/client.ts`
- Modify: `webui-v2/src/hooks/use-connection-store.ts`
- Create: `webui-v2/src/hooks/use-workspace-store.ts`
- Modify: `webui-v2/src/hooks/use-agent-activity.ts`
- Create: `webui-v2/src/components/workspace-switcher.tsx`
- Modify: `webui-v2/src/app.tsx`
- Modify: `webui-v2/src/hooks/use-chat-store.ts` 和相关 hooks(任何现在调 client 的地方)

**client.ts** — 每个方法加 `slug` 参数:

```ts
function wsBase(slug: string): string { return `${baseUrl()}/workspaces/${encodeURIComponent(slug)}`; }

export async function sendMessage(slug: string, payload: SendPayload): Promise<SendResp> {
  const r = await fetch(`${wsBase(slug)}/im/send`, { method: "POST", headers: jsonHeaders(), body: JSON.stringify(payload) });
  return r.json();
}
// 同理 listChannels, joinChannel, createChannel, readMessages, pollMessages, listUsers, threadMessages,
//        listCards, createCard, readCard, sendCardMessage, updateCard,
//        agentsList, agentsAdd, agentsStart, agentsStop, agentsRemove, agentsGet

// Global(不带 slug):
export async function listWorkspaces(): Promise<WorkspaceSummary[]>
export async function createWorkspace(req: CreateWorkspaceRequest): Promise<CreateWorkspaceResponse>
export async function deleteWorkspace(slug: string): Promise<void>
export async function preflight(provider: string): Promise<PreflightResult>
export async function health(): Promise<HealthResponse>
```

**use-workspace-store.ts**:

```ts
interface WorkspaceStore {
  workspaces: WorkspaceSummary[];
  activeSlug: string | null;
  loading: boolean;
  fetchAll: () => Promise<void>;
  setActive: (slug: string) => void;
  create: (req: CreateWorkspaceRequest) => Promise<WorkspaceSummary>;
  remove: (slug: string) => Promise<void>;
}
// localStorage key: "gitim-active-workspace" (只存 slug,不存 workspace path)
```

**use-connection-store.ts** — 保留 port,移除 workspace path 相关字段(path 现在是 workspace 属性,不是连接属性)。

**use-agent-activity.ts** — URL 改 `${baseUrl}/workspaces/${activeSlug}/agents/events`,`activeSlug` 变化时重新连接。

**workspace-switcher.tsx** — UI 组件:
- 显示当前 active workspace name
- 点击展开 dropdown 列所有 workspace
- "+ New workspace" 打开创建对话框(复用现 onboard flow 的字段)
- 每项 item 有删除按钮(确认弹窗 → `deleteWorkspace` → refresh)

**app.tsx** — Workspace 切换时:重置 chat store、agent store、重新 fetch `/workspaces/{slug}/im/channels` 等。

**Tests(本期不引入 vitest 基建,保留 TODO):**
- 前端测试留给 TODOS.md 里已有的 "webui-v2 前端测试基建"
- 至少**手动验证**:启动 runtime,创建两个 ws,切换,确认数据隔离

**Verify:**
```bash
cd webui-v2 && npm run build  # 无 TypeScript 错误
# 开 runtime 手测:create/switch/delete workspace,agent 活动不串 ws
```

**Commit:** `feat(webui): multi-workspace client, store, switcher UI`

---

## Task 10: Runtime HTTP integration test 骨架

**Files:**
- Create: `crates/gitim-runtime/tests/http_workspaces.rs`
- 可能需要加:`crates/gitim-runtime/Cargo.toml` dev-deps(reqwest + tokio-test,若未有)

**Purpose** — 补 TODOS.md 登记的"gitim-runtime HTTP 层 integration test"债务(同时本次改造的验收手段)。

**Setup helper:**

```rust
// tests/http_workspaces.rs
use gitim_runtime::http::create_router;
use tokio::net::TcpListener;

struct TestApp {
    base: String,
    _shutdown: tokio::sync::oneshot::Sender<()>,
    _tempdir: tempfile::TempDir,
}

async fn spawn_test_app() -> TestApp {
    let tempdir = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", tempdir.path());  // isolate ~/.gitim
    // ... set GITIM_TEST_* env vars for github mock if needed ...

    let (router, _state) = create_router();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async { let _ = rx.await; })
            .await.unwrap();
    });

    TestApp { base: format!("http://{addr}"), _shutdown: tx, _tempdir: tempdir }
}
```

**Test cases:**

```rust
#[tokio::test]
async fn health_without_workspaces() {
    let app = spawn_test_app().await;
    let r: serde_json::Value = reqwest::get(&format!("{}/health", app.base)).await.unwrap().json().await.unwrap();
    assert_eq!(r["service"], "gitim-runtime");
    assert_eq!(r["workspaces_count"], 0);
}

#[tokio::test]
async fn list_workspaces_empty() { /* GET /workspaces → {workspaces: []} */ }

#[tokio::test]
async fn create_workspace_local_mode() {
    let app = spawn_test_app().await;
    let ws_path = app._tempdir.path().join("project-frontend");
    std::fs::create_dir(&ws_path).unwrap();
    let resp = reqwest::Client::new().post(&format!("{}/workspaces", app.base))
        .json(&serde_json::json!({
            "path": ws_path.to_string_lossy(),
            "git": { "provider": "local" }
        })).send().await.unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["slug"], "project-frontend");
}

#[tokio::test]
async fn create_workspace_slug_conflict_appends_suffix() {
    // create dir "frontend" twice under different parent paths → slug=frontend, frontend-2
}

#[tokio::test]
async fn create_workspace_normalizes_basename_unicode() {
    // dir "前端 project" → slug ≈ "project"(unicode 全 `-`,trim → "project")
}

#[tokio::test]
async fn delete_workspace_removes_entry() { /* POST then DELETE then GET returns 404 */ }

#[tokio::test]
async fn delete_workspace_does_not_remove_local_files() {
    // assert workspace path still has content after DELETE
}

#[tokio::test]
async fn workspace_scoped_route_404_on_unknown_slug() {
    // GET /workspaces/nonexistent/im/channels → 404
}

#[tokio::test]
async fn slug_extractor_rejects_uppercase() {
    // GET /workspaces/Foo/im/channels → 400
}
```

**Verify:**
```bash
cargo test -p gitim-runtime --test http_workspaces
```

**Commit:** `test(runtime): HTTP integration tests for /workspaces routes`

---

## Task 11: Multi-workspace integration test(recover / isolation)

**Files:**
- Create: `crates/gitim-runtime/tests/multi_workspace.rs`

**Tests:**

```rust
#[tokio::test]
async fn recover_from_config_restores_multiple_workspaces() {
    // Prepare tempdir HOME with ~/.gitim/runtime.json containing 3 ws entries
    // Each path is a real (tempdir-created) dir with .gitim-runtime/human tree
    // Call recover_from_config
    // Assert all 3 appear in state.workspaces
}

#[tokio::test]
async fn recover_skips_missing_workspace_path() {
    // runtime.json has entry whose path doesn't exist → skipped, others still recovered
}

#[tokio::test]
async fn sse_isolates_per_workspace_events() {
    // Create workspace A and B
    // Subscribe to /workspaces/A/agents/events
    // Emit event on B's activity_tx
    // Assert A-subscriber does NOT receive it
    // Emit on A's tx → received
}

#[tokio::test]
async fn agent_activity_event_carries_workspace_id() {
    // Create workspace "foo", emit an event via agent_loop
    // Subscribe and assert event.workspace_id == "foo"
}
```

**Verify:**
```bash
cargo test -p gitim-runtime --test multi_workspace
cargo test -p gitim-runtime  # 全套过
```

**Commit:** `test(runtime): multi-workspace recovery + isolation integration tests`

---

## Execution Lane Map

```
Lane A (独立,无依赖):
  Task 1: Slug module
  Task 2: UserConfig v2

Lane B (依赖 A):
  Task 3: WorkspaceContext + RuntimeState 重构(+ shim)

Lane C (依赖 B,可并行):
  Task 4: Per-ws broadcast + workspace_id
  Task 5: Global /workspaces routes
  Task 8a: token propagation 迭代多 ws(纯函数调用点改造)

Lane D (依赖 C):
  Task 6: Slug extractor + router nest + handler rewiring + 删 shim
  Task 7: recover multi-workspace
  Task 8b: daemon lifecycle helpers

Lane E (依赖 D 的合约稳定):
  Task 9: webui-v2 multi-workspace client + UI
  Task 10: HTTP integration test

Lane F (依赖 7 + 10 骨架):
  Task 11: Multi-workspace integration test
```

**Critical path:** Task 1 → 2 → 3 → 6 → 9(前端可见)。实际实施中用 subagent 分工,跨 lane 并行,但 Task 3 和 6 必须独享(顶层 state + router 互斥修改)。

---

## Success Criteria

- [ ] `cargo test` 全绿(~280 个现有 + 新增 ~30)
- [ ] 手动 E2E:
  1. `gitim-runtime-bin` 启动
  2. webui 创建 workspace A(local),登录成功,收发消息正常
  3. webui 创建 workspace B(local),自动用 `<basename>-2` 解决冲突(若撞名)
  4. 切换 A ↔ B,chat 数据不串
  5. 各自 add agent,agent 活动只推送到对应 ws SSE
  6. `DELETE workspace B` → runtime.json B 条目清除、daemon 停、本地文件保留
  7. 重启 runtime → recover_from_config 恢复 A(B 已删)
- [ ] `~/.gitim/runtime.json` 结构正确(`{ workspaces: [...] }`)
- [ ] 旧单 ws endpoint(`/workspace`, `/git/init`)已移除,不再响应

---

## 执行注记(给自己)

- 执行顺序按 critical path:Task 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11
- 每个 task 完成立刻 commit(memory feedback_commit_plan_docs)
- Task 3 shim 在 Task 6 必删,不能遗漏
- Windows 不支持的分支继续 reject(`validate_workspace_path_from_env`)— 不改
- 前端 vitest 基建不在本期(已登记 TODO)
- Plan review 不求完美,发现架构 issue 的话就地 doc 更新此文件
