# 单 Runtime 多 Workspace — 需求共识

## 背景

当前 `gitim-runtime` 一个进程只服务一个 workspace:`RuntimeState.workspace: Option<PathBuf>`。用户想在同一台机上管理多个独立 GitIM 仓库(工作主线、实验分支、教学/演示等),当前必须:

- 为每个 workspace 启一个 runtime 进程(多端口、多 localStorage、多 WebUI 实例)
- 或手动切换 runtime 所服务的 workspace(需重启 + 重登 + 前端状态丢失)

目标:**单 runtime 进程,内部持有多个 workspace,前端走单一 `127.0.0.1:<port>` 直连,所有 workspace-scoped 操作带 slug 维度**。

## 用户决策(定海神针)

- **方案 2:单 runtime 多 workspace**(单进程,单端口,内部 `HashMap<Slug, WorkspaceContext>`)
- **WorkspaceId = slug**(短字符串,路径安全)
- **同一 handler 可跨 workspace**(每 ws 一份 human clone,commit author 相同、remote 不同)
- **前端 ↔ runtime 直连 `127.0.0.1:<port>`**,localStorage 只存一个端口
- **从 0 原生支持多 workspace**,不设计迁移路径、不考虑兼容旧 schema
- **v1 专注 multi-workspace feature 本身**,不扩展"跨 workspace 共享配置"等周边

## Phase 2 Grill 收敛(5 项开放项)

### 1. Slug 输入方式
- **系统生成**:取目录 basename → 规范化(小写,`[^a-z0-9-]` → `-`,连续 `-` 折叠,首尾去 `-`,截断 32 字符,空则 fallback `workspace`)
- **冲突加后缀** `-2`, `-3`, ...
- **v1 永久固定**,不支持重命名
- **独立字段 `workspace_name: String`** 承担用户备忘展示,可改,默认 = 目录 basename 原样(保留空格/大小写)
- **保留关键字**:`default`, `system`, `active`, `current` — 撞上同样走后缀逻辑

### 2. HTTP 路由形态
- **Path 前缀** `/workspaces/{slug}/...`(SSE 浏览器 `EventSource` 不支持自定义 header,一票否决 header 方案)
- axum 用 `.nest("/workspaces/:slug", ws_router)`

**全局路由(不带 slug)**:
- `GET  /health`
- `GET  /workspaces` — 列所有
- `POST /workspaces` — 创建(合并现 `/workspace` + `/git/init`)
- `DELETE /workspaces/{slug}` — 删除:停 daemon + 清 runtime.json 条目,**不删本地文件**
- `GET  /preflight/{provider}` — provider CLI 可用性,跨 ws

**Workspace-scoped**:
- `/workspaces/{slug}/im/*`
- `/workspaces/{slug}/agents/*`
- `/workspaces/{slug}/agents/events`(SSE)

### 3. SSE 分流
- **Per-workspace** `broadcast::Sender` 存于 `WorkspaceContext`,buffer 128 不变
- `AgentActivityEvent` 增加 `workspace_id: String` 字段(为未来全局聚合视图预留)
- `DELETE` 时 channel 随 ctx drop → 订阅者 EOF → 前端 EventSource 重连 → 404 → 前端显式处理"ws 已删"

### 4. 跨 Workspace 共享 Provider 配置
- **砍掉**。Provider 认证由 Claude/Codex CLI 自管(天然跨 ws 共享,GitIM 不碰)。Per-agent 配置继续存 `{agent}/.gitim/me.json`。v1 不引入"用户级默认偏好"。

### 5. WorkspaceContext + 锁粒度
- **大锁** `Arc<Mutex<RuntimeState>>` 保持现模式(本地单用户工具,并发竞争非瓶颈)
- `RuntimeState { workspaces: HashMap<Slug, WorkspaceContext>, last_activity, github_api, clone_url_override }`
- `WorkspaceContext { slug, workspace_name, path, human_repo, poll_cursor, agents, activity_tx, auth_failed, git_config }`
- **不引入 `active_workspace`** — 前端自己记最后选中的 slug

## 实施架构

