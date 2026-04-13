# Agent + Human E2E 交互环境 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Runtime 能在 git/init 后起 human daemon，暴露 IM + Agent 管理 HTTP API，用 E2E 测试验证人类与 agent（MockProvider）的完整交互链路。

**Architecture:** Runtime 在 git/init 完成后自动 clone shared repo → 起 human daemon → 持有 GitimClient。IM 请求通过 HTTP → GitimClient → Unix socket 代理。Agent 管理 API 控制 provision + agent_loop 生命周期。MockProvider 实现 Provider trait 返回固定文本。

**Tech Stack:** Rust (axum, gitim-client, gitim-agent-provider), TypeScript (Playwright), E2E

---

## File Structure

### Rust (crates/gitim-runtime/)
- **Modify:** `src/http.rs` — 扩展 RuntimeState，添加 `/im/*` 和 `/agents/*` 路由
- **Modify:** `src/agent.rs` — 添加 human daemon provisioning 函数
- **Modify:** `src/agent_loop.rs` — 支持接收 Provider 实例（而非硬编码 claude）
- **Modify:** `src/error.rs` — 添加新错误变体
- **Modify:** `src/lib.rs` — 导出新模块

### Rust (crates/gitim-agent-provider/)
- **Create:** `src/mock.rs` — MockProvider 实现
- **Modify:** `src/provider.rs` — 注册 "mock" provider
- **Modify:** `src/lib.rs` — 导出 mock 模块

### E2E (e2e/)
- **Create:** `tests/human-im.spec.ts` — Human IM API 测试
- **Create:** `tests/agent-management.spec.ts` — Agent CRUD + 启停测试
- **Create:** `tests/agent-interaction.spec.ts` — 人类与 agent 交互测试
- **Create:** `helpers/runtime-env.ts` — 共享的 runtime 环境 setup/teardown

---

## Task 1: E2E Helper — 共享 Runtime 环境

抽出 startup.spec.ts 中重复的环境启动逻辑，后续测试复用。

**Files:**
- Create: `e2e/helpers/runtime-env.ts`

- [ ] **Step 1: 创建 runtime-env.ts**

```typescript
// e2e/helpers/runtime-env.ts
import { execSync, spawn, type ChildProcess } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as net from "node:net";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "../..");
const WEBUI_DIR = path.join(ROOT, "webui-v2");

export async function freePort(): Promise<number> {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address() as net.AddressInfo;
      srv.close(() => resolve(addr.port));
    });
  });
}

export async function waitForHealth(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      const data = await res.json();
      if (data.service === "gitim-runtime") return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Runtime did not become healthy at ${url}`);
}

export async function waitForHttp(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (res.ok) return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Server did not become available at ${url}`);
}

export interface RuntimeEnv {
  runtimePort: number;
  vitePort: number;
  workspaceDir: string;
  runtimeProc: ChildProcess;
  viteProc: ChildProcess;
  baseUrl: string;
}

/** Build runtime binary (idempotent). */
export function buildRuntime() {
  execSync("cargo build -p gitim-runtime", {
    cwd: ROOT,
    stdio: "inherit",
  });
}

/**
 * Start a full runtime + webui environment.
 * Completes the startup flow (workspace + git/init) via HTTP so tests
 * begin in "ready" state.
 */
export async function startEnv(): Promise<RuntimeEnv> {
  const workspaceDir = fs.mkdtempSync(path.join(os.tmpdir(), "gitim-e2e-"));
  const runtimePort = await freePort();
  const vitePort = await freePort();

  const runtimeBin = path.join(ROOT, "target/debug/gitim-runtime");
  const runtimeProc = spawn(runtimeBin, ["--port", String(runtimePort)], {
    stdio: "pipe",
  });

  const viteProc = spawn(
    "npx",
    ["vite", "--port", String(vitePort), "--strictPort", "--host", "127.0.0.1"],
    {
      cwd: WEBUI_DIR,
      stdio: "pipe",
      env: { ...process.env, BROWSER: "none" },
    },
  );

  const baseUrl = `http://127.0.0.1:${runtimePort}`;

  await Promise.all([
    waitForHealth(`${baseUrl}/health`),
    waitForHttp(`http://127.0.0.1:${vitePort}`),
  ]);

  // Complete startup flow: workspace + git/init
  const wsRes = await fetch(`${baseUrl}/workspace`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path: workspaceDir }),
  });
  const wsData = await wsRes.json();
  if (!wsData.ok) throw new Error(`workspace setup failed: ${wsData.error}`);

  const gitRes = await fetch(`${baseUrl}/git/init`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ provider: "local" }),
  });
  const gitData = await gitRes.json();
  if (!gitData.ok) throw new Error(`git/init failed: ${gitData.error}`);

  return { runtimePort, vitePort, workspaceDir, runtimeProc, viteProc, baseUrl };
}

