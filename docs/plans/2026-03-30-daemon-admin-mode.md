# Daemon Admin Mode 实现计划

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 daemon 添加 admin 模式——通过 onboard 时传入 `--admin` flag，让 poll 绕过 channel membership 和 DM visibility 过滤，实现全局审查者视角。

**Architecture:** Admin 是纯运行时状态，存在 `AppState.is_admin` 里，onboard 时设置。Poll handler 检查该标志决定是否跳过过滤。Send 等写操作保持原有权限检查不变。

**Tech Stack:** Rust (gitim-daemon) + TypeScript (CLI) + React (WebUI)

---

## Chunk 1: Daemon 端 admin 模式

### Task 1: api.rs — Onboard request 加 admin 字段

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs:71-75`

- [ ] **Step 1: 修改 Onboard variant，添加 admin 字段**

```rust
#[serde(rename = "onboard")]
Onboard {
    git_server: String,
    auth: serde_json::Value,
    #[serde(default)]
    admin: bool,
},
```

- [ ] **Step 2: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译失败，因为 `handle_onboard` 的签名还没跟上（在 Task 3 修复）。先确认只有预期的编译错误。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-daemon/src/api.rs
git commit -m "feat(api): add admin field to Onboard request"
```

---

### Task 2: state.rs — AppState 加 is_admin 字段

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs:25-58`

- [ ] **Step 1: 在 AppState struct 添加 is_admin 字段**

在 `sync_started` 字段后面添加：

```rust
pub is_admin: AtomicBool,
```

- [ ] **Step 2: 在 constructor 中初始化 is_admin**

在 `new()` 方法的 `Self { ... }` 块中，`sync_started` 之后添加：

```rust
is_admin: AtomicBool::new(false),
```

- [ ] **Step 3: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译仍失败（handle_onboard 签名待修复），但不应有 state.rs 相关的新错误。

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-daemon/src/state.rs
git commit -m "feat(state): add is_admin field to AppState"
```

---

### Task 3: onboard.rs — 处理 admin flag 并写入 AppState

**Files:**
- Modify: `crates/gitim-daemon/src/onboard.rs:11-14`

- [ ] **Step 1: 修改 handle_onboard 签名，接收 admin 参数**

```rust
pub async fn handle_onboard(
    state: SharedState,
    git_server: String,
    auth: serde_json::Value,
    admin: bool,
) -> Response {
```

- [ ] **Step 2: 在 handle_onboard 函数体中，Step A 之前写入 is_admin**

在 `// --- Step A: Infer identity ---` 之前添加：

```rust
// --- Set admin mode ---
state.is_admin.store(admin, std::sync::atomic::Ordering::SeqCst);
if admin {
    info!("onboard: admin mode enabled");
}
```

- [ ] **Step 3: 修改 handlers.rs 中的 dispatch 调用，传递 admin 参数**

找到 `handlers.rs` 中调用 `handle_onboard` 的地方（在 `handle_request` 匹配 `Request::Onboard` 的分支），修改为：

```rust
Request::Onboard { git_server, auth, admin } => {
    crate::onboard::handle_onboard(state, git_server, auth, admin).await
}
```

- [ ] **Step 4: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译通过，无错误。

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-daemon/src/onboard.rs crates/gitim-daemon/src/handlers.rs
git commit -m "feat(onboard): pass admin flag to AppState"
```

---

### Task 4: handlers.rs — poll handler 跳过过滤

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs:596-680`

- [ ] **Step 1: 在 poll handler 中读取 is_admin 标志**

在 `handle_poll` 函数中，`let current_user_snapshot = ...` 那行（596 行）之后添加：

```rust
let is_admin = state.is_admin.load(std::sync::atomic::Ordering::SeqCst);
```

- [ ] **Step 2: 修改 channel membership cache 构建逻辑**

将 Step 1（membership cache 构建，598-630 行）用 admin 判断包裹——admin 模式下跳过整个 cache 构建：

```rust
// Step 1: Build channel membership cache (skip for admin)
let mut channel_membership: HashMap<String, bool> = HashMap::new();
if !is_admin {
    for (path, _) in &diff {
        // ... 原有逻辑不变 ...
    }
}
```

- [ ] **Step 3: 修改 channel membership 过滤**

将 660-665 行的 channel 过滤加 admin 条件：

```rust
// Channel membership filter (skip for admin)
if kind == "channel" && !is_admin {
    if !channel_membership.get(&channel).copied().unwrap_or(true) {
        continue;
    }
}
```

- [ ] **Step 4: 修改 DM visibility 过滤**

将 667-680 行的 DM 过滤加 admin 条件：

```rust
// DM visibility filter — skip DMs not involving current user (skip for admin)
if kind == "dm" && !is_admin {
    if let Some(stem) = path_str
        .strip_prefix("dm/")
        .and_then(|s| s.strip_suffix(".thread"))
    {
        if let Some((a, b)) = parse_dm_filename(stem) {
            match &current_user_snapshot {
                Some(me) if me == a || me == b => { /* allowed */ }
                _ => continue,
            }
        }
    }
}
```

