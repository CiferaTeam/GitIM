# GitIM

面向 Agent 团队的 AI 原生 IM 协议。纯文本文件 + Git。

## 架构

- 消息是 `.thread` 文件中的行，前缀格式：`[L<行号>][P<父行号>][@<handler>][<时间戳>] <正文>`
- 通过 `P` 字段实现线程链 — 无需 thread_id
- 续行：下一行没有 `[L...]` 开头即为当前消息的续行
- 用户：`users/<handler>.meta.yaml`，handler = GitHub handle（小写）
- 技术栈：Rust daemon（核心引擎）+ Rust CLI + React WebUI
- 通信：Unix socket（默认）+ HTTP（调试模式 & WebUI）
- Git 负责持久化、同步和审计追踪
- 合规性：daemon 写入验证（主防线）+ 读取检测（第二防线）

## Crate 地图

```
gitim-cli ──→ gitim-client ──→ [Unix socket IPC] ──→ gitim-daemon
                                                        ├── gitim-core（类型 + 解析）
                                                        ├── gitim-sync（git 同步）
                                                        └── gitim-index（全文搜索）

gitim-runtime ──→ gitim-client
               ──→ gitim-agent-provider
                     ├── claude（Claude CLI 集成）
                     ├── codex（Codex CLI 集成）
                     └── mock（测试用）
```

### 核心 crate

| Crate | 职责 | 关键模块 |
|-------|------|----------|
| `gitim-core` | 数据类型、消息解析、格式化、校验 | `types`, `parser`, `formatter`, `validator`, `dm`, `mention`, `link` |
| `gitim-daemon` | 主服务进程，处理所有 IM 操作 | `handlers`（消息/频道）, `board_handlers`（看板）, `onboard`（用户注册）, `identity`（身份推断）, `http`（SSE 推送）, `state`（共享状态） |
| `gitim-sync` | Git 同步循环、冲突解决、行号重编 | `git`（GitStorage 封装）, `sync_loop`, `conflict`, `renumber`, `watcher` |
| `gitim-index` | SQLite FTS5 全文搜索 | 单文件 `lib.rs`，支持按 author/channel/query 搜索 |
| `gitim-client` | IPC 客户端库，封装 daemon 通信 | `GitimClient`（所有 API 方法）, `daemon`（进程管理）|
| `gitim-cli` | 命令行工具（clap） | `send`, `read`, `channels`, `create-channel`, `join-channel`, `status` 等 |

### Agent 运行时

| Crate | 职责 | 关键模块 |
|-------|------|----------|
| `gitim-runtime` | Agent 生命周期管理、polling、HTTP API | `agent`（provision）, `agent_loop`（消息检测 → AI 处理 → 回复）, `poller`, `preflight`, `http`（WebUI API）|
| `gitim-agent-provider` | AI 提供商抽象层 | `claude`（Claude CLI）, `codex`（Codex CLI，部分 stub）, `mock` |

### 前端

| 目录 | 状态 | 说明 |
|------|------|------|
| `webui-v2/` | **当前主线** | React 19 + Vite + Radix UI + Tailwind + Zustand |
| `webui/` | 遗留 | 早期 React 原型，含 `legacy_client/`（Node.js bridge server）|

### 遗留 / 不要修改

| 目录 | 说明 |
|------|------|
| `legacy/cli/` | 旧版 TypeScript CLI（`@gitim-runtime/cli`），已被 Rust `gitim-cli` 取代 |
| `legacy/packages/` | 旧版 npm 包 |
| `webui/legacy_client/` | 旧版 Node.js bridge server |
| `products/site/` | 文档站点 |
| `demo/` | 演示用 |

## Onboard 流程

CLI 完全委托 daemon 处理身份推断和仓库初始化：

1. **CLI 阶段**：收集用户参数（git 类型、token 等）
2. **仓库克隆/初始化**（CLI）：克隆或创建 git 仓库，创建 `.gitim/` 目录（git 忽略）
3. **Daemon 阶段**：
   - **身份推断**（Onboard 处理）：根据 git 类型和 token 推断 handler + 信息
   - **用户注册**（RegisterUser 处理）：创建 `users/<handler>.meta.yaml`
   - **Repo 初始化**：生成 `.gitim/config.yaml`、初始化 `me.json`
   - **Git 提交**：各文件变更提交到 git

支持的身份推断渠道：
- **git 本地模式**：直接指定 handler + display_name
- **GitHub**：通过 token 调用 API 获取用户信息
- **Gitea/GitLab**：通过 token + 自定义 URL 调用对应 API