export function stopEnv(env: RuntimeEnv) {
  env.runtimeProc?.kill();
  env.viteProc?.kill();
  if (env.workspaceDir && fs.existsSync(env.workspaceDir)) {
    fs.rmSync(env.workspaceDir, { recursive: true, force: true });
  }
}
```

- [ ] **Step 2: 验证编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx tsc --noEmit`
Expected: 无错误（或只有已知 warning）

- [ ] **Step 3: Commit**

```bash
git add e2e/helpers/runtime-env.ts
git commit -m "refactor(e2e): extract shared runtime-env helper"
```

---

## Task 2: Human Daemon Provisioning（Rust 侧）

git/init 完成后，Runtime 自动 clone bare repo → 起 daemon → onboard → 存 client path。

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — 扩展 RuntimeState + git_init 后起 human daemon
- Modify: `crates/gitim-runtime/src/agent.rs` — 添加 `provision_human` 函数
- Modify: `crates/gitim-runtime/src/error.rs` — 添加错误变体

- [ ] **Step 1: 扩展 RuntimeState**

在 `crates/gitim-runtime/src/http.rs` 中，扩展 state：

```rust
#[derive(Default)]
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
}
```

- [ ] **Step 2: 添加 provision_human 函数**

在 `crates/gitim-runtime/src/agent.rs` 中添加：

```rust
/// Provision the human daemon: clone bare repo → start daemon → onboard with local git identity.
///
/// The human directory lives at `.gitim-runtime/human/` inside the workspace.
/// Identity is derived from `git config user.name` and `git config user.email`.
pub async fn provision_human(workspace: &Path) -> Result<PathBuf, RuntimeError> {
    let bare_repo = workspace.join("repo.git");
    let human_dir = workspace.join(".gitim-runtime/human");

    // Clone bare repo into human dir (idempotent)
    if human_dir.exists() {
        info!("human directory exists, skipping clone");
    } else {
        let output = Command::new("git")
            .args(["clone", &bare_repo.to_string_lossy(), "human"])
            .current_dir(workspace.join(".gitim-runtime"))
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::GitCloneFailed(stderr.to_string()));
        }
        info!("cloned bare repo for human");
    }

    // Ensure .gitim/ exists
    std::fs::create_dir_all(human_dir.join(".gitim"))?;

    // Start daemon
    let root = human_dir.clone();
    tokio::task::spawn_blocking(move || ensure_daemon(&root))
        .await
        .map_err(|e| RuntimeError::DaemonStartFailed(
            gitim_client::ClientError::ConnectionFailed(format!("task panicked: {e}"))
        ))??;
    info!("human daemon running");

    // Detect identity from git config
    let handler = detect_git_config("user.name", &human_dir)
        .unwrap_or_else(|| "human".to_string())
        .to_lowercase()
        .replace(' ', "-");
    let display_name = detect_git_config("user.name", &human_dir)
        .unwrap_or_else(|| "Human".to_string());

    // Onboard
    let client = GitimClient::new(&human_dir);
    let resp = client
        .onboard(
            "git",
            json!({
                "type": "git",
                "handler": handler,
                "display_name": display_name,
            }),
            true, // admin
            false,
        )
        .await
        .map_err(|e| RuntimeError::OnboardFailed(e.to_string()))?;

    if !resp.ok {
        let msg = resp.error.unwrap_or_else(|| "unknown onboard error".into());
        return Err(RuntimeError::OnboardFailed(msg));
    }
    info!(handler = %handler, "human onboarded");

    // Verify daemon is responsive
    client
        .status()
        .await
        .map_err(|e| RuntimeError::OnboardFailed(format!("status check failed: {e}")))?;

    Ok(human_dir)
}

fn detect_git_config(key: &str, cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if val.is_empty() { None } else { Some(val) }
    } else {
        None
    }
}
```

- [ ] **Step 3: 在 git_init 中调用 provision_human**

