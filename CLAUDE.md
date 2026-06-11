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
| `gitim-runtime` | Agent 生命周期管理、polling、HTTP API、CLI 子命令 | `agent`（provision）, `agent_loop`（消息检测 → AI 处理 → 回复）, `poller`, `preflight`, `http`（WebUI API）, `cli`（agent-facing 命令工具 → HTTP thin wrapper）|
| `gitim-agent-provider` | AI 提供商抽象层 | `claude`（Claude CLI）, `codex`（Codex CLI，部分 stub）, `mock` |

### 产品

| 目录 | 状态 | 说明 |
|------|------|------|
| `products/gitim/frontend/` | **当前主线** | gitim Web 前端（gitim.io）— React 19 + Vite + Radix UI + Tailwind + Zustand |
| `products/gitim/backend/` | **当前主线** | gitim API 后端 — Cloudflare Worker + D1/KV |

### 遗留 / 不要修改

| 目录 | 说明 |
|------|------|
| `webui/` | 早期 React 原型 |
| `webui/legacy_client/` | 旧版 Node.js bridge server |
| `legacy/cli/` | 旧版 TypeScript CLI（`@gitim-runtime/cli`），已被 Rust `gitim-cli` 取代 |
| `legacy/packages/` | 旧版 npm 包 |
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

## Rust toolchain policy

仓库根 `rust-toolchain.toml` 把 channel 锁到 **stable**。这是硬线,因为:

1. **Release reproducibility**: release.sh 跑 4 target 交叉编译,nightly 每日飘,rustc commit 不稳 → 用户装的 binary 行为不可复现
2. **Cross-compile 兼容性**: `cross-rs/cross` + rustup 1.28 在 nightly-host 下无法 provision matching nightly Linux-host toolchain 到容器里
3. **Library 代码可移植**: 未来走 WASM / 新贡献者进来,stable 能编是基本假设

### 规则

- **禁止 library code 依赖 nightly-only feature**。典型坑:
  - `str::floor_char_boundary` (unstable `round_char_boundary`)
  - `build-std` / `panic_immediate_abort`
  - async closures / async fn in trait 的 nightly 语义
  - 任何需要 `#![feature(...)]` 的东西

  想用类似功能 → 写 stable 等价实现(如 `floor_char_boundary` 其实 4 行 loop + `is_char_boundary()` 就能做)。

- **Maintainer dev 自由用 nightly**:`cargo +nightly <cmd>` 显式 override。`rust-toolchain.toml` 只 pin repo 默认,不绑死 CLI override

- **`release.sh` 保留 `+stable` 作为第二道锁**,即使 `rust-toolchain.toml` 被误改,release pipeline 仍然 pinned

- **WASM 路线不受影响**: `wasm32-unknown-unknown` / `wasm32-wasip{1,2}` 全部 stable 支持,`wasm-bindgen` / `wasm-pack` / `Leptos` / `gix` 等生态都 stable。切 WASM 的工作在 IPC / storage / git 层,不在 toolchain

### 为什么记这条(历史背景)

2026-04 首次跑 4-target cross-compile dry-run 时,发现 `agent_loop.rs` 用了 `floor_char_boundary` (nightly-only),maintainer 的 nightly dev 环境下编得过,切 stable 秒挂。这类"无意 leak"只有在项目强制 stable 时才能早发现。

## Pre-commit hook

新 clone 后,**在主 checkout 下**跑一次(不要在 worktree 里跑;symlink 会指向 worktree 路径,worktree 删了 hook 就坏):

```bash
scripts/install-hooks.sh
```

在 worktree 里测试 hook 可加 `--force` 强制安装。

会把 `scripts/hooks/pre-commit` symlink 到 git-common-dir 的 hooks 目录(worktree-safe,一次装所有 worktree 共享)。每次 `git commit` 自动跑:

1. `cargo fmt --all -- --check` — 不通过则拒绝 commit
2. `cargo clippy --workspace --all-targets --no-deps --locked` — error 则拒绝 commit

冷启 30s+,sccache 增量秒级。

紧急逃生(慎用):`git commit --no-verify`。