### Runtime / WebUI 路径（workspace 级）

WebUI 走 Runtime 的 `/git/init` HTTP 端点。两种 provider：

1. **local 模式**：创建 `$workspace/repo.git` bare repo → clone 到 `$workspace/.gitim-runtime/human/` → 本地 git config 推断身份。
2. **github 模式**：
   - `validate_workspace_path` 拒绝云同步路径（iCloud Drive / Dropbox / Google Drive / OneDrive）
   - Windows 不支持（v1 scope 外）
   - Runtime pre-flight：`github::verify_token` → `github::check_repo_access`（区分 404 / 403，分别映射 `invalid_token`、`token_lacks_repo_access`、`insufficient_scope` 等 `error_code`）
   - Clone token URL `https://x-access-token:TOK@github.com/owner/repo.git` 到 `.gitim-runtime/human/`（**不创建本地 bare**）
   - Daemon 走 `AuthData::GitHub` 分支自己推断身份（curl `/user`）
   - macOS 加 Time Machine exclusion xattr 到 `.gitim-runtime/`
   - 失败清理：kill daemon pid + rm human dir + 不写 config

### WorkspaceConfig Schema

`$workspace/.gitim-runtime/config.json`（chmod 0600，unix 唯一权限模型）：

```json
{
  "workspace": "/abs/path",
  "created_at": "2026-04-17T10:20:30Z",
  "git": {
    "provider": "local" | "github",
    "remote_url": "https://github.com/org/repo" | null,
    "token": "ghp_..." | null,
    "github_email": "owner@example.com" | null
  }
}
```

**Token source of truth = 这份文件**。各 clone 的 `.git/config` URL 里嵌的 token 是派生值。

**`github_email` source of truth** 也是这份文件(github 模式下 `/git/init` 时从 GitHub `/user` 自动拉取,best-effort)。`provision_agent` 读它,注入新 agent onboard 的 `git` 变体 auth payload (`github_email` 字段),走 daemon `InferredIdentity.email` → `write_me_json` → agent `.gitim/me.json` → `AppState.github_email` → commit author。所有 daemon commit 因此 author email 归 workspace owner,计入 contribution graph。

- Runtime 启动（recover workspace 后）+ `add_agent` 成功后 → 调 `token_propagation::propagate_token` 扫所有 clone 并覆盖 `remote.origin.url`
- 未来 "Update token" UI（v2）→ 改 config.json → propagate → 所有 clone 同步
- Daemon `write_me_json` 采用 **merge 语义**:re-onboard 不传 `github_email` 时保留旧值,防抹掉已配置的字段

### Handler 冲突防护（github 模式）

`add_agent` 在 provision 前：
1. `git fetch origin` human clone（best-effort，失败降级到本地检查）
2. 检查 `users/<handler>.meta.yaml` 存在性 → 存在 → 拒绝（`error_code: "handler_conflict"`）

防止多机 workspace 同名 agent 两处跑 daemon 造成 split-brain。

### sync_loop auth 熔断

daemon 的 push/fetch 连续 3 次 auth 失败（401 / 403） → `auth_failed` Arc<AtomicBool> 置位 → 后续 sync cycle 直接跳过 git 操作，只保持 cadence。

避免 PAT 过期 / revoke 后死循环烧 GitHub rate limit（5000 req/h）。v1 **无 UI 恢复路径**：用户要么重启 daemon（清标志），要么等 v2 加"更新 token"入口。

### Non-goals (v1)

- **local → github 迁移**：需 rm -rf workspace 重建
- **换 remote URL**：需 rm -rf 重建
- **Token rotate UI**：v1 无，手工改 config.json + 重启 runtime
- **Windows 支持**：`chmod 0600` + xattr + `dirs::home_dir` 的 OneDrive 检测不适配
- **Agent 独立 GitHub 身份**：共用 workspace PAT 和 workspace owner email。commit author name = agent handler；author email = `WorkspaceConfig.git.github_email`(github mode /git/init 时从 `/user` API 自动拉取),fallback `<handler>@gitim`。GitHub committer = PAT owner。审计归因通过 author **name** 字段(handler),email 统一到 workspace owner 后所有 daemon commit 都能算进该账户的 contribution graph。sync_loop 的 rebase-resolution commit 也 stamp daemon owner(而非本地 git config fallback),维持每个 clone 的"一人一 commit"归属
- **OAuth Device Flow**：不做。PAT 手动粘贴

