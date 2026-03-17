# GitIM UX Onboarding Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 `gitim onboard` 统一入口命令、身份自动推断、daemon 身份注入，让 Agent 零配置即可发消息。

**Architecture:** 自底向上分 4 层实现：Core Config 扩展 → Daemon API 扩展（register_user、stop、身份注入）→ CLI daemon 管理增强 → CLI onboard 命令。每层可独立测试。

**Tech Stack:** Rust (gitim-core, gitim-daemon), TypeScript (gitim-cli, commander, child_process)

**Spec:** `docs/superpowers/specs/2026-03-17-gitim-ux-onboarding-design.md`

---

## Dependency Graph

```
Task 1: Core Config 扩展（endpoint 字段）
  │
  ├→ Task 2: Daemon 身份读取（me.json → AppState.current_user）
  │    │
  │    ├→ Task 3: Daemon API — register_user
  │    ├→ Task 4: Daemon API — stop
  │    └→ Task 5: Daemon send 身份注入（去掉 author 必填）
  │
  └→ Task 6: CLI daemon.ts 增强（stale 清理）
       │
       └→ Task 7: CLI onboard 命令
            │
            └→ Task 8: CLI send/dm 去掉 -a 必填 + stop 命令 + 注册 onboard
                 │
                 └→ Task 9: E2E 测试
```

---

## File Structure

### 新增文件

| 文件 | 职责 |
|------|------|
| `cli/src/commands/onboard.ts` | onboard 命令：身份推断、clone/create/init、注册用户 |
| `cli/src/commands/stop.ts` | stop 命令：发送 stop API 给 daemon |

### 修改文件

| 文件 | 变更 |
|------|------|
| `crates/gitim-core/src/types/config.rs` | 增加 `endpoint`、`endpoint_url` 字段 |
| `crates/gitim-core/tests/validator_test.rs` | 增加 endpoint 相关配置验证测试 |
| `crates/gitim-daemon/src/state.rs` | `AppState` 增加 `current_user: Option<String>` |
| `crates/gitim-daemon/src/main.rs` | 启动时读取 `me.json`，传入 `current_user` |
| `crates/gitim-daemon/src/api.rs` | Request 增加 `RegisterUser`、`Stop` 变体 |
| `crates/gitim-daemon/src/handlers.rs` | 新增 `handle_register_user`、`handle_stop`；`handle_send` 支持 author 缺省时用 current_user |
| ~~`crates/gitim-daemon/src/server.rs`~~ | 无需修改 — dispatch 通过 `handlers::handle_request` 自动覆盖新变体 |
| `cli/src/daemon.ts` | `ensureDaemon` 增加 stale PID/socket 清理 |
| `cli/src/client.ts` | `send` 方法 author 改为可选；新增 `registerUser`、`stop` 方法 |
| `cli/src/commands/send.ts` | `-a` 改为可选参数 |
| `cli/src/commands/dm.ts` | `-a` 改为可选参数 |
| `cli/src/commands/init.ts` | 删除（onboard.ts 内联了初始化逻辑） |
| `cli/src/index.ts` | 移除 init 命令，注册 onboard、stop 命令 |
| `tests/e2e_test.sh` | 更新适配新 API |

---

## Chunk 1: Core + Daemon 后端

### Task 1: Core Config 扩展

**Files:**
- Modify: `crates/gitim-core/src/types/config.rs`
- Modify: `crates/gitim-core/tests/validator_test.rs`

- [ ] **Step 1: 写失败测试 — endpoint 字段解析**

在 `crates/gitim-core/tests/validator_test.rs` 末尾添加：

```rust
#[test]
fn test_config_with_endpoint() {
    let yaml = "version: 1\nendpoint: github\n";
    let config = validate_config(yaml).unwrap();
    assert_eq!(config.endpoint, "github");
    assert_eq!(config.endpoint_url, "");
}

#[test]
fn test_config_with_gitea_endpoint() {
    let yaml = "version: 1\nendpoint: gitea\nendpoint_url: https://gitea.example.com\n";
    let config = validate_config(yaml).unwrap();
    assert_eq!(config.endpoint, "gitea");
    assert_eq!(config.endpoint_url, "https://gitea.example.com");
}

#[test]
fn test_config_endpoint_defaults() {
    let yaml = "version: 1\n";
    let config = validate_config(yaml).unwrap();
    assert_eq!(config.endpoint, "github");
    assert_eq!(config.endpoint_url, "");
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo test --test validator_test test_config_with_endpoint test_config_with_gitea_endpoint test_config_endpoint_defaults -- --nocapture`