Lint 规则在根 `Cargo.toml` 的 `[workspace.lints]` 单点维护(`clippy::all = deny`、`dbg_macro` / `print_stdout` / `todo` / `unimplemented` = deny、`unwrap_used` / `expect_used` / `panic` = warn)。新 crate 加进 workspace 时,在它的 `Cargo.toml` 末尾加 `[lints]\nworkspace = true` 继承。

## 测试

```bash
cargo test                                    # 全量（700+ tests，数分钟级别，贵）
cargo test -p gitim-core                      # 核心类型/解析
cargo test -p gitim-daemon                    # daemon handler 集成测试
cargo test -p gitim-sync                      # git 同步逻辑
cargo test -p gitim-runtime --test poller     # poller 集成测试（需编译 daemon）
cargo test -p gitim-runtime --test cli_status # CLI subcommand 集成测试（每个 subcommand 一个 test target）
cargo test -p gitim-runtime --bin gitim-runtime # CLI argv 解析 / 模式分发的 in-process 测试
```

### 跑测试的节奏（重要）

**全量 `cargo test` 是一个昂贵操作**（700+ 测试、含启动真实 daemon 的集成测试，耗时以分钟计）。在多 agent / subagent / 长任务流程里频繁触发会把总时长拖得非常夸张。

**默认只跑 scoped 测试，不跑全量**：
- 只跑相关 crate / 相关 `--test` 目标 / 相关 `#[test]` 过滤（`cargo test -p <crate>`、`cargo test <name_substring>`、`cargo test --test <file>`）
- 跑前先想清楚改动落在哪个 crate / 哪些 test target，针对性触发
- Subagent / 并行任务里同样原则

**只有这两种情况跑全量**：
1. 用户明确要求（"跑一下全量"、"跑 cargo test"、"全跑一次确认 regression"）
2. 改动跨 crate、涉及共享类型 / 协议、或动了 workspace 级依赖 / build script —— 这种情况下 scoped 测试覆盖不到的连带回归只能全量兜底