在 `crates/gitim-runtime/src/http.rs` 的 `git_init` handler 中，git init 成功后添加 human daemon provisioning：

```rust
async fn git_init(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<GitInitRequest>,
) -> Json<serde_json::Value> {
    // ... existing git init code ...

    match output {
        Ok(o) if o.status.success() => {
            // Provision human daemon after bare repo is ready
            let workspace = workspace.clone();
            match crate::agent::provision_human(&workspace).await {
                Ok(human_repo) => {
                    let mut s = state.lock().unwrap();
                    s.human_repo = Some(human_repo);
                    Json(serde_json::json!({
                        "ok": true,
                        "repo_path": repo_path.to_string_lossy()
                    }))
                }
                Err(e) => {
                    Json(serde_json::json!({
                        "ok": false,
                        "error": format!("human daemon setup failed: {e}")
                    }))
                }
            }
        }
        // ... existing error handling ...
    }
}
```

- [ ] **Step 4: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/agent.rs
git commit -m "feat(runtime): provision human daemon after git/init"
```

---

## Task 3: `/im/me` 端点 + E2E 测试

第一个 IM 端点：返回 human daemon 的身份信息。

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — 添加 `/im/me` 路由
- Create: `e2e/tests/human-im.spec.ts` — 第一个 IM E2E 测试

- [ ] **Step 1: 添加 im_me handler**

在 `crates/gitim-runtime/src/http.rs` 中添加：

```rust
async fn im_me(
    State(state): State<SharedRuntimeState>,
) -> Json<serde_json::Value> {
    let human_repo = {
        let s = state.lock().unwrap();
        match &s.human_repo {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "human daemon not initialized"
                }));
            }
        }
    };

    let client = GitimClient::new(&human_repo);
    match client.status().await {
        Ok(resp) => Json(serde_json::json!({
            "ok": true,
            "data": resp.data
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("daemon error: {e}")
        })),
    }
}
```

在 `create_router()` 中添加路由：

```rust
.route("/im/me", get(im_me))
```

添加 import：

```rust
use gitim_client::GitimClient;
```

- [ ] **Step 2: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 3: 写 E2E 测试**

```typescript
// e2e/tests/human-im.spec.ts
import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("human IM API", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("/im/me returns human identity after setup", async () => {
    const res = await fetch(`${env.baseUrl}/im/me`);
    const data = await res.json();

    expect(data.ok).toBe(true);
    expect(data.data).toBeDefined();
  });
});
```

- [ ] **Step 4: 运行 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/human-im.spec.ts`
Expected: 1 passed

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/http.rs e2e/tests/human-im.spec.ts
git commit -m "feat(runtime): add /im/me endpoint with e2e test"
```

---

## Task 4: `/im/channels` + `/im/send` + `/im/read` 端点

代理 human daemon 的消息收发能力。

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — 添加 3 个端点

- [ ] **Step 1: 添加 helper 提取 human client**

在 `http.rs` 中添加复用函数：

```rust
fn human_client(state: &SharedRuntimeState) -> Result<GitimClient, Json<serde_json::Value>> {
    let s = state.lock().unwrap();
    match &s.human_repo {
        Some(p) => Ok(GitimClient::new(p)),
        None => Err(Json(serde_json::json!({
            "ok": false,
            "error": "human daemon not initialized"
        }))),
    }
}
```

- [ ] **Step 2: 添加 im_channels handler**

```rust
async fn im_channels(
    State(state): State<SharedRuntimeState>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.list_channels().await {
        Ok(resp) => Json(serde_json::json!({ "ok": true, "data": resp.data })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}
```

- [ ] **Step 3: 添加 im_send handler**

```rust
#[derive(Deserialize)]
struct SendRequest {
    channel: String,
    body: String,
    #[serde(default)]
    reply_to: Option<u64>,
}

async fn im_send(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<SendRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.send(&req.channel, &req.body, None, req.reply_to).await {
        Ok(resp) => Json(serde_json::json!({ "ok": true, "data": resp.data })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}
```

- [ ] **Step 4: 添加 im_read handler**

```rust
#[derive(Deserialize)]
struct ReadRequest {
    channel: String,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    since: Option<u64>,
}

async fn im_read(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<ReadRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    match client.read(&req.channel, req.limit, req.since).await {
        Ok(resp) => Json(serde_json::json!({ "ok": true, "data": resp.data })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}
```

- [ ] **Step 5: 注册路由**

在 `create_router()` 中添加：

```rust
.route("/im/channels", get(im_channels))
.route("/im/send", post(im_send))
.route("/im/read", post(im_read))
```

- [ ] **Step 6: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 7: 添加 E2E 测试**

在 `e2e/tests/human-im.spec.ts` 中添加：

```typescript
  test("/im/send + /im/read round-trip", async () => {
    // First create a channel via daemon (send implicitly creates)
    const sendRes = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "hello from e2e" }),
    });
    const sendData = await sendRes.json();
    expect(sendData.ok).toBe(true);

    // Read it back
    const readRes = await fetch(`${env.baseUrl}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general" }),
    });
    const readData = await readRes.json();
    expect(readData.ok).toBe(true);
    expect(readData.data).toBeDefined();

    // Verify message exists in response
    const messages = readData.data.entries ?? readData.data;
    const found = Array.isArray(messages) && messages.some(
      (m: any) => typeof m === "object" && (m.body?.includes("hello from e2e") || JSON.stringify(m).includes("hello from e2e"))
    );
    expect(found).toBe(true);
  });

  test("/im/channels returns channel list", async () => {
    const res = await fetch(`${env.baseUrl}/im/channels`);
    const data = await res.json();
    expect(data.ok).toBe(true);
  });
```

- [ ] **Step 8: 运行 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/human-im.spec.ts`
Expected: 3 passed

- [ ] **Step 9: Commit**

```bash
git add crates/gitim-runtime/src/http.rs e2e/tests/human-im.spec.ts
git commit -m "feat(runtime): add /im/channels, /im/send, /im/read endpoints"
```

---

## Task 5: `/im/poll` 端点

Human 轮询变化 — agent 消息到达后能被 poll 到。

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — 添加 `/im/poll` 路由 + 持久化 cursor

- [ ] **Step 1: 扩展 RuntimeState 存储 poll cursor**

```rust
#[derive(Default)]
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
}
```

- [ ] **Step 2: 添加 im_poll handler**

```rust
#[derive(Deserialize)]
struct PollRequest {
    #[serde(default)]
    since: Option<String>,
}

async fn im_poll(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<PollRequest>,
) -> Json<serde_json::Value> {
    let (client, cursor) = {
        let s = state.lock().unwrap();
        let repo = match &s.human_repo {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "human daemon not initialized"
                }));
            }
        };
        let cursor = req.since.clone().or_else(|| s.poll_cursor.clone());
        (GitimClient::new(&repo), cursor)
    };

    match client.poll(cursor.as_deref()).await {
        Ok(resp) => {
            // Update stored cursor
            if let Some(data) = &resp.data {
                if let Some(commit_id) = data["commit_id"].as_str() {
                    let mut s = state.lock().unwrap();
                    s.poll_cursor = Some(commit_id.to_string());
                }
            }
            Json(serde_json::json!({ "ok": true, "data": resp.data }))
        }
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}
```

- [ ] **Step 3: 注册路由**

```rust
.route("/im/poll", post(im_poll))
```

- [ ] **Step 4: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 5: 添加 E2E 测试**

在 `e2e/tests/human-im.spec.ts` 中添加：

```typescript
  test("/im/poll returns changes since cursor", async () => {
    // First poll initializes cursor
    const poll1 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });
    const data1 = await poll1.json();
    expect(data1.ok).toBe(true);
    const cursor = data1.data?.commit_id;
    expect(cursor).toBeDefined();

    // Send a message
    await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "poll-test", body: "poll message" }),
    });

    // Poll again — should see the new message
    const poll2 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ since: cursor }),
    });
    const data2 = await poll2.json();
    expect(data2.ok).toBe(true);
    expect(data2.data?.changes?.length).toBeGreaterThan(0);
  });