Expected: 编译失败，`endpoint` 字段不存在。

- [ ] **Step 3: 实现 — 修改 Config struct**

修改 `crates/gitim-core/src/types/config.rs`，在 `Config` struct 中增加：

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub version: u32,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub endpoint_url: String,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

fn default_endpoint() -> String {
    "github".to_string()
}
```

**注意：** derive 必须保留完整的 `Debug, Clone, Serialize, Deserialize, PartialEq`，与现有 struct 一致。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo test --test validator_test -- --nocapture`

Expected: 所有 validator_test 通过（含原有测试）。

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add crates/gitim-core/src/types/config.rs crates/gitim-core/tests/validator_test.rs
git commit -m "feat(core): add endpoint and endpoint_url to Config"
```

---

### Task 2: Daemon 身份读取

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs`
- Modify: `crates/gitim-daemon/src/main.rs`

- [ ] **Step 1: 修改 AppState — 增加 current_user**

修改 `crates/gitim-daemon/src/state.rs`：

```rust
pub struct AppState {
    pub repo_root: PathBuf,
    pub config: Config,
    pub thread_cache: RwLock<HashMap<String, ThreadFile>>,
    pub users: RwLock<Vec<String>>,
    pub current_user: Option<String>,
}

impl AppState {
    pub fn new(repo_root: PathBuf, config: Config, current_user: Option<String>) -> Self {
        Self {
            repo_root,
            config,
            thread_cache: RwLock::new(HashMap::new()),
            users: RwLock::new(Vec::new()),
            current_user,
        }
    }
}
```

- [ ] **Step 2: 修改 main.rs — 启动时读取 me.json**

在 `crates/gitim-daemon/src/main.rs` 中，在创建 AppState 之前增加 `me.json` 读取逻辑：

```rust
// Read identity from .gitim/me.json (written by CLI onboard)
let me_path = repo_root.join(".gitim").join("me.json");
let current_user: Option<String> = if me_path.exists() {
    let me_content = std::fs::read_to_string(&me_path)?;
    let me_json: serde_json::Value = serde_json::from_str(&me_content)?;
    me_json.get("handler").and_then(|v| v.as_str()).map(|s| s.to_string())
} else {
    tracing::warn!("no .gitim/me.json found, running without identity");
    None
};

if let Some(ref user) = current_user {
    tracing::info!("daemon identity: @{}", user);
}
```

更新 `AppState::new` 调用，传入 `current_user`。

- [ ] **Step 3: 修复所有编译错误**

`AppState::new` 签名变了，需要更新 main.rs 中的调用。确保 `cargo build` 通过。

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo build 2>&1`

Expected: 编译通过（可能有 warning）。

- [ ] **Step 4: 运行全部测试确认无回归**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo test 2>&1`

Expected: 所有测试通过。

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add crates/gitim-daemon/src/state.rs crates/gitim-daemon/src/main.rs
git commit -m "feat(daemon): read identity from me.json on startup"
```

---

### Task 3: Daemon API — register_user

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers.rs`
- [ ] **Step 1: 扩展 Request 枚举**

在 `crates/gitim-daemon/src/api.rs` 的 `Request` 枚举中增加：

```rust
#[serde(rename = "register_user")]
RegisterUser {
    handler: String,
    display_name: String,
    #[serde(default = "default_role")]
    role: String,
    #[serde(default = "default_introduction")]
    introduction: String,
},
```

在文件底部增加默认值函数：

```rust
fn default_role() -> String {
    "member".to_string()
}

fn default_introduction() -> String {
    "GitIM user".to_string()
}
```

- [ ] **Step 2: 实现 handle_register_user**

在 `crates/gitim-daemon/src/handlers.rs` 中：

1. 在 `handle_request` 的 match 中增加分支：

```rust
Request::RegisterUser { handler, display_name, role, introduction } => {
    handle_register_user(state, handler, display_name, role, introduction).await
}
```

2. 实现 handler 函数：