不要在任务开头 / 末尾"惯性"跑全量。相信 scoped 测试。

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
**Where we are**: 核心 IM 功能稳定（消息、频道、DM、看板、搜索）。Agent runtime 可用（provision → poll → AI 处理 → 回复）。WebUI v2 活跃开发中。Workspace **github 模式**已落地：PAT 粘贴 → `/git/init` → clone github remote → daemon 推断身份。sync_loop 有 auth 熔断。WebUI **自升级**已落地：右上角黄色 ⚠ 检测新版本,点击一键触发 `POST /runtime/update-and-restart` → runtime fork-exec 自己换三个 binary。**Agent 配置可编辑**已落地：detail 页 Edit 模式可改 `system_prompt` / `env` / `.env` 文件（via `PATCH /workspaces/{slug}/agents/{id}`）；`.env` 文件落 `<agent-clone>/.env`（chmod 0600、64KB 上限），workspace `/git/init` 自动把 `.env` 加到仓库 `.gitignore`（幂等，用 `system@gitim` 作者 commit）；provider/model 仍 immutable。**Hermes provider per-agent profile 隔离**已落地：每个 hermes agent 自动获得独立的 `~/.hermes/profiles/gitim-<handler>/` 目录,LLM 配置 / auth.json / sessions / cron / gateway PID 完全隔离;user 一次性跑 `hermes setup` 配 default profile,新 agent 通过 `hermes profile create --clone --no-alias` 自动继承,WebUI 零额外步骤。**Hermes 多 LLM 选择**已落地：WebUI 加 hermes agent 时可选具体 LLM provider × model;后端 introspect `~/.hermes/.env` + `config.yaml.custom_providers` 列出已配 provider,live-fetch `/models` 拉模型列表,创建 profile 后顺序 `hermes config set model.{provider,default,base_url}` 写入。回滚保证：任一 config-set 步骤失败 → delete_profile + cleanup_agent_dir,无半残状态。**Per-agent token usage 统计**已落地:Provider trait 加 `reports_usage()` / `usage_is_cumulative()` 让各 provider 声明语义;agent_loop 在 `update_session_usage` 末尾 normalize(cumulative provider 走 saturating_sub baseline 在 `AgentState.last_session_usage`)+ accumulate 到 `<workspace>/.gitim-runtime/usage/<handler>.json`(每日聚合 + 全历史 totals,90 天滚动,chmod 0600,跟 session reset 物理解耦);reset path 也走 accumulator 不丢 turn;HTTP `AgentInfo.usage_summary` 暴露 30 天 by_day 窗口,SSE `usage` event 保 SessionUsageSnapshot 字段 inline + 加 `usage_summary` sibling(老 frontend 兼容);hard delete 同步删 usage 文件;`/runtime/health` 暴露 `usage_save_failures` AtomicU64;WebUI 在 detail 页加 `AgentUsageCard`、list 行加 `AgentUsageTag`、agents 顶部加 `WorkspaceUsageHeader`(客户端 reduce 跨 agent,按 provider 分组),所有 sparkline 复用 `lib/sparkline.ts`。**gitim-index 改为 opt-in**：per-clone 配置 `indexer.enabled`（默认 false），agent daemon 停掉后台 FTS5 索引；CLI/Runtime human onboard 路径显式写 `true` 保留搜索能力；旧 `.gitim/index.db` 文件不动；`gitim search`/`reindex` 在 disabled clone 上返回带 actionable 指引的 error。**Cards 跟随 channel archive** 已落地：`archive_channel` 把 `channels/<ch>/cards/` 整个子目录 mv 到 `archive/channels/<ch>/cards/`,同时把每张卡的 `card.meta.yaml` 标记 `archived_via=channel`;`archive_card` 单独归档时标记 `archived_via=manual`。`unarchive_channel` 按字段筛选,只复活 `archived_via=channel` 的卡片,manual 归档的留在 archive。Rust daemon(`gitim-daemon/src/handlers/channel.rs` + `card_handlers.rs` + `reconcile.rs`)和 frontend daemon-web(`products/gitim/frontend/src/daemon-web/handlers.ts`)双端一致。`gitim-core::CardMeta.archived_via: Option<ArchivedVia>` 用 `#[serde(default, skip_serializing_if = "Option::is_none")]` 保后向兼容(旧 yaml 缺字段 → None,active card yaml 不污染)。**启动 reconcile**:daemon `AppState::new` 之后、frontend worker `init` 成功之后(失败不跑),各调用一次 `reconcile_orphan_cards` —— 扫 `channels/<archived-ch>/cards/`(legacy archive_channel 只 mv channel meta+thread 留下的孤儿),批量 mv 到 archive 并标记 `archived_via=channel`,单 commit 由 `system` 作者发起;无孤儿则 no-op 不 commit。daemon reconcile + archive_channel/unarchive_channel 都持 `commit_lock` 跟其他 writer 串行(`std::sync::MutexGuard` 在 `thread_cache.write().await` 前显式 drop 规避 !Send)。**Daemon log 测试隔离**已落地：production 仍把所有 per-daemon log 集中在 `~/.gitim/logs/<workspace>-<handler>.log`(`tail -f *.log` 一眼看全部 agent)。`daemon_log::logs_dir()` 加了 `GITIM_LOG_DIR` env override，**production 不设这个 env，测试 infra 设**：`tests/common/mod.rs::ensure_daemon_in_path` 用 `Once::call_once` 一次性把 GITIM_LOG_DIR 指到 process-wide TempDir，所有 spawn-daemon 的测试只要 call 它就自动 isolated。之前 41 个集成测试只有 6 个 install HomeGuard，其余直接污染真实 `~/.gitim/logs/`(2000+ test log)，且 `std::env::set_var("HOME")` 在 cargo test 多线程下本来就 race，单 env override + `Once` 保护比 per-test HomeGuard 干净。同时修了一个长期 bug：旧 workspace 推导走 `parent().parent()` 在 agent 平铺 layout (`<ws>/<handler>/`) 下越界，新版本按 `.gitim-runtime/` 中间层显式区分 human 和 agent。**gitim-runtime CLI 子命令**已落地：单 binary 双模式（无 arg = HTTP server，有 subcommand = 本机 runtime HTTP 的 thin wrapper），8 个 subcommand `status/runtime-id/workspaces/list-agents/add-agent/burn-agent/update-agent/preflight` 让 agent 通过 shell-out（Bash tool）能管本机 runtime 上的 agent；`list-agents` 默认 redacted（隐 repo_path/system_prompt/env/usage），`--detailed` 走 `AgentDetail` 但 env 走 `redact_env_secrets` 把含 KEY/TOKEN/SECRET/PASSWORD/API/AUTH 的 value 换成 `<redacted>`；exit code 0/1/2/3 分档（0=success / 1=CLI internal+network / 2=permanent server error_code / 3=transient 5xx）让 agent 能 reason about retry；CLI 端 typed wire DTO 跟 runtime lib `AgentInfo` 解耦（lib 大部分 response struct private 且 Serialize-only）；`runtime.json` 加 `listen_port` 字段做 CLI port discovery（bind 后 best-effort 写盘，CLI 读它优先于 default 16868）；legacy positional-arg agent mode（`<remote_url> <handler> <display_name>`）退役，clap subcommand 占据 argv 空间。CORS permissive 是已知 pre-existing risk，scope 外不修。spec 见 `docs/specs/runtime-cli.md`，design 见 `docs/plans/runtime-cli/00-requirements.md`。**Provisioning preflight gate**已落地：`POST /agents/add` 在 `handler_conflict` 检查后、`provision_agent` 调用前，调一道 server-side preflight gate 用 add request body 的 `env` / `model` / `llm_provider` / `llm_model` 调对应 provider 的 `preflight_X_with_config`，验"this specific agent 真能跑 LLM hello"。失败 → 返 `ErrorBody { error_code, preflight_detail: PreflightResult }`，**零 durable agent artifact**（没 commit / 没 push / 没 agent_dir / 没 state entry），因为 gate 在所有 side-effect 之前。Hermes 走 backward-compat 路径：双值 llm_provider/llm_model → chat-mode w/ overrides；双缺 → 读 `$HERMES_HOME/config.yaml` 的 `model.default` + `model.provider` 回填走 chat-mode（验 default profile 的 LLM）；缺一个 → `missing_llm_provider`；default profile 没 LLM → `hermes_default_profile_no_llm`。Claude/Codex `--model X` agent-aware；opencode/pi 只验 connectivity（CLI 无 per-invocation model flag，model 名拼错仍在第一 turn 才暴露 —— known limitation）。Mock provider 顶部 short-circuit `PreflightResult::success`，不 shell out。Outer timeout 90s (`PROVIDER_PREFLIGHT_TIMEOUT`)，比 LONG_REQUEST_TIMEOUT 300s 紧。CLI 端 `CliError::ResponseErrorCode` + `dto::ErrorResponse` 同步加 `preflight_detail` 字段；`run_cli` 在 stderr 输出结构化 "Preflight (provider):" block 含 error_kind / version / model / output_preview / detail 帮 agent 用 regex grab；exit code 仍 2（permanent）。WebUI 删 client-side `Detect` 按钮（add-agent-dialog.tsx），"Add agent" 一键直达 server preflight；失败时 inline 展示 preflight_detail 的 friendly error_kind label + 折叠 output_preview。`ErrorBody` 加 `preflight_detail: Option<PreflightResult>` (skip_serializing_if Option::is_none) 纯加性扩；旧 caller 不破。spec 见 `docs/specs/runtime-cli.md` "Provisioning preflight" 段，design 见 `docs/plans/provisioning-preflight/00-requirements.md`。**Agent routing v1** 已落地:daemon 在 poll 返回时给每条 channel / DM message entry 附 `recipients: [handler...]` —— channel 走 3 条规则的 union(`ChannelMeta.created_by` 当群主 + parent chain 上溯所有 author + 显式 `<@mention>`),DM 直接是双方 handler;event entry 永不带 recipients。Runtime `format_changes_as_prompt` 加一步过滤:`author == self_handler` 跳过(原有)+ recipients 非空且不含 self 跳过(新)。Empty / 缺失 `recipients` 走 broadcast fallback,覆盖三种情况:旧 daemon、card_thread / cron_thread(v1 不路由)、daemon 端 `compute_recipients` 空集 warn 后的兜底。Cascade(N agent 同时处理同一条用户消息)由此 cap 在 recipients 命中的 agent 集合;agent-agent 长对话深度收敛、群主转让、per-agent `responds_to` opt-out、card 路由等是 non-goal,留给 v2。`compute_recipients` 是 `gitim-core::recipients` 的纯函数(零 IO、cycle 防御 + 缺失 parent 兜底),BTreeSet 自动 sorted-dedup,wire 类型 `Vec<String>` 跟 ChannelMeta 字段对齐。spec / plan 见 `docs/plans/agent-routing/{00-requirements,01-plan}.md`。**Team Flows v1** 已落地:`flows/<slug>/index.md` 模板系统 — frontmatter 描述 DAG + body section 给节点 prompt;daemon `flow_handlers` + recursive file watcher + 软删除到 `.trash/`;`gitim flow list/show/create/rm/validate` 子命令;`gitim-runtime` 暴露 `/im/flows` HTTP gateway;WebUI Flows tab(lazy-loaded mermaid DAG + react-markdown)+ "Run this flow" 复制 `@coordinator 用 <slug>` 到剪贴板;agent system prompt 在 `default_gitim_api()` 自动暴露 flows API。Phase 2 fork instance + executor + conditional 留 schema 位但 v1 不实现。**Team Flows v1.5(runs+state)** 已落地:`flows/<slug>/runs/<run_id>/state.yaml` 记录每次具体执行的状态 —— `run_id` 格式 `YYYYMMDDTHHMMSS-XXXXXX`、必绑一个 channel(1 run ↔ 1 channel,1 channel ↔ 0..N runs)、节点 5 状态机 `pending → in_progress → done | failed | skipped`(只前向);run 4 状态机 `in_progress → done | failed | cancelled`(daemon 在所有节点终态时自动 flip)。`NodeStatus::as_str() / RunStatus::as_str()` 保证 event wire format 跟 serde snake_case 严格一致(regression test 锁住)。CLI:`gitim flow start --channel`、`runs`、`run-show`、`node-set`、`run-cancel`。Runtime HTTP:`POST /im/flows/:slug/runs`、`GET /im/runs?slug=&channel=&status=`、`GET /im/runs/:rid`、`PATCH /im/runs/:rid/nodes/:nid`、`DELETE /im/runs/:rid`(write 端点返非 2xx,not_found → 404、其他 → 422)。WebUI:channel 顶部 active runs pill 横条(只显 in_progress)、flow detail 底部 Recent runs(最近 10)、`/runs/:run_id` 详情页(mermaid DAG + per-node status color);is_write guard 守护 Start/NodeSet/Cancel,departed-user 检查也覆盖三个 mutation。Agent prompt 在 `default_gitim_api` 的 Flows 段后追加了 run lifecycle 契约。**悬空缓解**:配 cron 让 coordinator 定时扫 in_progress runs 的 updated_at,超阈值的 escalate;`flow_runs.list --status in_progress` 是 watchdog 的核心 query。Phase 2 真正的 executor + conditional 路由 + WebUI 内编辑 仍 v2 留位。**Oneshot timer** 已落地:agent 用 `gitim timer set <duration> <anchor> [--note]` 注册一次性提醒,状态存 `<agent_clone>/.gitim/timers.json`(gitignored),agent_loop 每 cycle pop 到期项并把 "## ⏰ Timer reminder(s) fired" prefix 注入 LLM prompt。零新 IPC、零新 tokio task、零 git commit;F1 用单独的 `.gitim/timers.json.lock` 做 lock anchor(防 atomic rename 把 inode-锁的 lock 抹掉),F5 用 `NamedTempFile::persist` 保证 write 失败不留 tmp 残骸。cap 每 agent 3 个 pending。design 见 `docs/plans/oneshot-timer/`。**Unified labels space v1** 已落地：CardMeta.labels（保持）/ BoardMeta.labels（原 tags rename + serde alias 兼容 + 移除 deny_unknown_fields）/ UserMeta.labels（新增）/ FlowNode.required_labels（新增，仅信息位不强制 routing）全部走 `gitim-core/src/types/labels.rs` 共享 validator（char set `a-z 0-9 - _`、单 label 32 char、各对象 max_count：card 10 / board 20 / user 20 / flow_node 10）。**eng-review Issue #1**：BoardMeta 移除 `deny_unknown_fields` 让新 daemon 写 `labels:` 时老 daemon fetch 不挂；**Issue #2**：`set_board_field` 同时接受 `"tags"` 和 `"labels"` arg 路由到同一字段；**Issue #3**：`LabelsAdd / Remove` read-modify-write 在 `commit_lock` 内,rollback yaml on commit fail（参考 `card_handlers::archive_card` pattern）。Daemon 4 个 IPC：`LabelsAdd / LabelsRemove`（self-claim only，验 `target == state.current_user`，否则 `error_code: not_self`）+ `LabelsList`（拒 departed handler，返 404 `unknown_user`）+ `AgentsWithLabels`（all-of subset，排除 `archive/users/`，empty query → empty result）。`handle_create_card` push 完成后调 `compute_suggested_assignees` 填 `CreateCardResponse.suggested_assignees`（best-effort，scan 失败 = `[]`，不阻塞 card 创建）。CLI 新加 `gitim labels add/remove/list/match`；Runtime HTTP `GET /im/labels/{handler}` + `POST/DELETE /im/labels` + `GET /im/agents-with-labels?labels=a,b`（写端点用 runtime 自身 me.json 推断 `<self>` handler——M7：runtime 信任自己的 human me.json，不接受任意 path 参数）。WebUI read-only chip 待 v1.5 加。Frontend wire 字段 `BoardMetaSummary.labels` / `BoardSummary.labels` 同步 rename + serde alias `"tags"` 兜底。spec / plan 见 `docs/plans/unified-labels/`。**Epoch auto-rotation (Snapshot Pack Phase B v2)** 已落地:daemon 在 on_pushed 后(60s throttle)数当前分支 commit 数,过阈值(默认 1M,`GITIM_ROTATION_THRESHOLD` 覆盖)自动 fire——orphan snapshot 开 `main-epoch-{N+1}` + 老分支 seal redirect commit,`git push --atomic` 双 ref 仲裁单 winner,loser 清理本地残留转 follow。三条协议不变量:sealed tip 永远是 R / atomic push 是唯一仲裁 / 判定一律以 origin 为准(不信本地残留)。sync_loop 三处 push-fence(direct push 前 / fetch 后 rebase 前 / rebase 后 push 前)保证消息永不发布到 sealed branch;fence 对 corrupt epoch.yaml fail-closed,并带自愈分支(HEAD redirected ∧ origin active → 重试 cleanup)。被拦消息经 `rebase --onto` migrate 到新分支;migrate 冲突降级走 capture → discard → clean follow → `conflict::resolve_content` renumber 重放(meta/board 增量在此路径让位,last-writer-wins)。零丢失守卫:fire 前置 `has_unpushed` + dirty-tracked-files 双 gate;cleanup reset 前验证 ahead-of-origin 全部是 `seal: redirect` 自产 commit 且无 dirty file。follow 多跳(max 32)一次到位,切换后 set-upstream 保 sync 可发布;boot 时清半成品 fire 残留(commit_lock 串行)。winner 落本地 bundle(`.gitim/archive/epoch-N.bundle`)+ archive tag(均 best-effort)。daemon-web v1 只做只读拦截:sync 检测 redirected epoch.yaml → latch + 16 个写 handler 拒 `epoch_redirected` + 永不 push sealed branch(conflict-resolve 的 commit 保留本地待 daemon migrate)。`/runtime/health` 暴露 `workspace_epochs`(per-workspace epoch,O(1);commit count 故意不进热轮询端点)。运维约束:混版本 workspace 不支持(老 daemon 无 fence)/`main-epoch-*` 保留命名空间/业务分支勿加 branch protection。竞态矩阵与零丢失论证见 `docs/plans/git-history-snapshot-pack/03-phase-b-v2-design.md`,plan 见同目录 04。**Channel Project Grouping v1** 已落地:channel 上加一层可选 project 归属做 sidebar 分组,纯管理语义 —— routing / permission / archive / flows / cards / index 全部不动(dispatch path audit 表见 `docs/plans/channel-project/audit-notes.md`,唯一碰 `projects/` 的 path-agnostic glob 是 sync conflict-resolver,走 type-agnostic last-writer-wins 分支,行为正确)。数据模型:`ChannelMeta.project: Option<String>`(`#[serde(default, skip_serializing_if)]` 保 backward-compat)+ 顶层扁平 `projects/<slug>.meta.yaml`(`ProjectMeta` 四字段对齐 ChannelMeta)+ `ProjectSlug` newtype(ChannelName 同款字符集 + reserved set)。Daemon 3 个 IPC:`CreateProject`(slug/meta 校验,`project_exists`)/ `SetChannelProject`(assign/clear/reassign 单接口,验 `project_not_found` / `project_meta_corrupted` / `channel_archived`,archive 周期保留 project 字段有 regression test 钉住)/ `ListProjects`(`channel_count` 派生,archive 不计,wire shape `{slug, meta: {...}, channel_count}` nested,raw-JSON 断言钉住);mutation 成功后 handler 推 SSE event(`project_created` / `channel_project_changed`,失败不推 —— 对齐 card_created convention,watcher 对 meta 类变更照旧不推);`ChannelSummary` 加 `project` 字段(纯加性,None 不序列化)。CLI:`gitim projects list/create`(`--intro` 必填,daemon 强制 1-500 字符)+ `gitim set-channel-project <ch> <slug>|--clear`(flat 顶层命令对齐现役惯例)。Runtime HTTP:`GET/POST /im/projects` + `PATCH /im/channels/{ch}/project`(200 + `ok:false` body 的 api_response_to_json convention;写守卫 daemon 端 enforce,runtime 原样转发)。WebUI:sidebar 平级 mixed sort(`buildSidebarTree` 纯函数:pinned 先 + 字典序,空 project 隐藏,孤儿 project 引用归 unassigned;**替换了旧 unread-first 排序**),project folder 默认折叠 + localStorage 持久化,pin 扩展 `gitim-pinned-conversations` schema 加 `projects[]`(旧值缺 key 不崩);cards filter bar 加单选 project dropdown + URL param `project=` / `__unassigned__`(post-filter,kanban 列不动)。**v1 mutation 入口 = CLI / HTTP only**,WebUI 无 create/assign 入口(by design,§2/§8);browser mode 全降级(listProjects 空、写拒,daemon-web `channels()` 透传 project 字段但可见行为仍平铺)。Playwright E2E(`e2e/channel-project.spec.ts`,opt-in 不在 test:e2e script)抓到并修了 ProjectItem 双 onClick bug;顺手修了 vitest 收集 playwright spec 的 pre-existing 配置缺陷(vite.config.ts `test.exclude e2e/**`)。v2+ 锁死不做:rename / archive / 嵌套 / 多归属。design / plan / audit 见 `docs/plans/channel-project/`。
**Where we're going**: Agent 自治能力（steering、coordinator prompt、agent 自主用 runtime-cli 招募新 agent）、多 provider 支持（GitLab/Gitea）、Token rotate UI、WebUI 完善、update 失败 fallback 机制、provider/model 修改（需 session 迁移方案）、opencode/pi 加 model arg 支持（让 preflight 也能验它们的 model 名）、agent routing v2(群主转让 / per-agent responds_to / agent-agent cascade 深度收敛 / card-thread 路由)、epoch rotation v2(新 clone single-branch 优化 / bundle 上传 / auto-prune 老 epoch / WebUI epoch 状态与 banner / browser 端完整 follow)、channel-project v1.5+(WebUI create/assign 入口 / daemon-web listProjects 点亮 browser 分组 / project rename·archive)
**Learnings**: AI 辅助开发时，模型倾向于保留旧测试不破坏，导致僵尸函数和空壳测试存活。需要定期审计测试有效性。Serde 的 `Option<Option<T>>` + `#[serde(default)]` 不能天然区分"字段缺省"和"字段 = null"—— 两者都解析成 `None`，三态语义需要自定义 deserializer 用 `Value` 中转（见 http.rs `deser_triple_option`）。`dirs::home_dir()` 推导用户 artifact 路径是 anti-pattern：cargo test 多线程下 `std::env::set_var("HOME")` RAII guard 有 race，再加上很多测试根本没 install guard，整个 home 会被测试持续污染。根治方法是让 path 从 caller 已经持有的根（workspace / repo_root）派生，不去碰 home。
**Tensions**: poller 集成测试依赖真实 daemon，环境敏感；codex provider 仍有 stub 代码；daemon 用 curl 调 GitHub `/user`（runtime 用 reqwest），两套 HTTP stack 是已知不一致，未来统一；update-and-restart endpoint 继承 permissive CORS,整站 CSRF 是 known risk；PATCH agent 的 me.json 写 + `.env` 写是**顺序而非事务**（无 WAL），`.env` 写失败时 me.json 已更新，客户端收到 500，靠幂等重试恢复。Hermes profile 集成依赖 shell out `hermes profile create/delete`,如果 hermes major 版本改了 profile 内部结构,我们的 `--clone` 会自动跟进(它是 hermes 的承诺),但 `default_profile_ready` 的 `.env`/`auth.json` 探测路径假设可能 drift,每个 hermes 大版本回归一次。