```

- [ ] **Step 6: 运行 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/human-im.spec.ts`
Expected: 4 passed

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-runtime/src/http.rs e2e/tests/human-im.spec.ts
git commit -m "feat(runtime): add /im/poll endpoint with cursor tracking"
```

---

## Task 6: MockProvider 实现

在 gitim-agent-provider 中添加 "mock" provider，返回固定文本。

**Files:**
- Create: `crates/gitim-agent-provider/src/mock.rs`
- Modify: `crates/gitim-agent-provider/src/provider.rs` — 注册 "mock"
- Modify: `crates/gitim-agent-provider/src/lib.rs` — 导出

- [ ] **Step 1: 创建 MockProvider**

```rust
// crates/gitim-agent-provider/src/mock.rs
use async_trait::async_trait;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, Session};

pub struct MockProvider {
    default_response: String,
}

impl MockProvider {
    pub fn new(_config: ProviderConfig) -> Self {
        Self {
            default_response: "mock-response: acknowledged".to_string(),
        }
    }

    pub fn with_response(response: String) -> Self {
        Self {
            default_response: response,
        }
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let response = self.default_response.clone();
        let cwd = opts.cwd.clone();

        let (event_tx, event_rx) = mpsc::channel(16);
        let (result_tx, result_rx) = oneshot::channel();

        let handle = tokio::spawn(async move {
            let start = Instant::now();

            // Emit the response as text event
            let _ = event_tx.send(Event::Text { content: response.clone() }).await;

            // If cwd is set, execute gitim send with the response
            if let Some(cwd) = &cwd {
                // Parse channel from prompt to know where to reply
                // Format: [#channel] @author: message
                let channel = prompt
                    .lines()
                    .find(|l| l.starts_with("[#"))
                    .and_then(|l| l.strip_prefix("[#"))
                    .and_then(|l| l.split(']').next())
                    .unwrap_or("general");

                let _ = event_tx.send(Event::ToolUse {
                    tool: "bash".to_string(),
                    call_id: "mock-send".to_string(),
                    input: serde_json::json!({"command": format!("gitim send {} \"{}\"", channel, response)}),
                }).await;

                // Actually execute the send via gitim CLI
                let output = std::process::Command::new("gitim")
                    .args(["send", channel, &response])
                    .current_dir(cwd)
                    .output();

                match output {
                    Ok(o) if o.status.success() => {
                        let _ = event_tx.send(Event::ToolResult {
                            call_id: "mock-send".to_string(),
                            output: String::from_utf8_lossy(&o.stdout).to_string(),
                        }).await;
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr);
                        let _ = event_tx.send(Event::Error {
                            content: format!("gitim send failed: {stderr}"),
                        }).await;
                    }
                    Err(e) => {
                        let _ = event_tx.send(Event::Error {
                            content: format!("failed to run gitim: {e}"),
                        }).await;
                    }
                }
            }

            let duration_ms = start.elapsed().as_millis() as u64;
            let _ = result_tx.send(ExecResult {
                status: ExecStatus::Completed,
                output: response,
                error: None,
                duration_ms,
                session_token: None,
            });
        });