```rust
async fn handle_register_user(
    state: SharedState,
    handler: String,
    display_name: String,
    role: String,
    introduction: String,
) -> Response {
    // Validate handler format (Handler::new takes &str)
    if let Err(e) = gitim_core::types::Handler::new(&handler) {
        return Response::error(format!("invalid handler: {}", e));
    }

    let meta_path = state.repo_root.join("users").join(format!("{}.meta.json", handler));

    // If already exists, return success with exists=true
    if meta_path.exists() {
        return Response::success(serde_json::json!({
            "handler": handler,
            "exists": true
        }));
    }

    // Create meta file
    let meta = serde_json::json!({
        "display_name": display_name,
        "role": role,
        "introduction": introduction
    });
    let meta_str = serde_json::to_string_pretty(&meta).unwrap();

    if let Err(e) = std::fs::write(&meta_path, &meta_str) {
        return Response::error(format!("failed to write user meta: {}", e));
    }

    // Add to users list
    {
        let mut users = state.users.write().await;
        if !users.contains(&handler) {
            users.push(handler.clone());
            users.sort();
        }
    }

    // Git add + commit (best effort, sync loop will push)
    let repo = &state.repo_root;
    let _ = std::process::Command::new("git")
        .args(["add", &format!("users/{}.meta.json", handler)])
        .current_dir(repo)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", &format!("feat: register user @{}", handler)])
        .current_dir(repo)
        .output();

    Response::success(serde_json::json!({
        "handler": handler,
        "exists": false
    }))
}
```

- [ ] **Step 3: 确认编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo build 2>&1`

Expected: 编译通过。

- [ ] **Step 4: 运行全部测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo test 2>&1`

Expected: 所有测试通过。

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add crates/gitim-daemon/src/api.rs crates/gitim-daemon/src/handlers.rs
git commit -m "feat(daemon): add register_user API endpoint"
```

---

### Task 4: Daemon API — stop

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1: 扩展 Request 枚举**

在 `crates/gitim-daemon/src/api.rs` 的 `Request` 枚举中增加：

```rust
#[serde(rename = "stop")]
Stop,
```

- [ ] **Step 2: 实现 handle_stop**

在 `crates/gitim-daemon/src/handlers.rs` 中：

1. 在 `handle_request` 的 match 中增加分支：

```rust
Request::Stop => handle_stop(state).await,
```

2. 实现 handler 函数：

```rust
async fn handle_stop(state: SharedState) -> Response {
    let lifecycle = crate::lifecycle::DaemonLifecycle::new(&state.repo_root);
    lifecycle.cleanup();
    tracing::info!("daemon stopping via API request");

    // Spawn a delayed exit so the response can be sent first
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::process::exit(0);
    });

    Response::success(serde_json::json!({ "status": "stopping" }))
}
```

- [ ] **Step 3: 确认编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo build 2>&1`

- [ ] **Step 4: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add crates/gitim-daemon/src/api.rs crates/gitim-daemon/src/handlers.rs
git commit -m "feat(daemon): add stop API endpoint for graceful shutdown"
```

---

### Task 5: Daemon send 身份注入

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1: 修改 Send 变体 — author 改为 Option**

在 `crates/gitim-daemon/src/api.rs` 中，修改 `Send` 变体：

```rust
#[serde(rename = "send")]
Send {
    channel: String,
    body: String,
    #[serde(default)]
    reply_to: Option<u64>,
    #[serde(default)]
    author: Option<String>,
},
```

- [ ] **Step 2: 修改 handle_send — 缺省时用 current_user**

在 `crates/gitim-daemon/src/handlers.rs` 中，修改 `handle_request` 中的 Send 匹配：

```rust
Request::Send { channel, body, reply_to, author } => {
    // Resolve author: explicit > current_user > error
    let resolved_author = match author {
        Some(a) if !a.is_empty() => a,
        _ => match &state.current_user {
            Some(u) => u.clone(),
            None => return Response::error("no author specified and no identity configured".to_string()),
        },
    };
    handle_send(state, channel, body, reply_to, resolved_author).await
}
```

更新 `handle_send` 函数签名，`author` 参数类型从处理 Option 改为直接接收 `String`（因为解析已在上层完成）。检查原有的 `handle_send` 函数签名和内部调用，确保参数一致。

- [ ] **Step 3: 确认编译通过 + 全部测试通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo build && cargo test 2>&1`