## Hermes profile 隔离机制

每个 gitim agent 1:1 对应一个 hermes profile,profile 名为 `gitim-<handler>`,放 `~/.hermes/profiles/gitim-<handler>/`。

**Provision 路径**(`http.rs` add_agent flow):
1. 写 `me.json` 后,如果 `provider == "hermes"` 才走下面
2. `hermes_profile::default_profile_ready()` — 检查 `HERMES_HOME` 或 `~/.hermes/` 下 `.env` 或 `auth.json` 是否存在;不存在则拒绝 add(`error_code: hermes_not_setup`)
3. `hermes_profile::ensure_profile(handler)` — shell out `hermes profile create gitim-<handler> --clone --no-alias`,从 user 的 active profile 拷 `config.yaml` + `.env` + `SOUL.md` + `memories/`,自带 70 bundled skills sync(几秒钟);失败 → `cleanup_agent_dir` + `error_code: hermes_profile_create_failed`
4. agent_loop 启动时 `build_provider_config` 自动注入 `HERMES_HOME=<profile_dir>` 到 `ProviderConfig.env`(me.json 显式 env 优先)

**Hard delete 路径**:`hard_delete_agent_dir` 后,如果 `provider == "hermes"`,best-effort `delete_profile`(失败仅 warn,不阻塞 user 响应)。soft delete 不动 profile。

