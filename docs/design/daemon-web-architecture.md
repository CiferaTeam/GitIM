# GitIM daemon-web — 移动端人类聊天客户端

> 在手机浏览器中运行 GitIM 的人类聊天功能，无需本地 daemon 或 git CLI。不是桌面端的完整复刻——聚焦消息收发，不含 agent 管理、看板、归档等桌面端功能。

## 动机

当前 GitIM 的数据流依赖本地 OS 原语：

```
webui-v2 → HTTP → gitim-runtime → Unix Socket → gitim-daemon → git CLI → remote
```

daemon 需要 Unix socket、文件系统、git CLI、SQLite、file watcher——这些在浏览器沙箱中均不可用。手机端缺少整条右侧链路。

目标：**纯客户端方案**，在手机浏览器中运行 GitIM 的人类聊天功能，无需自建业务服务器（仍需 CORS proxy，见下文信任模型）。

## 设计原则

- **并行实现，不动现有架构**：桌面端的 Rust daemon + runtime 保持不变，新增浏览器端实现
- **共享核心逻辑**：解析、验证、冲突解决编译为 WASM，两端跑同一份代码
- **共享 UI 层**：webui-v2 组件层完全复用，仅切换 backend adapter
- **共享数据格式**：`.thread` 文件格式、`meta.yaml` 结构、git remote 完全一致

## 架构总览

```
┌────────────── webui-v2 (React, 两端共享) ──────────────┐
│                                                         │
│  client.ts → Backend interface (18 async 函数)          │
│       ↓                                                  │
│  ┌──────────────────┬────────────────────┐              │
│  │   HttpBackend    │   LocalBackend     │              │
│  │  (桌面端，现有)   │  (手机端，新增)     │              │
│  └────────┬─────────┴─────────┬──────────┘              │
└───────────┼───────────────────┼─────────────────────────┘
            ↓                   ↓
     gitim-runtime       Web Worker
     + daemon (Rust)     + daemon-web (TS)
     + git CLI           + isomorphic-git
     + std::fs           + lightning-fs (IndexedDB)
            ↓                   ↓
          同一个 Git Remote (GitHub / Gitea / GitLab)
```

两端操作同一个 remote repo、同一套 `.thread` 文件格式，消息互通。

## WASM 共享层

### 可行性结论

gitim-core 的全部公开函数均为纯函数（无 I/O），依赖链 100% WASM 兼容：

| 依赖 | WASM 兼容性 |
|------|------------|
| serde / serde_json / serde_yaml | 完全兼容 |
| regex | 完全兼容 |
| thiserror | 完全兼容 |
| chrono | 需 `default-features = false, features = ["serde"]` |

gitim-sync 中 renumber 和 conflict 的纯逻辑函数同样可以导出。

### 导出的 WASM API

**来自 gitim-core：**

| 函数 | 用途 |
|------|------|
| `parse_thread(text) -> ThreadFile` | 解析 `.thread` 文件 |
| `format_message(ln, pt, author, ts, body) -> String` | 格式化消息行 |
| `format_event(ln, author, ts, event_type, meta) -> String` | 格式化事件行 |
| `validate_append(existing, new_lines, users, senders) -> Result` | 写入合规校验 |
| `validate_join / validate_leave` | 成员变更校验 |
| `validate_channel_meta / validate_user_meta` | 元数据校验 |
| `extract_mentions(body) -> Vec<Handler>` | 提取 @ 提及 |
| `extract_links(body) -> Vec<Link>` | 提取链接 |
| `dm_filename(a, b) -> String` | DM 文件名生成 |

**来自 gitim-sync（纯逻辑部分）：**

| 函数 | 用途 |
|------|------|
| `renumber_batch(batch, max_existing) -> String` | 冲突时行号重编 + P 引用重映射 |
| `merge_channel_meta(local, remote) -> ChannelMeta` | 元数据合并（成员取并集） |
| `build_rebase_commit_msg(mappings, additions) -> String` | 生成 rebase commit message |

### 实现方式

使用 `wasm-pack build --target web` 编译，通过 `wasm-bindgen` + `serde-wasm-bindgen` 导出 JS binding。TS 类型定义手写（复用现有 `types.ts` 风格）。