- [ ] **Step 4: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add crates/gitim-daemon/src/api.rs crates/gitim-daemon/src/handlers.rs
git commit -m "feat(daemon): make author optional in send, fallback to current_user"
```

---

## Chunk 2: CLI 层

### Task 6: CLI daemon.ts 增强 — stale 清理

**Files:**
- Modify: `cli/src/daemon.ts`

- [ ] **Step 1: 增加 stale 文件清理逻辑**

修改 `cli/src/daemon.ts` 中的 `ensureDaemon` 函数，在检查 `isDaemonRunning` 为 false 之后、spawn 之前，增加清理逻辑：

```typescript
export async function ensureDaemon(repoRoot: string): Promise<void> {
  const sockPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');

  if (isDaemonRunning(repoRoot)) {
    // Daemon process exists — wait for socket if not ready yet (startup race)
    if (fs.existsSync(sockPath)) return;
    const deadline = Date.now() + DAEMON_STARTUP_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (fs.existsSync(sockPath)) return;
      await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
    }
    throw new Error('daemon is running but socket not ready');
  }

  // Clean up stale runtime files before spawning
  cleanStaleFiles(repoRoot);

  const child = spawn('gitim-daemon', [], {
    cwd: repoRoot,
    detached: true,
    stdio: 'ignore',
  });
  child.unref();

  const deadline = Date.now() + DAEMON_STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    if (fs.existsSync(sockPath)) return;
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }

  throw new Error('daemon failed to start within timeout');
}

function cleanStaleFiles(repoRoot: string): void {
  const runDir = path.join(repoRoot, '.gitim', 'run');
  const files = ['gitim.pid', 'gitim.sock', 'gitim.port', 'gitim.lock'];
  for (const f of files) {
    const p = path.join(runDir, f);
    try { fs.unlinkSync(p); } catch { /* ignore */ }
  }
}
```

- [ ] **Step 2: 确认 CLI 编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding/cli && npx tsc --noEmit`

Expected: 无报错。

- [ ] **Step 3: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add cli/src/daemon.ts
git commit -m "feat(cli): add stale runtime file cleanup in ensureDaemon"
```

---

### Task 7: CLI onboard 命令

**Files:**
- Create: `cli/src/commands/onboard.ts`

- [ ] **Step 1: 创建 onboard.ts — 身份推断函数**

创建 `cli/src/commands/onboard.ts`：

```typescript
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

interface InferredIdentity {
  handler: string;
  displayName: string;
  endpoint: string;
}

