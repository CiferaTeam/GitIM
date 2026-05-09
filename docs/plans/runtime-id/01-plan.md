# Runtime ID v1 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 Runtime 一个稳定的、device-bound 的 UUID,落地到 `~/.gitim/runtime.json`,通过 `/health` 暴露。

**Architecture:** `UserConfig` 加 `runtime_id` 字段 + 自愈式 `ensure_runtime_id` 入口。`RuntimeState` 加 once-write 字段,启动时由 `bin/runtime.rs::run_shell()` 调 `ensure_runtime_id` 注入。`HealthResponse` 加同名字段透传。

**Tech Stack:** Rust, serde, uuid v4, tracing, axum, tempfile

**Spec reference:** [docs/plans/runtime-id/00-design.md](./00-design.md)

---

### Task 1: UserConfig 加 `runtime_id` 字段(schema 改动 + 兼容性测试)

**Files:**
- Modify: `crates/gitim-runtime/src/user_config.rs:11-15` (UserConfig struct)
- Modify: `crates/gitim-runtime/src/user_config.rs:67-156` (tests module)

**为什么先做 schema:** `ensure_runtime_id` 实现会依赖这个字段。先把 schema 落地 + 兼容性测试落地,后续 task 不需要再回来改 struct。

- [ ] **Step 1: 先写一个旧 schema 兼容性测试,放进 `user_config.rs` 的 tests 模块底部**

加到 `crates/gitim-runtime/src/user_config.rs:155` 之前(最后一个 `#[test]` 之后,`}` 之前):

```rust
    #[test]
    fn legacy_config_without_runtime_id_loads() {
        // 旧 schema 没有 runtime_id 字段;serde(default) 应让它解析为空字符串。
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let legacy = r#"{"workspaces":[{"slug":"a","workspace_name":"A","path":"/x"}]}"#;
        std::fs::write(&path, legacy).unwrap();
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.runtime_id, "");
        assert_eq!(cfg.workspaces.len(), 1);
        assert_eq!(cfg.workspaces[0].slug, "a");
    }
```

- [ ] **Step 2: 跑测试,确认编译错误**

Run: `cargo test -p gitim-runtime --lib user_config::tests::legacy_config_without_runtime_id_loads`
Expected: 编译失败 — `no field 'runtime_id' on type 'UserConfig'`

- [ ] **Step 3: 加 `runtime_id` 字段到 UserConfig**

把 `crates/gitim-runtime/src/user_config.rs:11-15` 的:

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}
```

改成:

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Stable device-bound UUID for this runtime install. Empty when
    /// uninitialized — `ensure_runtime_id` materializes it on first call.
    /// See docs/plans/runtime-id/00-design.md.
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}
```

- [ ] **Step 4: 跑测试,确认通过**

Run: `cargo test -p gitim-runtime --lib user_config::tests::legacy_config_without_runtime_id_loads`
Expected: PASS

- [ ] **Step 5: 跑整个 user_config 模块,确认其他测试没坏**

Run: `cargo test -p gitim-runtime --lib user_config::`
Expected: 全部通过(7 个测试)

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/user_config.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add runtime_id field to UserConfig

Schema groundwork for the device-bound Runtime ID. Field defaults to
empty string via serde(default); legacy ~/.gitim/runtime.json without
the field continues to load cleanly.

The actual generate-or-read logic lands in the next task as
ensure_runtime_id().

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: 实现 `ensure_runtime_id_at` + 单元测试

**Files:**
- Modify: `crates/gitim-runtime/src/user_config.rs` (加 `ensure_runtime_id_at` 函数 + 测试)

**Why split _at vs the home_dir wrapper:** 测试需要可控路径,production 用 `dirs::home_dir()`。两个函数:`ensure_runtime_id_at(path)` 是核心逻辑,`ensure_runtime_id()` 是 home_dir wrapper。本 task 实现 `_at`,下个 task 加 wrapper。

- [ ] **Step 1: 在 tests 模块底部写测试**

加到 `crates/gitim-runtime/src/user_config.rs` 的 tests 模块里(`legacy_config_without_runtime_id_loads` 之后,`}` 之前):