        Ok(Session::new(event_rx, result_rx, handle.abort_handle()))
    }
}
```

- [ ] **Step 2: 注册 "mock" provider**

在 `crates/gitim-agent-provider/src/provider.rs` 的 `create` 函数中添加：

```rust
"mock" => Ok(Box::new(crate::mock::MockProvider::new(config))),
```

- [ ] **Step 3: 导出 mock 模块**

在 `crates/gitim-agent-provider/src/lib.rs` 中添加：

```rust
pub mod mock;
```

- [ ] **Step 4: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-agent-provider 2>&1`
Expected: 编译成功

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-agent-provider/src/mock.rs crates/gitim-agent-provider/src/provider.rs crates/gitim-agent-provider/src/lib.rs
git commit -m "feat(provider): add MockProvider with fixed text response"
```

---

## Task 7: Agent 管理 — RuntimeState 扩展 + `/agents/add` + `/agents/list`

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — AgentInfo 结构 + 两个端点

- [ ] **Step 1: 添加 AgentInfo 和扩展 RuntimeState**

在 `http.rs` 中添加：

```rust
use std::collections::HashMap;
use tokio::task::AbortHandle;

#[derive(Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub handler: String,
    pub display_name: String,
    pub status: String, // "idle", "running", "error"
    #[serde(skip)]
    pub repo_root: PathBuf,
    #[serde(skip)]
    pub loop_handle: Option<AbortHandle>,
}

#[derive(Default)]
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
    pub human_repo: Option<PathBuf>,
    pub poll_cursor: Option<String>,
    pub agents: HashMap<String, AgentInfo>,
}
```

- [ ] **Step 2: 添加 agents_add handler**

```rust
#[derive(Deserialize)]
struct AddAgentRequest {
    handler: String,
    display_name: String,
}