function inferIdentity(endpoint: string, endpointUrl: string): InferredIdentity {
  if (endpoint === 'github') {
    try {
      const result = execSync('gh api /user', { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
      const user = JSON.parse(result);
      return {
        handler: user.login.toLowerCase(),
        displayName: user.name || user.login,
        endpoint: 'github',
      };
    } catch {
      console.error('Error: GitHub 认证不可用');
      console.error('  → 请运行 `gh auth login` 配置认证');
      process.exit(1);
    }
  } else if (endpoint === 'gitea') {
    const token = process.env.GITEA_TOKEN;
    if (!token) {
      console.error('Error: GITEA_TOKEN 环境变量未设置');
      console.error('  → 请设置 GITEA_TOKEN 环境变量');
      process.exit(1);
    }
    try {
      const result = execSync(
        `curl -sf -H "Authorization: token ${token}" ${endpointUrl}/api/v1/user`,
        { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] }
      );
      const user = JSON.parse(result);
      return {
        handler: user.login.toLowerCase(),
        displayName: user.full_name || user.login,
        endpoint: 'gitea',
      };
    } catch {
      console.error('Error: Gitea 认证失败');
      console.error(`  → 请确认 GITEA_TOKEN 和服务地址 ${endpointUrl} 正确`);
      process.exit(1);
    }
  }
  console.error(`Error: 不支持的 endpoint: ${endpoint}`);
  process.exit(1);
}
```

- [ ] **Step 2: 添加 repo 初始化函数**

在同一文件中继续添加：

```typescript
function initGitimRepo(
  repoDir: string,
  identity: InferredIdentity,
  endpoint: string,
  endpointUrl: string,
): void {
  // Create directory structure
  fs.mkdirSync(path.join(repoDir, '.gitim'), { recursive: true });
  fs.mkdirSync(path.join(repoDir, 'users'), { recursive: true });
  fs.mkdirSync(path.join(repoDir, 'channels'), { recursive: true });

  // Write config.yaml
  const configContent = [
    'version: 1',
    `endpoint: ${endpoint}`,
    `endpoint_url: "${endpointUrl}"`,
    '',
  ].join('\n');
  fs.writeFileSync(path.join(repoDir, '.gitim', 'config.yaml'), configContent);

  // Update .gitignore
  const gitignorePath = path.join(repoDir, '.gitignore');
  const existing = fs.existsSync(gitignorePath) ? fs.readFileSync(gitignorePath, 'utf-8') : '';
  const additions: string[] = [];
  if (!existing.includes('.gitim/run/')) additions.push('.gitim/run/');
  if (!existing.includes('.gitim/me.json')) additions.push('.gitim/me.json');
  if (additions.length > 0) {
    fs.appendFileSync(gitignorePath, '\n' + additions.join('\n') + '\n');
  }

  // Write me.json
  writeMeJson(repoDir, identity, endpoint);

  // Create user meta
  const userMeta = JSON.stringify({
    display_name: identity.displayName,
    role: 'member',
    introduction: 'GitIM user',
  }, null, 2);
  fs.writeFileSync(path.join(repoDir, 'users', `${identity.handler}.meta.json`), userMeta);

  // Create default general channel
  const now = new Date().toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
  const channelMeta = JSON.stringify({
    display_name: 'General',
    created_by: identity.handler,
    created_at: now,
    introduction: '默认频道',
  }, null, 2);
  fs.writeFileSync(path.join(repoDir, 'channels', 'general.meta.json'), channelMeta);
  fs.writeFileSync(path.join(repoDir, 'channels', 'general.thread'), '');

  // Git commit + push
  execSync('git add -A', { cwd: repoDir, stdio: 'ignore' });
  execSync(`git commit -m "feat: initialize GitIM repo by @${identity.handler}"`, { cwd: repoDir, stdio: 'ignore' });
  try {
    execSync('git push -u origin HEAD', { cwd: repoDir, stdio: 'ignore' });
  } catch {
    // Push may fail if no remote, that's ok for local testing
  }
}

function writeMeJson(repoDir: string, identity: InferredIdentity, endpoint: string): void {
  const now = new Date().toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
  const meJson = JSON.stringify({
    handler: identity.handler,
    endpoint,
    inferred_from: endpoint === 'github' ? 'gh_api' : 'gitea_api',
    inferred_at: now,
  }, null, 2);
  fs.writeFileSync(path.join(repoDir, '.gitim', 'me.json'), meJson);
}
```

- [ ] **Step 3: 添加 onboard 主命令函数**

```typescript
export async function onboardCommand(
  repoName: string | undefined,
  org: string | undefined,
  options: { endpoint: string; url: string; refresh: boolean },
): Promise<void> {
  const endpoint = options.endpoint || 'github';
  const endpointUrl = options.url || '';

  // --refresh mode: re-infer identity in current repo
  if (options.refresh) {
    const cwd = process.cwd();
    if (!fs.existsSync(path.join(cwd, '.gitim', 'config.yaml'))) {
      console.error('不在 GitIM 仓库中，无法 --refresh');
      process.exit(1);
    }
    const identity = inferIdentity(endpoint, endpointUrl);
    writeMeJson(cwd, identity, endpoint);
    console.log(`身份已刷新：@${identity.handler}`);
    return;
  }

  if (!repoName) {
    console.error('请指定仓库名称: gitim onboard <repo_name> [org]');
    process.exit(1);
  }

  // 1. Infer identity
  const identity = inferIdentity(endpoint, endpointUrl);
  console.log(`身份推断：@${identity.handler}`);

  // 2. Validate git is available
  try {
    execSync('git --version', { stdio: 'ignore' });
  } catch {
    console.error('Error: Git 命令不可用');
    console.error('  → 请安装 Git: https://git-scm.com/');
    process.exit(1);
  }

  // 3. Determine repo URL and try clone
  const owner = org || identity.handler;
  let repoUrl: string;
  if (endpoint === 'github') {
    repoUrl = `https://github.com/${owner}/${repoName}.git`;
  } else {
    repoUrl = `${endpointUrl}/${owner}/${repoName}.git`;
  }

  const targetDir = path.resolve(repoName);
  let cloneSucceeded = false;

  try {
    execSync(`git clone ${repoUrl} ${targetDir}`, { stdio: 'ignore' });
    cloneSucceeded = true;
  } catch {
    cloneSucceeded = false;
  }

  if (cloneSucceeded) {
    // Check if it's already a GitIM repo
    const isGitim = fs.existsSync(path.join(targetDir, '.gitim', 'config.yaml'));

    if (isGitim) {
      // 4a: Load flow
      writeMeJson(targetDir, identity, endpoint);
      await ensureDaemon(targetDir);
      const client = new GitimClient(targetDir);
      await client.registerUser(identity.handler, identity.displayName);
      console.log(`已加入 ${repoName}，身份：@${identity.handler}`);
    } else {
      // 4b: Init flow
      initGitimRepo(targetDir, identity, endpoint, endpointUrl);
      await ensureDaemon(targetDir);
      console.log(`已初始化 ${repoName}，身份：@${identity.handler}`);
    }
  } else {
    // 4c: Create flow — repo doesn't exist
    if (endpoint === 'github') {
      const ghRepo = org ? `${org}/${repoName}` : repoName;
      try {
        execSync(`gh repo create ${ghRepo} --private --clone`, {
          cwd: path.dirname(targetDir),
          stdio: 'ignore',
        });
      } catch (e) {
        console.error(`Error: 无法创建仓库 ${ghRepo}`);
        console.error('  → 请确认 Token 有仓库创建权限');
        process.exit(1);
      }
    } else {
      // Gitea create repo via API
      const token = process.env.GITEA_TOKEN!;
      const createUrl = org
        ? `${endpointUrl}/api/v1/orgs/${org}/repos`
        : `${endpointUrl}/api/v1/user/repos`;
      try {
        execSync(
          `curl -sf -X POST -H "Authorization: token ${token}" -H "Content-Type: application/json" -d '{"name":"${repoName}","private":true}' ${createUrl}`,
          { stdio: 'ignore' },
        );
        execSync(`git clone ${repoUrl} ${targetDir}`, { stdio: 'ignore' });
      } catch {
        console.error(`Error: 无法创建 Gitea 仓库 ${repoName}`);
        process.exit(1);
      }
    }
    initGitimRepo(targetDir, identity, endpoint, endpointUrl);
    await ensureDaemon(targetDir);
    console.log(`已创建并初始化 ${repoName}，身份：@${identity.handler}`);
  }
}
```

- [ ] **Step 4: 确认 CLI 编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding/cli && npx tsc --noEmit`

Expected: 可能报错 `registerUser` 不存在于 `GitimClient`（Task 8 会添加）。先确认逻辑无其他语法错误。

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add cli/src/commands/onboard.ts
git commit -m "feat(cli): add onboard command implementation"
```

---

### Task 8: CLI 集成 — client 扩展 + 命令注册

**Files:**
- Modify: `cli/src/client.ts`
- Create: `cli/src/commands/stop.ts`
- Modify: `cli/src/commands/send.ts`
- Modify: `cli/src/commands/dm.ts`
- Modify: `cli/src/index.ts`

- [ ] **Step 1: 扩展 GitimClient — 新增方法**

在 `cli/src/client.ts` 中增加：

```typescript
async registerUser(handler: string, displayName: string, role?: string, introduction?: string): Promise<ApiResponse> {
  return this.request('register_user', {
    handler,
    display_name: displayName,
    role: role ?? 'member',
    introduction: introduction ?? 'GitIM user',
  });
}

async stop(): Promise<ApiResponse> {
  return this.request('stop');
}
```

修改 `send` 方法，`author` 改为可选：

```typescript
async send(channel: string, body: string, author?: string, replyTo?: number): Promise<ApiResponse> {
  return this.request('send', {
    channel,
    body,
    author: author ?? null,
    reply_to: replyTo ?? null,
  });
}
```

- [ ] **Step 2: 创建 stop.ts 命令**

创建 `cli/src/commands/stop.ts`：

```typescript
import { findRepoRoot, isDaemonRunning } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function stopCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  if (!isDaemonRunning(repoRoot)) {
    console.log('Daemon is not running.');
    return;
  }

  const client = new GitimClient(repoRoot);
  try {
    await client.stop();
    console.log('Daemon stopped.');
  } catch {
    console.log('Daemon stopped.');
  }
}
```

- [ ] **Step 3: 修改 send.ts — author 改为可选**

在 `cli/src/commands/send.ts` 中，将 `options.author` 的使用改为可选传递：

```typescript
export async function sendCommand(
  channel: string,
  body: string,
  options: { author?: string; replyTo?: string },
): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const replyTo = options.replyTo ? parseInt(options.replyTo, 10) : undefined;
  const res = await client.send(channel, body, options.author, replyTo);

  if (res.ok) {
    console.log('Message sent.');
  } else {
    console.error('Error:', res.error);
  }
}
```

- [ ] **Step 4: 修改 dm.ts — author 改为可选**

在 `cli/src/commands/dm.ts` 中，`dmSendCommand` 和 `dmReadCommand` 的 `author` 改为可选。对于 `dmSendCommand`，当 author 不传时，从 `.gitim/me.json` 读取 handler 来构造 DM channel 名：

```typescript
import fs from 'node:fs';
import path from 'node:path';