```rust
    #[test]
    fn ensure_runtime_id_creates_when_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        assert!(!path.exists());
        let id = ensure_runtime_id_at(&path);
        // 是合法的 UUIDv4 dashed 格式
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        // 文件已经写入,内容里含同一个 ID
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.runtime_id, id);
    }

    #[test]
    fn ensure_runtime_id_returns_same_on_second_call() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let first = ensure_runtime_id_at(&path);
        let second = ensure_runtime_id_at(&path);
        assert_eq!(first, second);
    }

    #[test]
    fn ensure_runtime_id_regenerates_on_corruption() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        std::fs::write(&path, r#"{"runtime_id":"not-a-uuid","workspaces":[]}"#).unwrap();
        let id = ensure_runtime_id_at(&path);
        assert_ne!(id, "not-a-uuid");
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        let cfg = read_from(Some(&path));
        assert_eq!(cfg.runtime_id, id);
    }

    #[test]
    fn ensure_runtime_id_regenerates_on_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        std::fs::write(&path, r#"{"runtime_id":"","workspaces":[]}"#).unwrap();
        let id = ensure_runtime_id_at(&path);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(read_from(Some(&path)).runtime_id, id);
    }

    #[test]
    fn ensure_runtime_id_preserves_workspaces() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("runtime.json");
        let mut cfg = UserConfig::default();
        cfg.upsert(sample("frontend", "Frontend", "/ws/frontend"));
        cfg.upsert(sample("backend", "Backend", "/ws/backend"));
        write_to(&cfg, &path).unwrap();
        // ensure 接管:cfg 里 runtime_id 是 ""(default),应该被生成 + 写回
        let id = ensure_runtime_id_at(&path);
        assert!(!id.is_empty());
        let after = read_from(Some(&path));
        assert_eq!(after.runtime_id, id);
        assert_eq!(after.workspaces.len(), 2);
        assert_eq!(after.workspaces[0].slug, "frontend");
        assert_eq!(after.workspaces[1].slug, "backend");
    }
```

- [ ] **Step 2: 跑测试,确认编译错误(`ensure_runtime_id_at` 还不存在)**

Run: `cargo test -p gitim-runtime --lib user_config::tests::ensure_runtime_id_creates_when_missing`
Expected: 编译失败 — `cannot find function 'ensure_runtime_id_at'`

- [ ] **Step 3: 实现 `ensure_runtime_id_at`**

在 `crates/gitim-runtime/src/user_config.rs:48`(`write_to` 函数之后,`impl UserConfig` 之前)加:

```rust
/// Read or generate the device-bound runtime ID.
///
/// Behavior:
/// - If `path` exists and `runtime_id` parses as a UUID → return it as-is.
/// - Otherwise (missing file, missing field, empty string, malformed UUID)
///   → generate a new v4 UUID, write it back into the same file (preserving
///   `workspaces`), and return the new ID.
/// - Write failures are logged via `tracing::warn!` but do NOT propagate;
///   the in-memory UUID is still returned so runtime startup can proceed.
///   Next startup will retry the write.
///
/// See docs/plans/runtime-id/00-design.md for the full design and
/// non-goals (no platform-native device ID, no git sync, no agent injection).
pub fn ensure_runtime_id_at(path: &Path) -> String {
    let mut cfg = read_from(Some(path));
    if uuid::Uuid::parse_str(&cfg.runtime_id).is_ok() {
        return cfg.runtime_id;
    }
    let new_id = uuid::Uuid::new_v4().to_string();
    cfg.runtime_id = new_id.clone();
    if let Err(e) = write_to(&cfg, path) {
        tracing::warn!(
            error = %e,
            path = %path.display(),
            "failed to persist runtime_id; will retry on next startup"
        );
    }
    new_id
}
```

- [ ] **Step 4: 跑全部新测试**

Run: `cargo test -p gitim-runtime --lib user_config::tests::ensure_runtime_id`
Expected: 5 个测试都 PASS

- [ ] **Step 5: 跑整个 user_config 模块,确认其他测试没坏**

Run: `cargo test -p gitim-runtime --lib user_config::`
Expected: 全部通过(12 个测试 = 7 旧 + 5 新)

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/user_config.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add ensure_runtime_id_at for device-bound UUID

Self-healing accessor: read existing UUID from runtime.json, or generate
a fresh v4 UUID and write it back when missing/empty/malformed. Write
failures are logged (tracing::warn!) but never block startup — the
in-memory UUID is still returned and next startup retries.