**WASM 导出需要 wrapper 层**：gitim-core 的内部类型（`ThreadFile`, `ThreadEntry`, `Link` 等）不直接 derive `Serialize`/`Deserialize`，不能原样穿越 WASM 边界。需要新建一个 `gitim-wasm` crate，写 `#[wasm_bindgen]` wrapper 函数，负责：
- 入参：接收 `&str` / `JsValue`
- 调用 gitim-core 纯函数
- 出参：通过 serde-wasm-bindgen 或手动转换返回 JS-compatible 对象
- 为需要跨边界的类型补 serde derives（或在 wrapper 层做 DTO 转换）

WASM 产物集成到 webui-v2：手动 `wasm-pack build`，在 `package.json` 中以 `file:` 依赖引用 `pkg/` 目录。**注意**：`file:` 依赖要求 fresh checkout 时先跑 `wasm-pack build`，否则 `npm install` 会失败。需要在 webui-v2 的 `package.json` 加 `prebuild` 脚本，或在 README 说明构建顺序。Worker 内加载 WASM 需要处理 async init 和 asset URL（Vite 的 `?url` import 或 `new URL(..., import.meta.url)` 模式）。

gitim-core 的改动：
- `Cargo.toml` 确认 chrono 是否实际使用——如果未使用，移除依赖（比 per-crate override 更干净）
- 如果 chrono 仍需保留：per-crate override `chrono = { workspace = true, default-features = false, features = ["serde"] }`

gitim-sync 的改动：
- `#[cfg(not(target_arch = "wasm32"))]` 门控 I/O 模块（git.rs、watcher.rs、sync_loop.rs）
- **Cargo.toml 需要 target-specific deps**——仅 cfg gate 源码不够，`tokio`、`notify`、`rand` 在 WASM 编译时仍会被拉入。需要：
  ```toml
  [target.'cfg(not(target_arch = "wasm32"))'.dependencies]
  tokio = { workspace = true }
  notify = "7"
  ```
- `conflict.rs` 中 `resolve_content()` 拆为纯函数 `resolve_content_from_remote(local_additions: &HashMap<PathBuf, String>, remote_contents: &HashMap<PathBuf, String>)` + I/O wrapper。纯函数接收已读取的远端内容，不碰文件系统。

新增 `gitim-wasm` crate（预计工作量最大的一块）：
- 依赖 gitim-core + gitim-sync（仅纯函数部分）
- `#[wasm_bindgen]` wrapper 函数 + serde 转换
- `wasm-pack build --target web` 入口

## daemon-web（TS 平台层）

WASM 覆盖解析和校验，剩余的平台相关逻辑用 TypeScript 实现，运行在 Web Worker 中。

### 模块划分

```
webui-v2/src/daemon-web/
├── worker.ts        # Web Worker 入口，主线程 RPC 分发
├── git.ts           # isomorphic-git 封装（clone/fetch/commit/push）
├── sync.ts          # 同步循环：push-first + 冲突合并
├── handlers.ts      # API 实现（send/read/poll/thread/channels/users/search）
├── state.ts         # 内存状态：channels, users, thread cache, event queue
└── storage.ts       # OPFS 文件系统封装
```

### 各模块职责

**worker.ts** — Web Worker 入口

主线程通过 `postMessage` 发送请求，Worker 内分发到 handlers。请求/响应格式与 Rust daemon 的 `api.rs` 对齐。

**git.ts** — isomorphic-git 操作

封装以下 git 操作（对应 Rust 端 `gitim-sync/src/git.rs`）：

| 操作 | isomorphic-git API |
|------|-------------------|
| clone | `git.clone()` |
| fetch | `git.fetch()` |
| commit | `git.add() + git.commit()` |
| push | `git.push()` |
| rev_parse HEAD | `git.resolveRef({ ref: 'HEAD' })` |
| has_unpushed | `git.log()` 对比本地与 remote HEAD |
| diff_range | `git.walk()` 遍历两棵 tree 的差异 |

不需要的操作：`pull_rebase`（自定义合并策略替代）、`mv`（手机端不做文件重命名）、`discard_unpushed`（自定义 reset 逻辑）。

**sync.ts** — 同步循环

不用 git rebase。利用对 `.thread` 格式的完全掌控，实现更简单的合并策略：