function resolveAuthor(repoRoot: string, explicit?: string): string {
  if (explicit) return explicit;
  const mePath = path.join(repoRoot, '.gitim', 'me.json');
  if (fs.existsSync(mePath)) {
    const me = JSON.parse(fs.readFileSync(mePath, 'utf-8'));
    return me.handler;
  }
  console.error('Error: 未配置身份，请先运行 gitim onboard');
  process.exit(1);
}
```

使用 `resolveAuthor(repoRoot, options.author)` 替代直接使用 `options.author`。

- [ ] **Step 5: 修改 index.ts — 注册新命令，废弃 init**

在 `cli/src/index.ts` 中：

1. 移除 `init` 命令注册
2. 新增 import：

```typescript
import { onboardCommand } from './commands/onboard.js';
import { stopCommand } from './commands/stop.js';
```

3. 注册 onboard 命令：

```typescript
program
  .command('onboard [repo_name] [org]')
  .description('加入或创建 GitIM 仓库')
  .option('-e, --endpoint <type>', 'endpoint 类型: github 或 gitea', 'github')
  .option('-u, --url <url>', 'Gitea 服务地址')
  .option('--refresh', '重新推断身份')
  .action(async (repoName, org, options) => {
    await onboardCommand(repoName, org, options);
  });
