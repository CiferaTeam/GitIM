# Onboard 重构 Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 onboard 的身份推断、repo 初始化、用户注册全部收敛到 daemon，CLI 只负责收集参数和转发请求。

**Architecture:** daemon 新增 `Onboard` 请求类型，内部编排四步：身份推断 → 写 me.json → EnsureRepo（幂等初始化 .gitignore + channels）→ RegisterUser（注册用户）。daemon 支持 bootstrap 模式（无 config.yaml 时写默认值启动）。sync loop 延迟到 onboard 完成后再启动。

**Tech Stack:** Rust（daemon）、TypeScript（CLI）、shell out curl（API 调用）

---

## Chunk 1: Daemon Bootstrap 模式

### Task 1: Config 默认值 + 缺失时自动创建

**Files:**
- Modify: `crates/gitim-core/src/types/config.rs` — 为 Config 实现 Default trait
- Modify: `crates/gitim-daemon/src/main.rs:23-27` — config 文件不存在时写默认值而非 panic

**变更描述：**
- Config struct 实现 Default：version=1, endpoint="github", endpoint_url="", daemon 使用现有默认值
- main.rs 启动时：尝试读 config.yaml → 不存在则创建默认 Config，序列化为 YAML 写入 `.gitim/config.yaml`（先确保 `.gitim/` 目录存在）→ 继续正常流程
- validate_config 仍然对加载的 config 做校验

**验收标准：**
- 删除 .gitim/config.yaml 后启动 daemon，应自动创建默认配置并正常启动
- 已有 config.yaml 时行为不变

**Commit:** `feat(daemon): bootstrap — create default config.yaml if missing`

---

### Task 2: 无 me.json 时不 panic，延迟设置 current_user

**Files:**
- Modify: `crates/gitim-daemon/src/main.rs:44-52` — me.json 不存在时 current_user = None，不 warn
- Modify: `crates/gitim-daemon/src/state.rs:17-26` — 将 current_user 改为可变（用 RwLock 包装）

**变更描述：**
- main.rs：me.json 不存在 → current_user = None，静默处理（不是异常，是正常的首次启动）
- AppState.current_user 从 `Option<String>` 改为 `RwLock<Option<String>>`，Onboard 完成后写入
- handle_send 等读取 current_user 的地方适配 RwLock 读取

**验收标准：**
- 无 me.json 启动 daemon 不报错
- send 请求在未 onboard 时返回错误（current_user 为 None）
- 现有测试不受影响

**Commit:** `feat(daemon): allow startup without me.json, defer identity`

---

### Task 3: 延迟启动 sync loop

**Files:**
- Modify: `crates/gitim-daemon/src/main.rs:91-136` — sync loop 不在启动时立即 spawn
- Modify: `crates/gitim-daemon/src/state.rs` — 新增启动 sync loop 的方法或信号机制

**变更描述：**
- 如果启动时 current_user 为 None（未 onboard），不启动 sync loop
- 如果启动时 current_user 已有值（me.json 存在，重启场景），照常启动 sync loop
- Onboard 完成后触发 sync loop 启动（通过 oneshot channel 或直接在 handle_onboard 中 spawn）
- sync loop 的 spawn 逻辑抽取为独立函数，可被 main 和 onboard handler 调用

**验收标准：**
- 首次启动（无 me.json）：daemon 运行但不 sync
- Onboard 完成后：sync loop 开始运行
- 重启（有 me.json）：sync loop 立即启动

**Commit:** `feat(daemon): defer sync loop until identity is set`

---

## Chunk 2: 身份推断模块

### Task 4: 新增 identity 模块

**Files:**
- Create: `crates/gitim-daemon/src/identity.rs` — 身份推断逻辑
- Modify: `crates/gitim-daemon/src/main.rs` — mod identity