async fn agents_add(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AddAgentRequest>,
) -> Json<serde_json::Value> {
    let workspace = {
        let s = state.lock().unwrap();
        match &s.workspace {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "workspace not set"
                }));
            }
        }
    };

    let bare_repo = workspace.join("repo.git");
    let agents_dir = workspace.join(".gitim-runtime/agents");
    if let Err(e) = std::fs::create_dir_all(&agents_dir) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to create agents dir: {e}")
        }));
    }

    let config = crate::agent::AgentConfig {
        handler: req.handler.clone(),
        display_name: req.display_name.clone(),
        remote_url: bare_repo.to_string_lossy().to_string(),
    };

    match crate::agent::provision_agent(&agents_dir, &config).await {
        Ok(handle) => {
            let info = AgentInfo {
                id: req.handler.clone(),
                handler: req.handler.clone(),
                display_name: req.display_name,
                status: "idle".to_string(),
                repo_root: handle.repo_root,
                loop_handle: None,
            };
            let mut s = state.lock().unwrap();
            s.agents.insert(req.handler.clone(), info);
            Json(serde_json::json!({ "ok": true, "id": req.handler }))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": e.to_string()
        })),
    }
}
```

- [ ] **Step 3: 添加 agents_list handler**

```rust
async fn agents_list(
    State(state): State<SharedRuntimeState>,
) -> Json<serde_json::Value> {
    let s = state.lock().unwrap();
    let agents: Vec<&AgentInfo> = s.agents.values().collect();
    Json(serde_json::json!({ "ok": true, "agents": agents }))
}
```

- [ ] **Step 4: 注册路由**

```rust
.route("/agents", get(agents_list))
.route("/agents/add", post(agents_add))
```

- [ ] **Step 5: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 6: 写 E2E 测试**

```typescript
// e2e/tests/agent-management.spec.ts
import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("agent management API", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("/agents/add + /agents list", async () => {
    // Add an agent
    const addRes = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "test-agent", display_name: "Test Agent" }),
    });
    const addData = await addRes.json();
    expect(addData.ok).toBe(true);
    expect(addData.id).toBe("test-agent");

    // List agents
    const listRes = await fetch(`${env.baseUrl}/agents`);
    const listData = await listRes.json();
    expect(listData.ok).toBe(true);
    expect(listData.agents.length).toBe(1);
    expect(listData.agents[0].handler).toBe("test-agent");
    expect(listData.agents[0].status).toBe("idle");
  });
});
```

- [ ] **Step 7: 运行 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/agent-management.spec.ts`
Expected: 1 passed

- [ ] **Step 8: Commit**

```bash
git add crates/gitim-runtime/src/http.rs e2e/tests/agent-management.spec.ts
git commit -m "feat(runtime): add /agents/add and /agents list endpoints"
```

---

## Task 8: `/agents/start` + `/agents/stop`

启动 agent_loop 作为 tokio task，stop 时 abort。

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — 两个端点
- Modify: `crates/gitim-runtime/src/agent_loop.rs` — 支持传入 provider type

- [ ] **Step 1: 修改 AgentLoop 支持 provider 类型**

在 `crates/gitim-runtime/src/agent_loop.rs` 中，添加 `with_provider` 构造函数：

```rust
/// Build an AgentLoop with a specific provider type.
pub fn with_provider(repo_root: &Path, provider_type: &str) -> Result<Self, RuntimeError> {
    let state = AgentState::load(repo_root)?;

    let poller = match state.cursor {
        Some(cursor) => {
            info!(cursor = %cursor, "restored cursor from state");
            Poller::with_cursor(GitimClient::new(repo_root), cursor)
        }
        None => Poller::new(GitimClient::new(repo_root)),
    };

    let provider = create(provider_type, ProviderConfig::default())
        .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

    Ok(Self {
        poller,
        provider,
        session_token: state.session_token,
        poll_interval: Duration::from_secs(2),
        repo_root: repo_root.to_path_buf(),
        model: Some("claude-sonnet-4-6".to_string()),
    })
}
```

- [ ] **Step 2: 添加 agents_start handler**

在 `http.rs` 中添加：