```
定时触发（或 visibilitychange 唤醒）
  → fetch origin
  → 检测本地是否有未 push 的 commit
  ├─ 无冲突 → fast-forward merge → done
  └─ 有冲突 →
      1. 提取本地 additions（diff origin/main..HEAD，注意方向）
      2. reset 本地到 remote HEAD（isomorphic-git: 更新 ref + checkout worktree + 清 index）
      3. 调 WASM renumber_batch()：从 remote max_line + 1 开始重编号
      4. 调 WASM merge_channel_meta()：成员取并集
      5. 写入合并后的文件
      6. commit（调 WASM build_rebase_commit_msg() 生成 message）
      7. **重置 poll cursor 到新 HEAD**（旧 cursor 指向的 commit 已被重写，不再有效）
      8. **通知 UI 层触发全量消息重加载**（而非增量 diff，避免重复/丢失消息）
      9. push
      10. push 失败 → 回到 fetch 重试（最多 3 次）
```

**cursor 重置策略**：sync 重写 commit 历史后（步骤 2-6），localStorage 中保存的 commit hash 指向已不存在的 commit。必须在步骤 7 重置 cursor，并在步骤 8 触发全量重加载。这是与 Rust 端的关键差异——Rust 端 rebase 保持线性历史，cursor 始终有效。

退避策略与 Rust 端一致：指数退避 + 1/3 jitter。

**handlers.ts** — API 实现

实现 webui-v2 实际调用的 API 子集（手机端只服务人类节点，不需要 agent 管理）：

| API | 说明 |
|-----|------|
| `me()` | 从本地 config 读取当前用户身份 |
| `poll(since?)` | 基于 commit hash 返回增量变更 |
| `channels()` | 扫描 channels/ 目录，解析 meta.yaml |
| `read(channel, limit?)` | 解析 .thread 文件（调 WASM parse_thread） |
| `send(channel, body, reply_to?)` | 格式化消息（调 WASM format_message）→ 追加文件 → commit |
| `thread(channel, line)` | 从解析结果中提取线程树（注意：需支持 DM 路径，daemon 源码有 bug） |
| `users()` | 扫描 users/ 目录 |
| `joinChannel(channel, targets?)` | 加入频道（chat UI 中的 join banner 依赖此 API） |

不实现的 API：`search`（MVP 不含搜索）、`onboard`（手机端有独立的初始化流程）、agent 相关的 6 个端点、`reindex`、`subscribe`、`stop`。

**state.ts** — 内存状态

```typescript
interface DaemonWebState {
  repoDir: string              // OPFS 中的仓库路径
  me: { handler: string; display_name: string }
  channels: Map<string, ChannelMeta>
  users: Map<string, UserMeta>
  threadCache: Map<string, ThreadFile>  // channel -> parsed thread
  headCommit: string           // 当前 HEAD commit hash
  eventQueue: Event[]          // 待消费的事件队列
  syncStatus: 'idle' | 'syncing' | 'error'
}
```

**storage.ts** — lightning-fs 文件系统