- [ ] **Step 5: 同样修改 channel_meta 过滤（641 行）**

将 meta change 的 membership 检查也加 admin 条件：

```rust
} else if let Some(ch_name) = name.strip_suffix(".meta.json") {
    // Meta change — only push if user is (now) a member (skip for admin)
    if !is_admin && !channel_membership.get(ch_name).copied().unwrap_or(true) {
        continue;
    }
```

- [ ] **Step 6: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-daemon/src/handlers.rs
git commit -m "feat(poll): skip channel/dm filters in admin mode"
```

---

## Chunk 2: CLI 端 --admin 参数

### Task 5: cli/onboard.ts — 添加 --admin 命令行参数

**Files:**
- Modify: `cli/src/commands/onboard.ts:10-21,77-78`
- Modify: `cli/src/client.ts:77-79`

- [ ] **Step 1: OnboardOptions 接口添加 admin 字段**

在 `cli/src/commands/onboard.ts` 的 `OnboardOptions` 接口中添加：

```typescript
interface OnboardOptions {
  gitServer: GitServer;
  token?: string;
  handler?: string;
  displayName?: string;
  url?: string;
  refresh?: boolean;
  debugHttp?: boolean;
  withWebui?: boolean;
  webuiPort?: string;
  webuiDev?: boolean;
  admin?: boolean;
}
```

- [ ] **Step 2: GitimClient.onboard 方法透传 admin 参数**

修改 `cli/src/client.ts` 的 `onboard` 方法：

```typescript
async onboard(gitServer: string, auth: Record<string, string>, admin?: boolean): Promise<ApiResponse> {
  return this.request('onboard', { git_server: gitServer, auth, admin: admin ?? false });
}
```

- [ ] **Step 3: onboardCommand 中传递 admin 参数**

修改 `onboard.ts` 中两处调用 `client.onboard` 的地方：

refresh 模式（约 199 行）：
```typescript
const res = await client.onboard(gitServer, auth, options.admin);
```

正常模式（约 236 行）：
```typescript
const res = await client.onboard(gitServer, auth, options.admin);
```

- [ ] **Step 4: 在成功输出中提示 admin 模式**

修改正常模式的输出（约 245 行）：

```typescript
const adminTag = options.admin ? ' [ADMIN]' : '';
console.log(`成功 ${created}：@${handler}${adminTag} @ ${repoName}`);
```

修改 refresh 模式的输出（约 204 行）：

```typescript
const adminTag = options.admin ? ' [ADMIN]' : '';
console.log(`身份已刷新：@${res.data?.handler}${adminTag}`);
```

- [ ] **Step 5: 确认 CLI 框架中 --admin 参数的注册**

检查 CLI 入口文件（通常在 `cli/src/index.ts` 或 `cli/src/main.ts`），找到 onboard 命令的 option 注册，添加：

```typescript
.option('--admin', 'admin 模式：poll 返回所有内容（审查视角）')
```

- [ ] **Step 6: 编译验证**

Run: `cd cli && npm run build 2>&1 | tail -10`
Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add cli/src/commands/onboard.ts cli/src/client.ts cli/src/index.ts
git commit -m "feat(cli): add --admin flag to onboard command"
```

---

## Chunk 3: WebUI DM 显示名 fallback

### Task 6: webui/Sidebar.tsx — admin 看别人 DM 时显示双方名字

**Files:**
- Modify: `webui/src/components/Sidebar.tsx:32-39`

- [ ] **Step 1: 修改 dmDisplayName 函数**

```typescript
const dmDisplayName = (name: string) => {
  const parts = name.split('--');
  const isSelf = parts.every((p) => p === currentUser);
  if (isSelf) return `${currentUser} (我)`;
  // 自己是其中一方：显示对方名字；都不是自己（admin 视角）：显示双方
  if (parts.includes(currentUser)) {
    return parts.find((p) => p !== currentUser) ?? name;
  }
  return parts.join(' ↔ ');
};
```

- [ ] **Step 2: 本地验证**

Run: `cd webui && npm run build 2>&1 | tail -5`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add webui/src/components/Sidebar.tsx
git commit -m "feat(webui): show both handlers for DMs not involving current user"
```

---

## Chunk 4: 端到端手动验证

### Task 7: 端到端验证

- [ ] **Step 1: 正常模式验证**

启动不带 `--admin` 的 daemon，确认 poll 仍然只返回自己相关的内容：

```bash
gitim onboard test-repo --git-server git --handler alice --display-name Alice --with-webui
```

- [ ] **Step 2: Admin 模式验证**

在另一个终端以 admin 模式启动：

```bash
gitim onboard test-repo --git-server git --handler admin-user --display-name Admin --admin --refresh --with-webui
```

验证：
1. 频道列表显示所有频道（包括未加入的）
2. DM 列表显示所有 DM（包括不属于自己的）
3. 不属于自己的 DM 在侧栏显示为 `alice ↔ bob` 格式
4. 能正常阅读所有 channel 和 DM 的消息
5. Send 仍然遵守原有权限（只能在加入的 channel 发消息）