The path-injectable variant is the testable seam; the home_dir-bound
production wrapper lands next.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Production wrapper `ensure_runtime_id` (bind to `~/.gitim/`)

**Files:**
- Modify: `crates/gitim-runtime/src/user_config.rs` (加 `ensure_runtime_id` 函数)

- [ ] **Step 1: 实现 wrapper**

在 `ensure_runtime_id_at` 函数之后(还在 `impl UserConfig` 之前)加:

```rust
/// Production entry point: resolves `~/.gitim/runtime.json` and delegates to
/// `ensure_runtime_id_at`. If `dirs::home_dir()` returns `None` (rare —
/// containers, no-HOME environments), generates a fresh in-memory UUID
/// without persisting it, matching the existing `write()` noop semantics.
/// In that case the runtime keeps a stable ID for its current process
/// lifetime but rolls a new one on each restart — acceptable for the
/// tail-edge case.
pub fn ensure_runtime_id() -> String {
    match config_path() {
        Some(p) => ensure_runtime_id_at(&p),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            tracing::warn!(
                runtime_id = %id,
                "dirs::home_dir() returned None; runtime_id not persisted, will reroll on restart"
            );
            id
        }
    }
}
```

- [ ] **Step 2: 加一个 ignored sanity test**

加到 tests 模块底部:

```rust
    #[test]
    #[ignore = "writes to real ~/.gitim/runtime.json; run manually with --ignored"]
    fn ensure_runtime_id_returns_valid_uuid() {
        // Manual smoke test for the home_dir-bound production wrapper.
        // Marked #[ignore] because it touches the real $HOME — running it in
        // CI or in a developer's normal `cargo test` would write/mutate
        // ~/.gitim/runtime.json. The integration tests in
        // tests/runtime_id.rs cover the wiring without this side effect.
        let id = ensure_runtime_id();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }
```

设计判断:Task 3 的 wrapper 是 `home_dir() + delegate to ensure_runtime_id_at`,没有独立分支(除了 `None` 分支,后者用 env-var-mock 测会让其他测试串味)。完整覆盖留给 Task 7 集成测试。

- [ ] **Step 3: 跑测试(默认跳过 ignored)**

Run: `cargo test -p gitim-runtime --lib user_config::`
Expected: 全部通过(12 个测试,sanity test 显示为 `1 ignored`)

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/user_config.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add ensure_runtime_id home_dir wrapper

Production entry point that resolves ~/.gitim/runtime.json and delegates
to ensure_runtime_id_at. Handles dirs::home_dir() == None by returning
an unpersisted in-memory UUID — matches existing user_config::write
noop semantics for that edge case.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `RuntimeState` 加 `runtime_id` 字段

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:247-282` (RuntimeState struct)
- Modify: `crates/gitim-runtime/src/http.rs:294-329` (Default impl)

- [ ] **Step 1: 加字段到 struct**

在 `crates/gitim-runtime/src/http.rs:247-282` 的 `RuntimeState` struct 内加新字段(放在 `listen_port` 之后,struct 末尾):

```rust
    pub listen_port: u16,
    /// Stable device-bound UUID for this runtime install. Once-write at
    /// startup by `bin/runtime.rs::run_shell()` from
    /// `user_config::ensure_runtime_id`; read-only thereafter. Empty
    /// string when constructed via `Default::default()` for tests that
    /// don't go through the boot path. See docs/plans/runtime-id/00-design.md.
    pub runtime_id: String,
}
```

- [ ] **Step 2: 在 Default impl 里初始化**

在 `crates/gitim-runtime/src/http.rs:313-327` 的 `Self { ... }` 里加(放在 `listen_port: DEFAULT_PORT,` 之后):

```rust
            listen_port: DEFAULT_PORT,
            runtime_id: String::new(),
        }
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p gitim-runtime`
Expected: 成功,无 error。可能有 `dead_code` warning 关于 `runtime_id` 字段(因为还没读它),Task 5 会消除。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "$(cat <<'EOF'
feat(runtime): add runtime_id field to RuntimeState

Once-write field populated at startup. Default impl sets empty string
so existing test constructors (RuntimeState::default, create_router)
continue to work without changes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `HealthResponse` 加 `runtime_id` + `health` handler 透传 + 测试

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:80-85` (HealthResponse)
- Modify: `crates/gitim-runtime/src/http.rs:354-361` (health handler)
- Modify: `crates/gitim-runtime/src/http.rs` tests module(找一个现有的 health-related 测试位置或新写)