```rust
#[derive(Deserialize)]
struct AgentIdRequest {
    id: String,
}

async fn agents_start(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    let (repo_root, current_status) = {
        let s = state.lock().unwrap();
        match s.agents.get(&req.id) {
            Some(info) => {
                if info.status == "running" {
                    return Json(serde_json::json!({
                        "ok": false,
                        "error": "agent already running"
                    }));
                }
                (info.repo_root.clone(), info.status.clone())
            }
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "agent not found"
                }));
            }
        }
    };

    // Create agent loop with mock provider
    let mut agent_loop = match AgentLoop::with_provider(&repo_root, "mock") {
        Ok(al) => al,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "error": format!("failed to create agent loop: {e}")
            }));
        }
    };

    // Spawn as background task
    let state_clone = state.clone();
    let agent_id = req.id.clone();
    let handle = tokio::spawn(async move {
        let result = agent_loop.run().await;
        // On exit (error or abort), update status
        let mut s = state_clone.lock().unwrap();
        if let Some(info) = s.agents.get_mut(&agent_id) {
            info.status = match &result {
                Ok(_) => "idle".to_string(),
                Err(e) => format!("error: {e}"),
            };
            info.loop_handle = None;
        }
    });

    // Store abort handle and update status
    {
        let mut s = state.lock().unwrap();
        if let Some(info) = s.agents.get_mut(&req.id) {
            info.loop_handle = Some(handle.abort_handle());
            info.status = "running".to_string();
        }
    }

    Json(serde_json::json!({ "ok": true }))
}
```

- [ ] **Step 3: 添加 agents_stop handler**

```rust
async fn agents_stop(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    let mut s = state.lock().unwrap();
    match s.agents.get_mut(&req.id) {
        Some(info) => {
            if let Some(handle) = info.loop_handle.take() {
                handle.abort();
            }
            info.status = "idle".to_string();
            Json(serde_json::json!({ "ok": true }))
        }
        None => Json(serde_json::json!({
            "ok": false,
            "error": "agent not found"
        })),
    }
}
```

- [ ] **Step 4: 注册路由**

```rust
.route("/agents/start", post(agents_start))
.route("/agents/stop", post(agents_stop))
```

添加 import：

```rust
use crate::AgentLoop;
```

- [ ] **Step 5: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 6: 添加 E2E 测试**

在 `e2e/tests/agent-management.spec.ts` 中添加：

```typescript
  test("/agents/start + /agents/stop lifecycle", async () => {
    // Add agent first
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "lifecycle-agent", display_name: "Lifecycle Agent" }),
    });

    // Start it
    const startRes = await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-agent" }),
    });
    expect((await startRes.json()).ok).toBe(true);

    // Verify status is running
    const list1 = await fetch(`${env.baseUrl}/agents`);
    const data1 = await list1.json();
    const agent1 = data1.agents.find((a: any) => a.id === "lifecycle-agent");
    expect(agent1.status).toBe("running");

    // Stop it
    const stopRes = await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-agent" }),
    });
    expect((await stopRes.json()).ok).toBe(true);

    // Verify status is idle
    const list2 = await fetch(`${env.baseUrl}/agents`);
    const data2 = await list2.json();
    const agent2 = data2.agents.find((a: any) => a.id === "lifecycle-agent");
    expect(agent2.status).toBe("idle");
  });
```

- [ ] **Step 7: 运行 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/agent-management.spec.ts`
Expected: 2 passed

- [ ] **Step 8: Commit**

```bash
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/agent_loop.rs e2e/tests/agent-management.spec.ts
git commit -m "feat(runtime): add /agents/start and /agents/stop endpoints"
```

---

## Task 9: 人类与 Agent 交互 E2E

完整链路：人类发消息 → agent daemon sync → agent_loop poll → MockProvider 回复 → human daemon sync → human poll 可见。

**Files:**
- Create: `e2e/tests/agent-interaction.spec.ts`

- [ ] **Step 1: 写交互 E2E 测试**

```typescript
// e2e/tests/agent-interaction.spec.ts
import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("human-agent interaction", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("human sends message, agent replies via MockProvider", async () => {
    // 1. Add and start agent
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "mock-bot", display_name: "Mock Bot" }),
    });

    await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "mock-bot" }),
    });

    // 2. Initialize poll cursor
    const poll0 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });
    const cursor0 = (await poll0.json()).data?.commit_id;

    // 3. Human sends a message
    const sendRes = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "hello agent" }),
    });
    expect((await sendRes.json()).ok).toBe(true);

    // 4. Wait for agent to process and reply (poll with retries)
    // Agent polls every 2s, git sync takes time. Allow up to 30s.
    let agentReplied = false;
    const deadline = Date.now() + 30_000;

    while (Date.now() < deadline) {
      const pollRes = await fetch(`${env.baseUrl}/im/poll`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ since: cursor0 }),
      });
      const pollData = await pollRes.json();

      if (pollData.ok && pollData.data?.changes) {
        for (const change of pollData.data.changes) {
          for (const entry of change.entries ?? []) {
            const body = entry.body ?? JSON.stringify(entry);
            if (body.includes("mock-response")) {
              agentReplied = true;
              break;
            }
          }
          if (agentReplied) break;
        }
      }
      if (agentReplied) break;
      await new Promise((r) => setTimeout(r, 2000));
    }

    expect(agentReplied).toBe(true);

    // 5. Cleanup: stop agent
    await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "mock-bot" }),
    });
  });
});
```

- [ ] **Step 2: 运行交互 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/agent-interaction.spec.ts --timeout 60000`
Expected: 1 passed