## 约定
- Handler：小写 a-z 0-9 连字符，1-39 字符，`system` 为保留字
- DM 文件名：两个 handler 按字典序排列，`--` 连接
- Plan / 需求 / 设计文档统一放 `docs/plans/<feature-slug>/`，不要散落在仓库根或新建 `plans/`

## 测试

```bash
cargo test                                    # 全量（700+ tests，数分钟级别，贵）
cargo test -p gitim-core                      # 核心类型/解析
cargo test -p gitim-daemon                    # daemon handler 集成测试
cargo test -p gitim-sync                      # git 同步逻辑
cargo test -p gitim-runtime --test poller     # poller 集成测试（需编译 daemon）
```

### 跑测试的节奏（重要）

**全量 `cargo test` 是一个昂贵操作**（700+ 测试、含启动真实 daemon 的集成测试，耗时以分钟计）。在多 agent / subagent / 长任务流程里频繁触发会把总时长拖得非常夸张，不要无脑跑。

节奏约定：
- **任务开头**：跑一次全量，建立 baseline（确认当前 main 是绿的，排除祖传红测试干扰判断）
- **任务末尾 / 交付前**：跑一次全量，确认没有 regression
- **开发中间**：**只跑相关 crate / 相关 `--test` 目标 / 相关 `#[test]` 过滤**（`cargo test -p <crate>`、`cargo test <name_substring>`、`cargo test --test <file>`），不要每改一次就全量
- Subagent / 并行任务里：同样原则，subagent 自己干活时只跑相关测试，汇总到主线再考虑全量

如果某次改动跨 crate、涉及共享类型 / 协议、或改了 workspace 级依赖，才需要中途加跑一次全量。否则相信 scoped 测试。

注意事项：
- `gitim-runtime` 的 poller 测试启动真实 daemon 进程，用 `serial_test` 串行执行
- `claude.rs` 和 `agent_loop.rs` 的测试标记 `#[ignore]`，需要真实 Claude CLI，手动运行
- 测试惯例：外部 `tests/` 目录优先，内联 `#[cfg(test)]` 用于纯 unit test

## Design System
Always read DESIGN.md before making any visual or UI decisions.
All font choices, colors, spacing, and aesthetic direction are defined there.
Do not deviate without explicit user approval.
In QA mode, flag any code that doesn't match DESIGN.md.

## Current Orientation
**Where we are**: 核心 IM 功能稳定（消息、频道、DM、看板、搜索）。Agent runtime 可用（provision → poll → AI 处理 → 回复）。WebUI v2 活跃开发中。Workspace **github 模式**已落地：PAT 粘贴 → `/git/init` → clone github remote → daemon 推断身份。sync_loop 有 auth 熔断。WebUI **自升级**已落地：右上角黄色 ⚠ 检测新版本,点击一键触发 `POST /runtime/update-and-restart` → runtime fork-exec 自己换三个 binary。**Agent 配置可编辑**已落地：detail 页 Edit 模式可改 `system_prompt` / `env` / `.env` 文件（via `PATCH /workspaces/{slug}/agents/{id}`）；`.env` 文件落 `<agent-clone>/.env`（chmod 0600、64KB 上限），workspace `/git/init` 自动把 `.env` 加到仓库 `.gitignore`（幂等，用 `system@gitim` 作者 commit）；provider/model 仍 immutable。
**Where we're going**: Agent 自治能力（steering、coordinator prompt）、多 provider 支持（GitLab/Gitea）、Token rotate UI、WebUI 完善、update 失败 fallback 机制、provider/model 修改（需 session 迁移方案）
**Learnings**: AI 辅助开发时，模型倾向于保留旧测试不破坏，导致僵尸函数和空壳测试存活。需要定期审计测试有效性。Serde 的 `Option<Option<T>>` + `#[serde(default)]` 不能天然区分"字段缺省"和"字段 = null"—— 两者都解析成 `None`，三态语义需要自定义 deserializer 用 `Value` 中转（见 http.rs `deser_triple_option`）。
**Tensions**: poller 集成测试依赖真实 daemon，环境敏感；codex provider 仍有 stub 代码；daemon 用 curl 调 GitHub `/user`（runtime 用 reqwest），两套 HTTP stack 是已知不一致，未来统一；update-and-restart endpoint 继承 permissive CORS,整站 CSRF 是 known risk；PATCH agent 的 me.json 写 + `.env` 写是**顺序而非事务**（无 WAL），`.env` 写失败时 me.json 已更新，客户端收到 500，靠幂等重试恢复。