使用 [lightning-fs](https://github.com/nicolo-ribaudo/isomorphic-git-lightning-fs)（IndexedDB backed），这是 isomorphic-git 官方推荐的 fs backend。

- isomorphic-git 原生集成，零适配代码
- 数据持久化在 IndexedDB 中，刷新页面不丢失
- 如果未来性能不够，可以切换到 OPFS adapter（需要自写），但 MVP 不需要

兼容性：IndexedDB 覆盖所有主流手机浏览器。

## webui-v2 改动

### client.ts — backend 选择

当前 `client.ts` 的 18 个函数全部直接调 `fetch(baseUrl() + path)`。改为通过 backend interface 分发：

**注意**：Backend interface 匹配的是 **gitim-runtime 的 HTTP 路由**（`/im/send`, `/im/channels` 等），不是 daemon 的 `api.rs`。因为 webui-v2 现在就是跟 runtime 通信，LocalBackend 要对齐的是 runtime 的响应格式。

```typescript
// backend interface — 匹配 runtime HTTP 路由的响应格式
interface Backend {
  health(): Promise<ApiResponse>
  me(): Promise<ApiResponse>
  poll(since?: string): Promise<ApiResponse>
  channels(): Promise<ApiResponse>
  read(channel: string, limit?: number): Promise<ApiResponse>
  send(channel: string, body: string, author?: string, replyTo?: number): Promise<ApiResponse>
  thread(channel: string, line: number): Promise<ApiResponse>
  users(): Promise<ApiResponse>
  joinChannel(channel: string, targets?: string[]): Promise<ApiResponse>
  // agent 端点仅 HttpBackend 实现，LocalBackend 返回 not_supported
}
```

`HttpBackend`：现有的 fetch 逻辑原样搬入。
`LocalBackend`：通过 `postMessage` 与 Web Worker 通信。app.tsx 中 `listAgents()` 等调用在 local 模式下需跳过（不只是 SSE 禁用，poll 循环里的 agent 刷新也要跳过）。

### use-connection-store.ts — 模式切换

新增 `mode: 'remote' | 'local'` 状态：

- `remote`：连接 gitim-runtime（现有行为）
- `local`：使用 daemon-web（手机端）

初始化流程根据 mode 分支：
- remote 模式：检查 runtime port → `/health` → `/set-workspace`
- local 模式：初始化 Web Worker → clone repo 到 lightning-fs → 启动 sync 循环

### use-agent-activity.ts — 条件禁用

local 模式下不启用 SSE 连接（无 agent 管理），该 hook 直接返回空状态。

### 前端流程适配

local 模式不是简单切换 backend adapter——前端还有 setup-gate、workspace/provider 设置、agent 列表刷新、默认路由跳 `/management` 等逻辑需要适配：

| 组件/流程 | remote 模式 | local 模式 |
|-----------|-------------|------------|
| `setup-gate.tsx` | 检查 runtime 连接 → workspace 设置 | 跳过，直接进入 clone/init 流程 |
| 默认路由 | `/management`（agent 管理） | `/chat`（直接进聊天） |
| agent 列表刷新 | poll 时同步获取 | 禁用 |
| SSE 连接 | 连接 `/agents/events` | 禁用 |
| workspace/provider 设置 | runtime API | 不需要，本地 Worker 直接持有 repo |

local 模式需要一套独立的初始化 gate（输入 remote URL + token + handler → clone → ready），替代现有的 setup-gate。

## CORS 与 Git Remote 访问

浏览器直接通过 HTTP 协议访问 git remote 会被 CORS 策略拦截。isomorphic-git 内置 `corsProxy` 参数：

```typescript
await git.clone({
  fs, http, dir: '/repo',
  url: 'https://github.com/team/im-repo',
  corsProxy: 'https://your-proxy.workers.dev',
  onAuth: () => ({ username: token, password: 'x-oauth-basic' })
})
```

CORS proxy 是 HTTP pass-through。部署方案：

| 方案 | 成本 | 说明 |
|------|------|------|
| Cloudflare Worker | 免费 | 10 行代码，免费 tier 100K req/day |
| 自部署 nginx | 极低 | 纯转发，不需要 daemon 运行环境 |
| 公共 proxy | 免费 | `cors.isomorphic-git.org`，不建议生产使用 |

与"跑一台服务器当 daemon"的本质区别：CORS proxy 不承载任何 GitIM 业务逻辑，可被任何同类服务替换。

### CORS proxy 信任模型

**CORS proxy 能看到什么：** git HTTP smart protocol 的全部流量，包括 Authorization header（含 token）和 packfile 内容（即仓库数据）。它不是"不接触数据"——它是中间人。

**风险评估：**
- 自部署 CORS proxy（Cloudflare Worker / 自有 nginx）：信任边界等同于信任自己的基础设施，可接受
- 公共 CORS proxy：等同于把 token 和仓库内容交给第三方，仅限开发/测试使用
- 如果仓库含敏感数据，**必须**使用自部署 proxy

**与远端 daemon 方案的区别：** CORS proxy 是无状态转发，不运行 GitIM 业务逻辑，不持久化数据，不需要维护。运维负担远低于跑一个 daemon 进程。但信任要求并非为零。

### 认证

手机端没有 GitIM 自己的 token 概念——GitIM 无中心服务器，所有认证都是直接对 git remote。用户需要自行在 git 托管平台生成一个 Personal Access Token，粘贴到初始化页面。isomorphic-git 的 `onAuth` 回调把 token 塞进 HTTP Basic Auth：

```typescript
onAuth: () => ({ username: token, password: 'x-oauth-basic' })
```

**不做 OAuth Device Flow**：需要为 GitHub / Gitea / GitLab 分别注册 OAuth App 并做 endpoint 适配，用户自部署 Gitea 实例还要各自配 OAuth App，分发 client_id 的复杂度不可接受。PAT 是三个平台的最大公约数，一份文档覆盖全部场景，零运维。

#### Token 来源(用户手动生成)

| 平台 | 生成路径 | 最小 scope |
|------|---------|-----------|
| GitHub (Fine-grained，推荐) | Settings → Developer settings → Personal access tokens → Fine-grained tokens | Repository access 锁到目标 repo；Contents R/W + Metadata R |
| GitHub (Classic) | Settings → Developer settings → Tokens (classic) | `repo` |
| Gitea | Settings → Applications → Manage Access Tokens | `write:repository` |
| GitLab | User Settings → Access Tokens | `write_repository` |

推荐 Fine-grained PAT(GitHub)：权限锁死到单个 repo，被盗的影响面最小。这套 token 生成步骤需要一份独立的用户文档并配截图——手机用户不看长文字。

#### Deeplink 辅助

手机端初始化 gate 放一个 "Generate token on GitHub" 按钮，跳转到预填好 name / scope 的生成页：

```
https://github.com/settings/personal-access-tokens/new?name=GitIM%20mobile&...
```

Gitea / GitLab 不提供同类预填 query，只能给静态跳转链接。

#### Token 生命周期

Fine-grained PAT **强制过期**(最长 1 年)，Gitea/GitLab 默认也有过期时间。"重新输入 token" 不是边缘场景，是每个用户一年内必经路径。UI 必须常备此路径：

- **过期前提醒**：初始化时记录 token 过期日，到期前 7 天在 UI 顶栏显示 banner
- **过期后的软着陆**：fetch/push 收到 401 时弹重新输入 dialog，仓库不用重 clone，只换 token
- **"Update access token" 入口常驻**：设置页要有显式按钮，不能只在报错时才出现

#### Token 存储

Token 存储在浏览器 localStorage 或 IndexedDB 中，仅限当前 origin。同 origin 的 XSS 或被攻破的前端依赖都能窃取——参见 "已知限制与风险 → Token 安全" 小节。

## 手机端初始化流程

手机端不走现有的 `gitim onboard`（那是 CLI + daemon 编排）。独立的浏览器内初始化：

```
1. 用户输入：git remote URL + token + handler
   - 支持两种方式：页面内表单手动输入，或通过 URL fragment (#) 传入
   - **禁止使用 URL query parameter 传递 token**（会被记录在浏览器历史、服务器日志、Referrer header 中）
2. Web Worker 启动
3. isomorphic-git clone repo 到 lightning-fs（通过 CORS proxy）
4. 构建本地身份状态：
   - 读取 users/<handler>.meta.yaml 确认用户已注册
   - 构建等价于 .gitim/me.json 的内存状态：{ handler, display_name, guest: false }
   - 手机端不支持 guest 和 admin 模式（始终为普通注册用户）
   - 此身份状态用于 DM 可见性过滤和频道成员检查
5. 读取 channels/ 目录构建 channel 列表（仅显示当前用户为成员的频道）
6. 启动 sync 循环
7. 切换到 ready 状态，UI 渲染
```

前提：用户已在桌面端完成 onboard（用户注册、channel 创建等），手机端是只读注册——读取已有身份，不创建新用户。

### poll 语义差异

local 模式下 `poll(since?)` 的语义与 Rust daemon 不同：

- **Rust daemon**：被动接收 sync loop 推送的变更，poll 是 diff 本地 commit 历史
- **daemon-web**：poll 主动触发 `git fetch`，然后 diff remote 与本地的差异，返回新增消息

即 local 模式的 poll = fetch + diff + 返回增量。每次 poll 都有网络开销，建议 local 模式下 poll 间隔适当拉长（5-10s，而非 remote 模式的 3s）。

**注意**：fetch 只更新 remote refs，不更新 worktree。poll 的正确顺序是：fetch → diff（对比 old HEAD 与 new remote HEAD）→ fast-forward checkout → 更新 cursor。如果先 checkout 再 diff，会得到空 diff。

## Scope

### 包含

- 浏览器内完整的消息收发（send / read / poll / thread）
- 后台 git 同步（push-first + 自定义冲突合并）
- 频道和用户列表
- 离线缓存（IndexedDB 持久化，打开即可看历史消息）

### 不包含

- Agent 管理（手机端只服务人类节点）
- Onboard 创建新用户（需先在桌面端注册）
- Board / Card（看板功能，后续按需扩展）
- 频道创建 / 归档（管理操作留在桌面端）
- 搜索（避免启动全量解析和移动端内存压力，后续按需扩展）

## 已知限制与风险

### 技术层

- **isomorphic-git 不支持 SSH 协议**：只能用 HTTPS + token 认证。如果 remote 仅支持 SSH，手机端无法连接。现有场景（GitHub / Gitea / GitLab）均支持 HTTPS，不构成阻塞。
- **OPFS 存储配额**：浏览器对 origin 的存储有上限（通常 > 1GB），大型仓库可能触及。GitIM 仓库以文本为主，正常使用下远不会达到上限。
- **Service Worker 生命周期**：手机浏览器会积极杀后台进程。sync 循环不能依赖持续运行——采用 `visibilitychange` 事件在页面回到前台时触发同步，配合 `setInterval` 在前台期间定时同步。
- **WASM 包体积**：gitim-core 编译到 WASM 预估 100-200KB（gzip 后 50-80KB），可接受。
- **isomorphic-git 性能**：对大仓库的 clone / fetch 比 native git 慢。GitIM 仓库是增量文本，初始 clone 后增量 fetch 很快。

### 产品层

- **首次 clone 耗时**：仓库越大越慢。考虑支持 `--depth 1` shallow clone 减少初始数据量。**注意**：shallow clone 与 commit-hash poll 和 conflict diff 有冲突——旧 cursor 对应的 commit 可能不存在。如果用 shallow clone，poll 必须用 tree-walk diff 而非 commit diff。
- **双端同步冲突**：用户同时在手机和桌面发消息，两端各自 push 可能产生冲突。现有 renumber 机制可以处理，但用户可能看到消息行号跳变。这与多 agent 场景下的行为一致，不是新问题。
- **Token 安全**：git token 存储在浏览器 localStorage 中。同 origin 的任何 XSS 或被攻破的前端依赖都能窃取写权限 token。建议：使用最小权限 scope 的 token、提供 session-scoped 存储选项（关闭标签页即清除）、设置严格 CSP。
- **冲突合并的边界情况**：
  - 归档复活：桌面端归档/删除频道后，手机端离线期间仍有该频道的待发消息，冲突合并会重新创建该频道文件
  - 踢人恢复：桌面端移除某成员后，手机端用过期的本地 meta 合并，`merge_channel_meta()` 的成员取并集会把被踢的人加回来
  - 过期成员发送：用户被移出频道后，手机端用缓存的旧成员列表仍可通过 `validate_append()`，push 后消息留存
  - 这些需要在 push 前重新验证远端状态（fetch 后对比），或接受最终一致性
- **多标签页并发**：两个标签页可能同时实例化 Worker，竞争同一个 IndexedDB 仓库。需要 Web Locks API 或 BroadcastChannel leader election 保证单写者。
- **隐私模型**：git clone 是 repo 级别的，即使 UI 只显示用户所属频道和 DM，IndexedDB 中存有完整仓库内容（包括其他用户的 DM）。GitIM 的隐私是展示层过滤，不是存储层隔离。如果仓库含敏感内容，需要分 repo 部署。

### 实施注意事项

以下问题在架构层面已知，需要在实施计划中具体解决：

- **cursor 存储键**：当前 UI 用 `workspace` 路径做 cursor key，local 模式没有 workspace。需要改为 `remote URL + handler` 作为 key
- **cursor reset 的 UI 契约**：当前 app.tsx 只处理增量 `changes`，没有 "全量重加载" 的事件类型。需要在 poll 响应中加 `{ reset: true }` 变体
- **renumber 后的 pending 状态**：send 返回的 line_number 在冲突重编后失效，UI 中的 pending 消息标记和 thread 引用会指向错误行号。全量重加载时需清除 pending 和 selected thread 状态
- **默认分支**：Rust 端硬编码 `origin/main`，手机端不能假设同样的默认分支。初始化时需检测实际默认分支
- **git 错误分类**：Rust 端从 stderr 解析 push conflict / rate limit。浏览器端得到的是 HTTP 错误码。需要建立错误分类规则（401=auth, 429=rate limit, 409=non-fast-forward, 502/503=proxy 故障）

## 落地节奏

### Phase 0：WASM 编译链 + Backend 抽象

- gitim-core 编译到 WASM，验证 `parse_thread` 和 `format_message` 在浏览器中正确运行
- gitim-sync 纯函数门控 + 导出，验证 `renumber_batch` 正确性
- 用 Rust 端已有的测试用例作为 TS 端的验收标准
- **client.ts Backend interface 抽取**：把现有 fetch 逻辑搬进 HttpBackend，导出 Backend interface。daemon-web 从第一天就对着真实接口开发

### Phase 1：daemon-web 核心

- storage.ts（OPFS 封装）
- git.ts（isomorphic-git clone / fetch / commit / push）
- handlers.ts（read / channels / users / me，只读路径）
- 验证：在手机浏览器中 clone 一个 GitIM 仓库并展示消息

### Phase 2：写入 + 同步

- handlers.ts 扩展（send）
- sync.ts（push-first + 冲突合并）
- 验证：手机端发消息 → 桌面端收到；桌面端发消息 → 手机端收到

### Phase 3：webui-v2 集成

- LocalBackend + Web Worker RPC（Backend interface 已在 Phase 0 抽取）
- connection store 模式切换
- local 模式初始化 gate（替代 setup-gate）
- local 模式路由适配（默认进 /chat，禁用 /management）
- 验证：同一套 UI 在桌面和手机上均可使用

## 涉及源文件

### 新增

| 文件 | 职责 |
|------|------|
| `webui-v2/src/daemon-web/worker.ts` | Web Worker 入口 |
| `webui-v2/src/daemon-web/git.ts` | isomorphic-git 封装 |
| `webui-v2/src/daemon-web/sync.ts` | 同步循环 |
| `webui-v2/src/daemon-web/handlers.ts` | API 实现 |
| `webui-v2/src/daemon-web/state.ts` | 内存状态 |
| `webui-v2/src/daemon-web/storage.ts` | lightning-fs 封装 |
| `crates/gitim-wasm/` | WASM 导出 crate（wasm-bindgen wrapper + serde 转换） |
| `webui-v2/src/lib/backend/http-backend.ts` | 现有 fetch 逻辑 |
| `webui-v2/src/lib/backend/local-backend.ts` | Worker RPC 封装 |

### 修改

| 文件 | 改动 |
|------|------|
| `webui-v2/src/lib/client.ts` | 引入 backend interface，按 mode 分发 |
| `webui-v2/src/hooks/use-connection-store.ts` | 新增 `mode: 'remote' \| 'local'` |
| `webui-v2/src/hooks/use-agent-activity.ts` | local 模式下禁用 SSE |
| `crates/gitim-core/Cargo.toml` | 添加 cdylib target，调整 chrono features |
| `crates/gitim-sync/src/lib.rs` | cfg 门控 I/O 模块 |
| `crates/gitim-sync/src/conflict.rs` | 提取 `resolve_content` 的文件 I/O |

## Alternatives Considered

### A. 远端 Runtime（被拒绝）

把 gitim-runtime + daemon 跑在服务器上，手机浏览器直接 HTTP 连接。改动量最小（runtime 加 auth + TLS），但需要自建服务器运行 daemon 进程，违背"纯客户端"目标。

### B. 只读 / 低频写入移动端（被拒绝）

只实现读取，不实现发送。大幅简化（不需要 sync、冲突解决），但无法满足"做任务处理"的需求——需要回复消息。

### C. 当前方案：浏览器原生 daemon

承认复杂度代价：引入第二套 daemon 实现（TS）、WASM bridge、浏览器内 git、CORS proxy、worker RPC、双模式 UI。这是为"纯客户端"目标付出的架构成本。

选择理由：
- 用户明确要求纯客户端，不接受服务器依赖
- WASM 共享层消除了最大的维护风险（解析逻辑分歧）
- CORS proxy 的运维负担远低于完整 daemon 服务器
- 移动端聊天是高频使用场景，值得投入