```
┌──────────────────────────────────────────────────────────┐
│                  RuntimeState (Arc<Mutex<>>)              │
│                                                           │
│  ┌─────────────────────────────────────────────────────┐ │
│  │ workspaces: HashMap<Slug, WorkspaceContext>         │ │
│  │                                                      │ │
│  │  ┌──────────────────┐   ┌──────────────────┐        │ │
│  │  │ slug: "frontend" │   │ slug: "auth-ref" │  ...   │ │
│  │  │ workspace_name   │   │ workspace_name   │        │ │
│  │  │ path             │   │ path             │        │ │
│  │  │ human_repo       │   │ human_repo       │        │ │
│  │  │ poll_cursor      │   │ poll_cursor      │        │ │
│  │  │ agents: HashMap  │   │ agents: HashMap  │        │ │
│  │  │ activity_tx      │   │ activity_tx      │        │ │
│  │  │ auth_failed      │   │ auth_failed      │        │ │
│  │  │ git_config       │   │ git_config       │        │ │
│  │  │ daemon_handle    │   │ daemon_handle    │        │ │
│  │  └──────────────────┘   └──────────────────┘        │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                           │
│  last_activity: AtomicU64     (全局 idle watchdog)       │
│  github_api: Arc<dyn ...>     (跨 ws 共享 HTTP client)   │
│  clone_url_override           (e2e 测试 seam)            │
└──────────────────────────────────────────────────────────┘
                         │
                         ▼
         ┌───────────────────────────────┐
         │  HTTP Router (axum)            │
         │                                │
         │  Global:                       │
         │    /health                     │
         │    GET/POST/DELETE /workspaces │
         │    /preflight/{provider}       │
         │                                │
         │  .nest("/workspaces/:slug",    │
         │    ws_router) where ws_router: │
         │    /im/*                       │
         │    /agents/*                   │
         │    /agents/events (SSE)        │
         └───────────────────────────────┘

Per-workspace daemon:
  每个 WorkspaceContext 持有自己的 daemon 进程(Unix socket 在
  workspace/.gitim/daemon.sock),独立于其他 ws。
```

### runtime.json schema(用户级 `~/.gitim/runtime.json`)

```json
{
  "workspaces": [
    { "slug": "frontend", "workspace_name": "Frontend 主线", "path": "/Users/x/code/frontend" },
    { "slug": "auth-ref", "workspace_name": "auth refactor", "path": "/Users/x/code/auth-ref" }
  ]
}
```

从 0 支持,**无兼容旧 `{ workspace: ... }` schema 的负担**。

## Phase 3 Eng Review 决议

### Step 0 Scope ✓
- Touched files ~14-16(runtime 顶层 state 重构 + webui-v2 client 层 + switcher UI)
- 是真实复杂度(顶层架构变更),不是 scope creep
- TODO 交叉:`gitim-runtime HTTP 层 integration test`(2026-04-17 登记)— 本次改造是合适时机**新增 HTTP integration test 骨架**(不求本次完整覆盖,但要搭起来)

### Architecture

**A1** [P1,9/10] `POST /workspaces` TOCTOU — 并发两次带同一 basename 会都分到同一后缀。修复:**slug 生成 + HashMap slot 占位 + runtime.json 写入**必须在同一个 `state.lock()` 生命周期内串行(后续 IO 如 daemon 启动可释放锁,失败回滚时再拿锁删 slot)。

**A2** [P1,9/10] Daemon 生命周期 — `WorkspaceContext` 持 `daemon_handle: Option<Child>`(或 PID);`DELETE /workspaces/{slug}` 走 graceful(SIGTERM + 5s + SIGKILL);runtime 进程退出时(signal handler / Drop)**迭代所有 ws** 执行同样 shutdown。

**A3** [P1,8/10] Slug path-traversal 防护 — axum `:slug` 天然不跨 `/` 所以最坏情况不 match,但应在 extractor 显式校验 `^[a-z0-9-]{1,32}$`,非法直接 400(防呆 + future-proof)。

**A4** [P2,7/10] `POST /workspaces` 失败回滚语义 — daemon 启动失败或 clone 失败 → 不写 runtime.json,不留 HashMap slot(carry 现单 ws 的 "失败清理" 语义)。

### Code Quality

**C1** [P2,7/10] DRY — 每个 workspace-scoped handler 开头都 `state.lock().workspaces.get(&slug)` 查找 + 404 分支。建议:抽一个 `fn with_workspace<F, R>(state, slug, f) -> Result<R>` helper,或实现一个 axum extractor `WorkspaceCtxSnapshot`,一处完成 404/校验/快照字段拷出。注意**不跨 await 持锁**。

**C2** [P2,7/10] Recover 并行 — `recover_from_config` 对 N 个 workspace 串行启动 daemon + N × M 个 agent 子进程会慢。建议:`tokio::join!` 或 `FuturesUnordered` 并行 recover 各 workspace(内部单 ws 的 agents 仍按现有顺序)。

### Tests