**先确认现有 tests 模块里有没有 health 相关测试,有则扩展,没则加新测试。**

- [ ] **Step 1: 写测试,先确认会失败**

在 `crates/gitim-runtime/src/http.rs` 的 tests 模块里(grep `mod tests` 找到位置 — 常规位置在文件底部),加:

```rust
    #[tokio::test]
    async fn health_response_includes_runtime_id() {
        use axum::body::to_bytes;
        use tower::ServiceExt;

        let (router, state) = create_router();
        // 模拟启动期注入
        state.lock().unwrap().runtime_id = "test-runtime-id-1234".to_string();

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body = to_bytes(response.into_body(), 1024 * 16).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json.get("runtime_id").and_then(|v| v.as_str()),
            Some("test-runtime-id-1234")
        );
        // 现存字段不能被破坏
        assert_eq!(
            json.get("service").and_then(|v| v.as_str()),
            Some("gitim-runtime")
        );
    }
```

**注意:** 如果 `tests` 模块里已经有 `use` 语句覆盖 `create_router`、`tower::ServiceExt` 等,不必重复 import。先用 `cargo test` 跑,看编译错误指出什么 missing,再补 use。

- [ ] **Step 2: 跑测试,确认失败**

Run: `cargo test -p gitim-runtime --lib http::tests::health_response_includes_runtime_id`
Expected: PASS or FAIL? — 大概率 FAIL,因为 HealthResponse 目前没有 `runtime_id` 字段,JSON 里查不到。

- [ ] **Step 3: 改 HealthResponse 加字段**

把 `crates/gitim-runtime/src/http.rs:80-85` 的:

```rust
#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
    workspaces_count: usize,
}
```

改成:

```rust
#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
    workspaces_count: usize,
    runtime_id: String,
}
```

- [ ] **Step 4: 改 health handler 透传 runtime_id**

把 `crates/gitim-runtime/src/http.rs:354-361` 的:

```rust
async fn health(State(state): State<SharedRuntimeState>) -> Json<HealthResponse> {
    let s = state.lock().unwrap();
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
        workspaces_count: s.workspaces.len(),
    })
}
```

改成:

```rust
async fn health(State(state): State<SharedRuntimeState>) -> Json<HealthResponse> {
    let s = state.lock().unwrap();
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
        workspaces_count: s.workspaces.len(),
        runtime_id: s.runtime_id.clone(),
    })
}
```

- [ ] **Step 5: 跑测试,确认通过**

Run: `cargo test -p gitim-runtime --lib http::tests::health_response_includes_runtime_id`
Expected: PASS

- [ ] **Step 6: 跑整个 http 模块测试,确认没把别的测试搞坏**

Run: `cargo test -p gitim-runtime --lib http::tests::`
Expected: 全部通过

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "$(cat <<'EOF'
feat(runtime): expose runtime_id via /health

HealthResponse gains a runtime_id field; health handler clones it from
state. Frontend already polls /health for version + workspace count, so
runtime_id is visible to the WebUI without any client-side changes.

This is non-breaking: clients ignoring the new field continue to work.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: `bin/runtime.rs::run_shell()` 启动时调用 `ensure_runtime_id`

**Files:**
- Modify: `crates/gitim-runtime/src/bin/runtime.rs:142-148` (run_shell 启动序列)

- [ ] **Step 1: 在 `run_shell()` 里 listen_port 写入之后、`recover_from_config` 之前加注入**

把 `crates/gitim-runtime/src/bin/runtime.rs:142-148` 的:

```rust
    let (router, state) = gitim_runtime::http::create_router_with_exe(canonical_exe);
    // Record the port we're about to bind so the self-update async phase can
    // pass the same `--port` to the replacement runtime. `run_shell` is the
    // single writer; nothing else in the crate needs to mutate this.
    state.lock().unwrap().listen_port = port;
```

改成:

```rust
    let (router, state) = gitim_runtime::http::create_router_with_exe(canonical_exe);
    // Record the port we're about to bind so the self-update async phase can
    // pass the same `--port` to the replacement runtime. `run_shell` is the
    // single writer; nothing else in the crate needs to mutate this.
    state.lock().unwrap().listen_port = port;

    // Materialize the device-bound runtime ID. First boot generates and
    // persists; subsequent boots read the existing UUID. Either way it lands
    // in RuntimeState before recover_from_config so /health responds with the
    // real ID even during the recovery window.
    // See docs/plans/runtime-id/00-design.md.
    let runtime_id = gitim_runtime::user_config::ensure_runtime_id();
    state.lock().unwrap().runtime_id = runtime_id.clone();
    eprintln!("runtime started, id: {runtime_id}");
```

- [ ] **Step 2: 编译验证**

Run: `cargo build -p gitim-runtime --bin gitim-runtime`
Expected: 成功。

- [ ] **Step 3: 手动 smoke test**

```bash
# 选一个空的临时 HOME 目录
export TEST_HOME=$(mktemp -d)
HOME=$TEST_HOME ./target/debug/gitim-runtime --port 12347 &
RUNTIME_PID=$!
sleep 1
curl -s http://127.0.0.1:12347/health | python3 -m json.tool
kill $RUNTIME_PID
wait $RUNTIME_PID 2>/dev/null
ls $TEST_HOME/.gitim/
cat $TEST_HOME/.gitim/runtime.json
rm -rf $TEST_HOME
```

Expected:
- `/health` 输出含 `"runtime_id": "<uuid>"` 字段
- `~/.gitim/runtime.json` 存在,内容形如 `{"runtime_id":"...","workspaces":[]}`
- runtime startup 日志含 `runtime started, id: <uuid>`

**如果手动测试通不过**:回到 Step 1 检查改动是否正确。这是 Task 6 唯一的"事实"信号 — 后面的集成测试会自动化这一步,但人工跑一次锁住理解。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/bin/runtime.rs
git commit -m "$(cat <<'EOF'
feat(runtime): wire ensure_runtime_id into run_shell startup

Runtime now materializes its device-bound UUID on boot — generates +
persists on first run, reads on subsequent runs — and exposes it via
/health. Startup log prints the ID so operators can spot it from logs.

Lands before recover_from_config so the ID is observable during the
recovery window. With this in place, future distributed coordination
work (multi-runtime event channel, same-machine agent detection) has a
stable anchor to build on.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: 集成测试 `tests/runtime_id.rs`

**Files:**
- Create: `crates/gitim-runtime/tests/runtime_id.rs`

**为什么单独建集成测试:** 验证 `ensure_runtime_id` ↔ `RuntimeState` ↔ `HealthResponse` 三者串联起来。每环都有 unit test,但串起来的 wiring 错误 unit test 抓不到。集成测试不启 socket、不开子进程,直接构造 router + 调 handler — 跟 `tests/http_workspaces.rs` 同款。

- [ ] **Step 1: 看一下 `tests/http_workspaces.rs` 的测试风格,follow 同一种构造方式**

Run: `head -60 crates/gitim-runtime/tests/http_workspaces.rs`
Expected: 看到一个用 `create_router()` + `tower::ServiceExt::oneshot` 调 handler 的测试。

(这一步是阅读,不写代码。如果发现风格跟下面的代码不一致,以本仓库 tests/ 目录现存风格为准 — 修改下面的代码模板。)

- [ ] **Step 2: 创建 `crates/gitim-runtime/tests/runtime_id.rs`**

写入以下完整内容(集成测试不能用 `cargo test --lib`,只能通过 `cargo test --test runtime_id` 跑):

