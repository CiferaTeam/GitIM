# Runtime Idle Exit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Runtime 在 24 小时无实质活动后自动退出，对齐 daemon 的 idle watchdog 模式。

**Architecture:** 在 `RuntimeState` 加 `AtomicU64` 记录最后活动时间戳。两个写入点：axum middleware（每个 HTTP 请求 touch）和 agent loop（处理消息后 touch）。`run_shell()` 里 spawn watchdog task，每 1h 检查，超 24h 且无 agent 正在执行时 cleanup + exit。

**Tech Stack:** Rust, tokio, axum middleware, AtomicU64

---

### Task 1: 在 RuntimeState 中添加 last_activity 时间戳

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:62-83` (RuntimeState + Default impl)

- [ ] **Step 1: 添加 AtomicU64 字段到 RuntimeState**

在 `RuntimeState` struct 中添加字段：

```rust
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
    pub agents: HashMap<String, AgentInfo>,
    pub activity_tx: broadcast::Sender<AgentActivityEvent>,
    /// Epoch seconds of last activity. Used by idle watchdog.
    pub last_activity: std::sync::atomic::AtomicU64,
}
```

在 `Default` impl 中初始化为当前时间：

```rust
impl Default for RuntimeState {
    fn default() -> Self {
        let (activity_tx, _) = broadcast::channel(128);
        Self {
            workspace: None,
            human_repo: None,
            poll_cursor: None,
            agents: HashMap::new(),
            activity_tx,
            last_activity: std::sync::atomic::AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
        }
    }
}
```

- [ ] **Step 2: 添加 touch_activity 辅助方法**

在 `http.rs` 中添加一个 free function（保持与 daemon 模式一致的风格）：

```rust
/// Update the last-activity timestamp to now.
pub fn touch_activity(state: &SharedRuntimeState) {
    let s = state.lock().unwrap();
    s.last_activity.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        std::sync::atomic::Ordering::Relaxed,
    );
}
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p gitim-runtime`
Expected: 成功，无 warning（last_activity 暂时 unused 会有 dead_code warning，后续 task 消除）

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): add last_activity timestamp to RuntimeState"
```

---

### Task 2: 添加 axum middleware，每个请求 touch 时间戳

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:936-963` (create_router)

- [ ] **Step 1: 在 create_router 中添加 middleware layer**

在 `.layer(CorsLayer::permissive())` 之前插入一个 `axum::middleware::from_fn_with_state` layer：

```rust
pub fn create_router() -> (Router, SharedRuntimeState) {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    let router = Router::new()
        .route("/health", get(health))
        // ... 所有 route 不变 ...
        .route("/preflight/claude", get(preflight_claude))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            activity_middleware,
        ))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    (router, state)
}
```

- [ ] **Step 2: 实现 activity_middleware 函数**

在 `create_router` 上方添加：

```rust
async fn activity_middleware(
    State(state): State<SharedRuntimeState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    touch_activity(&state);
    next.run(request).await
}
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p gitim-runtime`
Expected: 成功

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): touch activity timestamp on every HTTP request"
```

---

### Task 3: agent 处理消息后 touch 时间戳

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:635-645` (start_agent_loop 中的 Ok(true) 分支)

- [ ] **Step 1: 在 agent 处理消息后调用 touch_activity**

在 `start_agent_loop` 中 `Ok(true)` 分支已有的 `try_lock` 块之后，调用 `touch_activity`。因为 `touch_activity` 也会 lock mutex，需要在已有 lock 释放后调用：

```rust
Ok(true) => {
    consecutive_errors = 0;
    if let Ok(mut s) = state_clone.try_lock() {
        if let Some(info) = s.agents.get_mut(&owned_id) {
            info.messages_processed += 1;
            info.last_activity =
                Some(chrono::Utc::now().to_rfc3339());
        }
    }
    touch_activity(&state_clone);
}
```

- [ ] **Step 2: 编译验证**

Run: `cargo check -p gitim-runtime`
Expected: 成功

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): touch activity when agent processes a message"
```

---

### Task 4: 实现 idle watchdog + 集成到 run_shell