覆盖要求(本期 in-scope):
- Slug 生成/规范化/冲突后缀 — unit test
- `POST /workspaces` 成功 + basename 冲突 + 非法字符规范化 + daemon 失败回滚
- `DELETE /workspaces/{slug}` 成功 + 404 + daemon kill + SSE EOF
- Recover multi-workspace(空 / 1 / N / 部分失败)
- HTTP 路由 integration — 证明 `/workspaces/{slug}/im/*` 正确 dispatch(新建 integration test 骨架)
- `AgentActivityEvent.workspace_id` 填充正确

### Performance

- 每请求 HashMap lookup:O(1),忽略
- Per-ws broadcast 内存:~20 ws × 128 × 200B ≈ 500KB,忽略
- Recover 并行化(C2)= 主要优化项
- Token propagation 从"iterate agents"→"iterate (ws × agents)",启动期多跑一轮,非 hot path

## 非目标(v1 不做)

- `active_workspace` 字段(前端自己记)
- Workspace / slug 重命名
- Local → github 迁移 / 换 remote URL(决策摘要已定)
- Token rotate UI(继续手工改 config.json + 重启)
- Windows(继承 workspace-github-mode 的 scope 限制)
- Agent 独立 GitHub 身份(共用 workspace PAT)
- OAuth Device Flow
- 跨 workspace 共享 provider 配置(Q4 砍掉)
- 全局聚合 activity feed UI(字段预留,UI 推 v2)
- 前端跨 workspace 数据并发预取 / cache 共享

## 测试要点

### Runtime 单元测试
- `slug.rs`:`generate_from_basename`, `resolve_conflict`, `validate`
- `RuntimeState::add_workspace`, `remove_workspace`
- `AgentActivityEvent` 序列化含 `workspace_id`

### Runtime 集成测试(新增 HTTP integration test 骨架)
- `POST /workspaces` 端到端 — local 模式(github 模式延续 workspace-github-mode 现有测试模式)
- `DELETE /workspaces/{slug}` — daemon kill + HashMap 移除
- `GET /workspaces` — 列表返回正确 slug + name
- `/workspaces/{slug}/im/send` 路由 dispatch + 错误 slug 404
- SSE `/workspaces/{slug}/agents/events` 订阅 + DELETE 触发 EOF

### Runtime 端到端(poller 测试风格)
- 两个 workspace 并行存在,各自 agent 互不干扰
- Recover 从 runtime.json 恢复 N 个 ws

### WebUI-v2
- Workspace switcher UI(加入 vitest 基建同时搞定)
- `client.ts` 带 slug 的 fetch
- EventSource URL 带 slug 重连

## 关键文件映射

| 职责 | 文件 |
|---|---|
| RuntimeState 定义 | `crates/gitim-runtime/src/http.rs:104-146`(顶层拆分) |
| WorkspaceContext 新文件 | `crates/gitim-runtime/src/workspace.rs`(新增) |
| Slug 模块(新增) | `crates/gitim-runtime/src/slug.rs`(新增) |
| runtime.json schema | `crates/gitim-runtime/src/http.rs:1491-1502`(recover + 写入) |
| HTTP router | `crates/gitim-runtime/src/http.rs:1773-1810`(nest) |
| SSE endpoint | `crates/gitim-runtime/src/http.rs:1468-1487` |
| AgentActivityEvent | `crates/gitim-runtime/src/http.rs:73-79` |
| Agent loop event emit | `crates/gitim-runtime/src/agent_loop.rs`(加 workspace_id) |
| Token propagation | `crates/gitim-runtime/src/token_propagation.rs:34-54`(多 ws 迭代) |
| WorkspaceConfig | `crates/gitim-runtime/src/git_config.rs`(每 ws 一份,不变) |
| Preflight | `crates/gitim-runtime/src/preflight.rs`(全局,不变) |
| WebUI client | `webui-v2/src/lib/client.ts:12-14`(URL 加 slug) |
| WebUI SSE hook | `webui-v2/src/hooks/use-agent-activity.ts:30-60` |
| WebUI connection store | `webui-v2/src/hooks/use-connection-store.ts`(加 active workspace slug) |
| WebUI workspace switcher(新增) | `webui-v2/src/components/workspace-switcher.tsx` |
| WebUI app shell | `webui-v2/src/app.tsx`(wire up switcher) |

## 后续(Phase 4 Writing-Plans 产出 `01-plan.md`)

- 按 TDD + tracer-bullet 拆分实施 steps
- 识别并行 lane(后端数据结构 → HTTP → 前端 client → 前端 UI)
- 每步包含:测试先写、实现、验收命令