**变更描述：**
- 定义 `GitServer` 枚举：Git, GitHub, Gitea, GitLab
- 定义 `AuthData` 枚举，每个 variant 携带对应认证信息：
  - Git: handler + display_name（用户直传，不需要推断）
  - GitHub: token → shell out `curl -sf -H "Authorization: token {}" https://api.github.com/user`，解析 JSON 拿 login + name
  - Gitea: token + url → shell out `curl -sf -H "Authorization: token {}" {url}/api/v1/user`
  - GitLab: token + url → shell out `curl -sf -H "Authorization: Bearer {}" {url}/api/v4/user`，解析 JSON 拿 username + name
- 定义 `InferredIdentity` 结构体：handler, display_name
- 公开函数 `infer_identity(git_server, auth_data) -> Result<InferredIdentity, IdentityError>`
- handler 结果需通过 `Handler::new()` 校验合法性

**验收标准：**
- 单元测试：Git 模式直接返回传入值
- 单元测试：各平台模式在 curl 失败时返回明确错误
- handler 不合法时返回 IdentityError

**Commit:** `feat(daemon): add identity inference module with git/github/gitea/gitlab support`

---

## Chunk 3: Onboard 请求处理

### Task 5: 新增 Onboard 请求类型

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs:18-58` — 新增 Onboard variant
- Modify: `crates/gitim-daemon/src/handlers.rs:10-43` — dispatch Onboard

**变更描述：**
- Request 枚举新增 `Onboard { git_server: String, auth: serde_json::Value }`
  - git_server: "git" | "github" | "gitea" | "gitlab"
  - auth: 不同 git_server 对应不同结构，handler 内解析
- handle_request 中增加 Onboard 分支，调用 handle_onboard

**验收标准：**
- Onboard 请求能正确反序列化
- 未知 git_server 值返回错误

**Commit:** `feat(api): add Onboard request type`

---

### Task 6: 实现 handle_onboard 核心逻辑

**Files:**
- Create: `crates/gitim-daemon/src/onboard.rs` — onboard 编排逻辑
- Modify: `crates/gitim-daemon/src/handlers.rs` — 调用 onboard 模块
- Modify: `crates/gitim-daemon/src/main.rs` — mod onboard

**变更描述：**

handle_onboard 内部四步编排：

**Step A: 身份推断**
- 解析 git_server + auth → 调用 identity::infer_identity
- 失败则返回错误，中断流程

**Step B: 写 me.json**
- 将 handler、display_name、git_server、推断时间写入 `.gitim/me.json`
- 更新 AppState.current_user（写 RwLock）

**Step C: EnsureRepo（幂等）**
- 检查 `.gitignore` 是否已包含 `.gitim/` → 没有则追加
- 检查 `channels/general.meta.json` 是否存在 → 没有则创建 meta + 空 thread
- 如果有变更：git add + commit + push
- push 冲突（PushConflict）→ discard_unpushed() + 跳过（别人已初始化）
- push 其他错误 → 返回错误

**Step D: RegisterUser（幂等）**
- 检查 `users/{handler}.meta.json` 是否存在 → 存在则跳过
- 不存在：创建文件 → git add + commit + push
- push 冲突 → fetch + rebase_onto_origin + retry push（最多 3 次）
- rebase 失败 → 报错（真正的文件冲突）

**Onboard 完成后：**
- 触发 sync loop 启动（Task 3 中定义的机制）
- 返回 `{handler, created}`

**验收标准：**
- e2e 测试：首次 onboard 创建 config + me.json + .gitignore + channels + users，push 成功
- e2e 测试：二次 onboard 全部跳过，返回 created=false
- e2e 测试：并发 EnsureRepo 冲突 → discard 后仍成功
- e2e 测试：RegisterUser push 冲突（不同用户并发）→ rebase 重试成功
- e2e 测试：身份推断失败 → 返回错误，不创建任何文件

**Commit:** `feat(daemon): implement handle_onboard with EnsureRepo + RegisterUser`

---

## Chunk 4: CLI 重构

### Task 7: 更新 CLI 命令定义

**Files:**
- Modify: `cli/src/index.ts:20-27` — 更新 onboard 命令参数
- Modify: `cli/src/client.ts` — 新增 onboard 方法

**变更描述：**
- onboard 命令参数变更：
  - 保留：`[repo_name] [org]`、`--refresh`
  - 改：`--endpoint` 改为 `--git-server`，值域扩展为 `git | github | gitea | gitlab`
  - 新增：`--token`（GitHub/Gitea/GitLab 的认证 token）
  - 新增：`--handler`（git 本地模式必填）
  - 新增：`--display-name`（git 本地模式必填）
  - 保留：`--url`（Gitea/GitLab 服务地址）
- GitimClient 新增 `onboard(gitServer, auth)` 方法，发送 Onboard 请求

**验收标准：**
- `gitim onboard --help` 显示新参数
- client.onboard() 能正确序列化请求

**Commit:** `feat(cli): update onboard command args and client method`

---

### Task 8: 重写 onboard 命令实现

**Files:**
- Modify: `cli/src/commands/onboard.ts` — 大幅简化
- Modify: `cli/src/daemon.ts:8-18` — findRepoRoot 不再依赖 config.yaml

**变更描述：**

onboard.ts 简化为：
1. 参数校验：根据 git-server 类型校验必填参数（github 需要 token，git 需要 handler + display_name）
2. Clone/Create repo（保留现有 clone + repo create 逻辑）
3. 确保 `.gitim/` 目录存在（仅目录，不写文件）
4. 启动 daemon（ensureDaemon）
5. 发送 Onboard 请求（传 git_server + auth 数据）
6. 输出结果

删除的逻辑：
- `inferIdentity()` 函数整个删除
- `initGitimRepo()` 函数整个删除
- `writeMeJson()` 函数整个删除
- 所有直接操作 git 的代码删除

daemon.ts:
- `findRepoRoot()` 改为查找 `.gitim/` 目录（而非 `.gitim/config.yaml`），因为 config 可能还没创建
- `--refresh` 模式：改为发送 Onboard 请求（daemon 重新推断身份并更新 me.json）

**验收标准：**
- `gitim onboard test-repo --git-server github --token ghp_xxx` 完成全流程
- `gitim onboard test-repo --git-server git --handler alice --display-name Alice` 完成全流程
- `gitim onboard --refresh` 重新推断身份
- onboard.ts 中无任何直接的 git 操作或文件系统初始化操作

**Commit:** `refactor(cli): delegate onboard logic to daemon Onboard request`

---

## Chunk 5: 集成测试 + 清理

### Task 9: 端到端集成测试

**Files:**
- Create: `crates/gitim-sync/tests/onboard_test.rs` — Onboard 相关的 git 操作测试
- Modify: 现有 e2e 测试适配新流程

**变更描述：**
- 测试 EnsureRepo 幂等性：两次调用结果一致
- 测试 EnsureRepo push 冲突：两个 clone 同时 EnsureRepo，第二个 discard 后成功
- 测试 RegisterUser 幂等性：已存在跳过
- 测试 RegisterUser 并发：两个不同用户同时注册，rebase 重试成功
- 测试 bootstrap 模式：无 config.yaml 启动 daemon，发送 Onboard，验证 config + me.json + repo 结构全部创建
- 验证现有 sync / conflict / send 测试不被破坏

**验收标准：**
- 所有新增测试通过
- 所有现有测试通过
- `cargo test` 全绿

**Commit:** `test: add onboard e2e tests for EnsureRepo and RegisterUser`

---

### Task 10: 清理遗留代码

**Files:**
- Modify: `cli/src/commands/onboard.ts` — 确认无死代码
- Modify: `crates/gitim-daemon/src/handlers.rs` — handle_register_user 保留但确认不再被 onboard 直接调用（仍可作为独立请求使用）

**变更描述：**
- 确认 CLI 中 inferIdentity / initGitimRepo / writeMeJson 已完全删除
- 确认 daemon 中 handle_register_user 仍独立可用（非 onboard 场景下的用户注册）
- 更新 CLAUDE.md 中的架构描述，反映新的 onboard 流程

**验收标准：**
- 无未使用的代码残留
- `cargo test` + `npm test`（如有）全绿

**Commit:** `chore: clean up legacy onboard code and update docs`