```

4. 注册 stop 命令：

```typescript
program
  .command('stop')
  .description('停止当前仓库的 daemon')
  .action(async () => {
    await stopCommand();
  });
```

5. 修改 `send` 命令的 `-a, --author` 为可选（删除 `<required>` 语法）：

```typescript
.option('-a, --author <handler>', '作者 handler（可选，默认使用 onboard 身份）')
```

6. 修改 `dm send` 和 `dm read` 的 `-a, --author` 同理。

7. 删除 `cli/src/commands/init.ts`（onboard.ts 已内联了初始化逻辑）。移除 `index.ts` 中对 `initRepo` 的 import。

- [ ] **Step 6: 确认 CLI 编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding/cli && npx tsc --noEmit`

Expected: 无报错。

- [ ] **Step 7: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git rm cli/src/commands/init.ts
git add cli/src/client.ts cli/src/commands/stop.ts cli/src/commands/send.ts cli/src/commands/dm.ts cli/src/index.ts
git commit -m "feat(cli): integrate onboard/stop commands, make author optional"
```

---

## Chunk 3: E2E 测试

### Task 9: 端到端集成测试

**Files:**
- Modify: `tests/e2e_test.sh`

- [ ] **Step 1: 更新 e2e 测试脚本**

重写 `tests/e2e_test.sh` 以适配新 API（author 可选、me.json 身份注入）。使用 `nc -U`（macOS 原生支持），替换原来的 `socat`（环境中未安装）：

```bash
#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DAEMON_BIN="$REPO_ROOT/target/debug/gitim-daemon"

