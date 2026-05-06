# GitIM UX Onboarding 实现计划

**状态：已完成**

**Goal:** 实现 `gitim onboard` 统一入口命令、身份自动推断、daemon 身份注入，让 Agent 零配置即可发消息。

**Architecture:** 自底向上分 4 层实现：Core Config 扩展 → Daemon API 扩展（register_user、stop、身份注入）→ CLI daemon 管理增强 → CLI onboard 命令。Rust 后端（Task 1-5）和 CLI 前端（Task 6-8）可并行开发。

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
  │    └→ Task 5: Daemon send 身份注入（author 改为 Option）
  │
  └→ Task 6: CLI daemon.ts 增强（stale 清理）
       │
       └→ Task 7: CLI onboard 命令
            │
            └→ Task 8: CLI send/dm 去掉 -a 必填 + stop 命令 + 注册 onboard
                 │
                 └→ Task 9: E2E 测试
```

**并行策略：** Task 1-5（Rust）和 Task 6-8（TypeScript）文件无重叠，可并行执行。

---

## 文件变更清单

### 新增文件

| 文件 | 职责 |
|------|------|
| `cli/src/commands/onboard.ts` | onboard 命令：身份推断、clone/create/init、注册用户 |
| `cli/src/commands/stop.ts` | stop 命令：发送 stop API 给 daemon |

### 修改文件

| 文件 | 变更 |
|------|------|
| `crates/gitim-core/src/types/config.rs` | 增加 `endpoint`、`endpoint_url` 字段（带 serde 默认值） |
| `crates/gitim-daemon/src/state.rs` | `AppState` 增加 `current_user: Option<String>` |
| `crates/gitim-daemon/src/main.rs` | 启动时读取 `me.json`，传入 `current_user` |
| `crates/gitim-daemon/src/api.rs` | Request 增加 `RegisterUser`、`Stop` 变体 |
| `crates/gitim-daemon/src/handlers.rs` | 新增 `handle_register_user`、`handle_stop`；`handle_send` 支持 author 缺省 |
| `cli/src/daemon.ts` | `ensureDaemon` 增加 stale PID/socket 清理 |
| `cli/src/client.ts` | `send` author 改为可选；新增 `registerUser`、`stop` 方法 |
| `cli/src/commands/send.ts` | `-a` 改为可选参数 |
| `cli/src/commands/dm.ts` | `-a` 改为可选，新增 `resolveAuthor` 从 me.json 读取 |
| `cli/src/index.ts` | 移除 init，注册 onboard、stop |
| `tests/e2e_test.sh` | 更新适配新 API |

### 删除文件

| 文件 | 原因 |
|------|------|
| `cli/src/commands/init.ts` | 被 onboard.ts 替代 |

---

## Chunk 1: Core + Daemon 后端

### Task 1: Core Config 扩展

- [x] Config struct 增加 `endpoint`（默认 "github"）和 `endpoint_url`（默认 ""）字段
- [x] 新增 3 个 validator 测试覆盖 endpoint 解析和默认值
- [x] Commit: `feat(core): add endpoint and endpoint_url to Config`

### Task 2: Daemon 身份读取

- [x] `AppState` 增加 `current_user: Option<String>` 字段，`new()` 签名更新
- [x] `main.rs` 启动时读取 `.gitim/me.json`，提取 handler 传入 AppState
- [x] Commit: `feat(daemon): read identity from me.json on startup`

### Task 3: Daemon API — register_user

- [x] Request 枚举新增 `RegisterUser { handler, display_name, role, introduction }` 变体
- [x] 实现 handler：校验 handler 格式 → 检查已存在 → 创建 meta.json → 加入用户列表 → git add + commit
- [x] Commit: `feat(daemon): add register_user API endpoint`

### Task 4: Daemon API — stop

- [x] Request 枚举新增 `Stop` 变体
- [x] 实现 handler：清理运行时文件 → 延迟 100ms 退出（保证响应发送完成）
- [x] Commit: `feat(daemon): add stop API endpoint for graceful shutdown`

### Task 5: Daemon send 身份注入

- [x] Send 变体的 `author` 改为 `Option<String>`（`#[serde(default)]`）
- [x] 解析链：explicit author > state.current_user > 返回错误
- [x] Commit: `feat(daemon): make author optional in send, fallback to current_user`

---

## Chunk 2: CLI 层

### Task 6: CLI daemon.ts 增强

- [x] `ensureDaemon` 增加 stale 运行时文件清理（pid、sock、port、lock）
- [x] 增加 startup race 处理：daemon 进程存在但 socket 未就绪时轮询等待
- [x] Commit: `feat(cli): add stale runtime file cleanup in ensureDaemon`

### Task 7: CLI onboard 命令

- [x] 实现 `inferIdentity`：GitHub（`gh api /user`）和 Gitea（curl API）两种推断方式
- [x] 实现 `initGitimRepo`：创建目录结构、写 config.yaml、更新 .gitignore、创建用户和默认频道
- [x] 实现 `onboardCommand`：--refresh 模式 / clone → 加载/初始化/创建 三条分支
- [x] 使用 `execFileSync` 替代 `execSync` 防止命令注入
- [x] Commit: `feat(cli): add onboard command implementation`

### Task 8: CLI 集成

- [x] `client.ts` 新增 `registerUser()`、`stop()` 方法，`send()` author 改为可选
- [x] 创建 `stop.ts` 命令
- [x] `send.ts`、`dm.ts` 的 `-a` 改为可选，dm 新增 `resolveAuthor` 从 me.json 读取
- [x] `index.ts` 移除 init，注册 onboard、stop
- [x] DM channel 名称排序确保一致性
- [x] Commit: `feat(cli): integrate onboard/stop commands, make author optional`

---

## Chunk 3: E2E 测试

### Task 9: 端到端集成测试

- [x] 重写 `e2e_test.sh`，使用 `nc -U`（macOS 原生），覆盖：status、send without author、send with author、read with identity check、channels、users、register_user（新建 + 已存在）、stop
- [x] Commit: `test: update e2e test for identity injection and new API endpoints`