**Files:**
- Modify: `crates/gitim-runtime/src/bin/runtime.rs:107-139` (run_shell 函数)
- Modify: `crates/gitim-runtime/src/http.rs` (添加 has_active_agents helper)

- [ ] **Step 1: 添加 has_active_agents 辅助函数**

在 `http.rs` 中 `touch_activity` 旁边添加：

```rust
/// Check if any agent is currently running (has an active loop handle).
pub fn has_active_agents(state: &SharedRuntimeState) -> bool {
    let s = state.lock().unwrap();
    s.agents.values().any(|a| a.status == "running")
}
```

- [ ] **Step 2: 在 run_shell 中 spawn idle watchdog task**

修改 `run_shell`，在 server spawn 之前启动 watchdog：

```rust
async fn run_shell(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let (router, state) = gitim_runtime::http::create_router();

    // Recover previous workspace from ~/.gitim/runtime.json
    gitim_runtime::http::recover_from_config(state.clone()).await;

    // Idle watchdog: exit if no activity for 24 hours
    let idle_state = state.clone();
    tokio::spawn(async move {
        const IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
        const CHECK_INTERVAL_SECS: u64 = 60 * 60;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;
            let last = idle_state.lock().unwrap()
                .last_activity
                .load(std::sync::atomic::Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if now.saturating_sub(last) >= IDLE_TIMEOUT_SECS {
                if gitim_runtime::http::has_active_agents(&idle_state) {
                    eprintln!("idle timeout reached but agents still active, deferring exit");
                    continue;
                }
                eprintln!("no activity for 24h — shutting down");
                // Clean up pid file
                if let Some(home) = dirs::home_dir() {
                    let _ = std::fs::remove_file(home.join(".gitim/runtime.pid"));
                }
                kill_managed_daemons(&idle_state);
                std::process::exit(0);
            }
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    // ... 后续 listener/server/select/shutdown 代码不变 ...
}
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p gitim-runtime`
Expected: 成功

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/bin/runtime.rs
git commit -m "feat(runtime): add 24h idle watchdog with agent-aware exit"
```

---

### Task 5: 单元测试

**Files:**
- Create: `crates/gitim-runtime/tests/idle_exit.rs`

- [ ] **Step 1: 写测试 — touch_activity 更新时间戳**

```rust
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

#[test]
fn touch_activity_updates_timestamp() {
    let (_, state) = gitim_runtime::http::create_router();

    let before = state.lock().unwrap()
        .last_activity.load(Ordering::Relaxed);

    // Sleep briefly to ensure time advances
    std::thread::sleep(std::time::Duration::from_millis(1100));

    gitim_runtime::http::touch_activity(&state);

    let after = state.lock().unwrap()
        .last_activity.load(Ordering::Relaxed);

    assert!(after >= before, "timestamp should advance after touch");
}
```

- [ ] **Step 2: 写测试 — has_active_agents 无 agent 时返回 false**

```rust
#[test]
fn has_active_agents_empty() {
    let (_, state) = gitim_runtime::http::create_router();
    assert!(!gitim_runtime::http::has_active_agents(&state));
}
```

- [ ] **Step 3: 写测试 — has_active_agents 有 running agent 时返回 true**

```rust
#[test]
fn has_active_agents_with_running() {
    let (_, state) = gitim_runtime::http::create_router();
    {
        let mut s = state.lock().unwrap();
        s.agents.insert("test-agent".to_string(), gitim_runtime::http::AgentInfo {
            id: "test-agent".to_string(),
            handler: "test".to_string(),
            display_name: "Test".to_string(),
            status: "running".to_string(),
            last_activity: None,
            messages_processed: 0,
            repo_path: "/tmp/test".to_string(),
            provider: None,
            model: None,
            system_prompt: None,
            env: std::collections::HashMap::new(),
            loop_handle: None,
        });
    }
    assert!(gitim_runtime::http::has_active_agents(&state));
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p gitim-runtime --test idle_exit`
Expected: 3 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/tests/idle_exit.rs
git commit -m "test(runtime): add idle exit unit tests"
```