# Build daemon
echo "=== Building daemon ==="
cargo build --bin gitim-daemon --manifest-path "$REPO_ROOT/Cargo.toml"

# Create temp repo
TMPDIR=$(mktemp -d)
trap 'kill $(cat "$TMPDIR/.gitim/run/gitim.pid" 2>/dev/null) 2>/dev/null; rm -rf "$TMPDIR"' EXIT

cd "$TMPDIR"
git init
git config user.email "test@test.com"
git config user.name "Test"

# Setup GitIM structure
mkdir -p .gitim users channels
echo "version: 1" > .gitim/config.yaml

# Write me.json (simulating onboard)
cat > .gitim/me.json <<'MEJSON'
{"handler":"tester","endpoint":"github","inferred_from":"test","inferred_at":"20260317T120000Z"}
MEJSON

# Create user
cat > users/tester.meta.json <<'USERMETA'
{"display_name":"Tester","role":"dev","introduction":"hi"}
USERMETA

# Create channel
cat > channels/general.meta.json <<'CHANMETA'
{"display_name":"General","created_by":"tester","created_at":"20260317T120000Z","introduction":"test channel"}
CHANMETA
touch channels/general.thread

# .gitignore
echo -e ".gitim/run/\n.gitim/me.json" > .gitignore

git add -A && git commit -m "init" --quiet

# Start daemon
echo "=== Starting daemon ==="
PATH="$(dirname "$DAEMON_BIN"):$PATH"
gitim-daemon &
DAEMON_PID=$!

# Wait for socket
SOCK="$TMPDIR/.gitim/run/gitim.sock"
for i in $(seq 1 50); do
  [ -S "$SOCK" ] && break
  sleep 0.1
done
[ -S "$SOCK" ] || { echo "FAIL: daemon socket not ready"; exit 1; }

echo "=== Running tests ==="

# Test: status
RES=$(echo '{"method":"status"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: status"; exit 1; }
echo "PASS: status"

# Test: send WITHOUT author (should use me.json identity)
RES=$(echo '{"method":"send","channel":"general","body":"hello no author"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: send without author"; exit 1; }
echo "PASS: send without author"

# Test: send WITH explicit author
RES=$(echo '{"method":"send","channel":"general","body":"hello with author","author":"tester"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: send with author"; exit 1; }
echo "PASS: send with author"

# Test: read — verify both messages have @tester
RES=$(echo '{"method":"read","channel":"general"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: read"; exit 1; }
echo "$RES" | grep -q '@tester' || { echo "FAIL: read author check"; exit 1; }
echo "PASS: read with identity"

# Test: list channels
RES=$(echo '{"method":"channels"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"general"' || { echo "FAIL: channels"; exit 1; }
echo "PASS: channels"

# Test: register_user (new user)
RES=$(echo '{"method":"register_user","handler":"newbie","display_name":"New User"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"ok":true' || { echo "FAIL: register_user"; exit 1; }
[ -f "$TMPDIR/users/newbie.meta.json" ] || { echo "FAIL: newbie meta not created"; exit 1; }
echo "PASS: register_user"

# Test: register_user (existing user, should succeed with exists=true)
RES=$(echo '{"method":"register_user","handler":"tester","display_name":"Tester"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"exists":true' || { echo "FAIL: register_user existing"; exit 1; }
echo "PASS: register_user existing"

# Test: stop
RES=$(echo '{"method":"stop"}' | nc -U "$SOCK")
echo "$RES" | grep -q '"stopping"' || { echo "FAIL: stop"; exit 1; }
sleep 0.5
kill -0 $DAEMON_PID 2>/dev/null && { echo "FAIL: daemon still running after stop"; exit 1; }
echo "PASS: stop"

echo ""
echo "=== All tests passed ==="
```

- [ ] **Step 2: 运行 e2e 测试**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && bash tests/e2e_test.sh`

Expected: 所有测试通过。

- [ ] **Step 3: 修复测试失败的问题**

如果有测试失败，根据错误信息修复代码，重新运行直到全部通过。

- [ ] **Step 4: 运行全部 Rust 单元测试确认无回归**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding && cargo test`

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/ux-onboarding
git add tests/e2e_test.sh
git commit -m "test: update e2e test for identity injection and new API endpoints"
```