**User 切换某个 agent 的 LLM**:用 hermes 原生 CLI,如 `hermes -p gitim-alice setup model` / `hermes -p gitim-alice login`。WebUI v1 不暴露,user 直接终端操作。

**已知 non-goals(v1)**:
- WebUI 暴露 hermes profile 概念(profile 名由后端推导,前端零感知)
- 多 source profile 选择(永远从 active profile clone)
- profile 重命名 / 跨 agent 迁移(handler immutable,profile 跟随 handler)
- soft delete 时清理 profile(soft delete 保留所有 agent 数据)
- 已有 agent 的 retroactive profile 创建(只对新加 agent 生效,迁移见 `docs/plans/hermes-profile-isolation/migration.md`)
- **OAuth 类 LLM provider**（Nous / openai-codex）—— v2 处理,涉及 auth.json clone 和 active_provider 切换
- **已有 agent 的 retroactive LLM 配置** —— 用户手动 `hermes -p gitim-<h> config set` 迁移
- **创建后编辑 LLM** —— 涉及 hot-reload + session-migration 语义,单独立 plan
- **`BUILTIN_PROVIDERS` 表跟 hermes 源码 CI 同步校验** —— 半年人工 PR

**实现位置**:`crates/gitim-runtime/src/hermes_profile.rs`(模块) + `agent_loop.rs::build_provider_config`(env 注入) + `http.rs` add_agent / agents_remove(provision / cleanup wiring) + `preflight.rs::preflight_hermes_with`(可选 `hermes_home` 参数)。