```rust
//! Integration tests for runtime_id end-to-end:
//! ensure_runtime_id → RuntimeState → /health response.
//!
//! Unlike the unit tests in user_config.rs (which exercise only the
//! file-IO layer) and http.rs (which exercises only the handler with a
//! pre-injected ID), these tests cover the wiring that bin/runtime.rs
//! does at startup.

use axum::body::to_bytes;
use gitim_runtime::http::create_router;
use gitim_runtime::user_config;
use tempfile::TempDir;
use tower::ServiceExt;

/// Issue a GET /health and return the parsed JSON body.
async fn fetch_health(router: axum::Router) -> serde_json::Value {
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .uri("/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 1024 * 16).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn health_returns_runtime_id_after_ensure() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime.json");

    // 模拟 run_shell 启动序列:ensure → 注入 state
    let id = user_config::ensure_runtime_id_at(&path);
    let (router, state) = create_router();
    state.lock().unwrap().runtime_id = id.clone();

    let json = fetch_health(router).await;
    assert_eq!(
        json.get("runtime_id").and_then(|v| v.as_str()),
        Some(id.as_str())
    );
}

#[tokio::test]
async fn restart_preserves_runtime_id() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("runtime.json");

    // 第一次"启动"
    let first_id = user_config::ensure_runtime_id_at(&path);
    let (router1, state1) = create_router();
    state1.lock().unwrap().runtime_id = first_id.clone();
    let json1 = fetch_health(router1).await;
    let observed1 = json1.get("runtime_id").and_then(|v| v.as_str()).unwrap().to_string();

    // 模拟重启:state 全部丢失,只有磁盘上的 runtime.json 留下
    drop(state1);

    let second_id = user_config::ensure_runtime_id_at(&path);
    let (router2, state2) = create_router();
    state2.lock().unwrap().runtime_id = second_id.clone();
    let json2 = fetch_health(router2).await;
    let observed2 = json2.get("runtime_id").and_then(|v| v.as_str()).unwrap().to_string();

    assert_eq!(first_id, second_id, "ensure_runtime_id_at should be stable");
    assert_eq!(observed1, observed2, "/health should return the same ID across restarts");
}
```

- [ ] **Step 3: 跑集成测试**

Run: `cargo test -p gitim-runtime --test runtime_id`
Expected: 2 个测试 PASS

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/tests/runtime_id.rs
git commit -m "$(cat <<'EOF'
test(runtime): integration coverage for runtime_id end-to-end

Verify the ensure_runtime_id → RuntimeState → /health wiring works
across simulated restart cycles. Unit tests cover each layer
individually; this is the test that catches integration regressions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: 全量 regression 验证

**Files:** 无改动

- [ ] **Step 1: 跑 gitim-runtime 全部测试**

Run: `cargo test -p gitim-runtime`
Expected: 全绿。预期新增的测试 = 5 (Task 2) + 1 (Task 3 sanity) + 1 (Task 5 health) + 2 (Task 7 integration) = 9 个。

- [ ] **Step 2: 跑 workspace 全量(确认没把别的 crate 撞坏)**

Run: `cargo test`
Expected: 全绿。

注意:全量测试很慢(700+ tests,数分钟),但**这是交付前必须跑的**。

如有失败,逐个排查。预期 0 个失败 — 本 plan 改动面只有新增字段(serde default 兼容)和新函数,不动现有接口。

- [ ] **Step 3: 跑 clippy(代码风格 + warnings)**

Run: `cargo clippy -p gitim-runtime --all-targets -- -D warnings`
Expected: 通过,无 warning。

如有 dead_code / unused 警告,意味着某个字段未被使用 — 逐个修。

---

### Task 9: 准备验收

**Files:** 无改动

- [ ] **Step 1: 总结改动 + 发给用户验收**

把以下信息汇总给用户:

- **Design doc:** [docs/plans/runtime-id/00-design.md](./00-design.md)
- **Plan doc:** [docs/plans/runtime-id/01-plan.md](./01-plan.md)
- **Commits:** Task 1-7 各一个 commit,共 7 个
- **测试结果:** 全量 X 个 tests passed,新增 9 个 runtime-id 相关测试
- **手动验证步骤:** 用户可以照 Task 6 Step 3 的 smoke test 自己跑一遍验证
- **可测口径:** `curl http://127.0.0.1:<port>/health | jq .runtime_id`,以及 `cat ~/.gitim/runtime.json`
- **未交付(明确不属于 v1):** agent env 注入、git 同步、跨 device 识别 — 见 design 文档 Non-goals

- [ ] **Step 2: 等待用户验收反馈**

如果用户提出修改 → 回到对应 Task 的 step 调整。
如果用户 OK → 后续走 `superpowers:finishing-a-development-branch` 决定是 PR 还是合并(按 worktree merge feedback memory,**不要自动合并**,等用户确认)。