- [ ] **Step 3: Commit**

```bash
git add e2e/tests/agent-interaction.spec.ts
git commit -m "test(e2e): add human-agent interaction test via MockProvider"
```

---

## Task 10: `/agents/{id}` + `/agents/remove` + 运行时状态

完善 Agent CRUD 和状态查询。

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs` — 添加 get/remove 端点

- [ ] **Step 1: 添加 agents_get handler**

```rust
async fn agents_get(
    State(state): State<SharedRuntimeState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let s = state.lock().unwrap();
    match s.agents.get(&id) {
        Some(info) => Json(serde_json::json!({ "ok": true, "agent": info })),
        None => Json(serde_json::json!({ "ok": false, "error": "agent not found" })),
    }
}
```

- [ ] **Step 2: 添加 agents_remove handler**

```rust
async fn agents_remove(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<AgentIdRequest>,
) -> Json<serde_json::Value> {
    let mut s = state.lock().unwrap();
    match s.agents.remove(&req.id) {
        Some(info) => {
            // Abort loop if running
            if let Some(handle) = &info.loop_handle {
                handle.abort();
            }
            Json(serde_json::json!({ "ok": true }))
        }
        None => Json(serde_json::json!({ "ok": false, "error": "agent not found" })),
    }
}
```

- [ ] **Step 3: 注册路由**

```rust
.route("/agents/:id", get(agents_get))
.route("/agents/remove", post(agents_remove))
```

- [ ] **Step 4: Build 验证**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo build -p gitim-runtime 2>&1`
Expected: 编译成功

- [ ] **Step 5: 添加 E2E 测试**

在 `e2e/tests/agent-management.spec.ts` 中添加：

```typescript
  test("/agents/:id returns agent detail", async () => {
    // Add agent
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "detail-agent", display_name: "Detail Agent" }),
    });

    const res = await fetch(`${env.baseUrl}/agents/detail-agent`);
    const data = await res.json();
    expect(data.ok).toBe(true);
    expect(data.agent.handler).toBe("detail-agent");
    expect(data.agent.display_name).toBe("Detail Agent");
  });

  test("/agents/remove removes agent", async () => {
    // Add agent
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "doomed-agent", display_name: "Doomed Agent" }),
    });

    // Remove it
    const removeRes = await fetch(`${env.baseUrl}/agents/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "doomed-agent" }),
    });
    expect((await removeRes.json()).ok).toBe(true);

    // Verify gone
    const getRes = await fetch(`${env.baseUrl}/agents/doomed-agent`);
    const getData = await getRes.json();
    expect(getData.ok).toBe(false);
  });
```

- [ ] **Step 6: 运行全部 agent management 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test tests/agent-management.spec.ts`
Expected: 4 passed

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-runtime/src/http.rs e2e/tests/agent-management.spec.ts
git commit -m "feat(runtime): add /agents/:id get and /agents/remove endpoints"
```

---

## Task 11: 全量 E2E 回归验证

确保所有测试一起通过，包括原有 startup 测试。

- [ ] **Step 1: 运行全部 E2E 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e/e2e && npx playwright test --timeout 60000`
Expected: 所有测试通过（startup 4 + human-im 4 + agent-management 4 + agent-interaction 1 = 13）

- [ ] **Step 2: 运行 Rust 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/agent-human-e2e && cargo test 2>&1`
Expected: 全部通过

- [ ] **Step 3: 修复任何失败**

如果有测试失败，定位原因并修复。

- [ ] **Step 4: Final commit (if fixes needed)**

```bash
git commit -m "fix: resolve e2e test regressions"
```
