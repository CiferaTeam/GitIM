# Workspace GitHub Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Runtime 的 `/git/init` 端点支持 GitHub provider，让 WebUI 用户能用 PAT 从 github remote 起一个 GitIM workspace，后续多 agent 共用 workspace 级 PAT。

**Architecture:** Remote 模式下跳过本地 bare repo，human/agent clone 直接挂 github 为 origin。Token 集中存 `$workspace/.gitim-runtime/config.json`，派生到每个 clone 的 `.git/config` URL。身份从 GitHub `/user` API 自动推断，无手工覆盖入口（v1）。

**Tech Stack:** Rust（runtime、reqwest），React / TypeScript（webui-v2）

---

## 设计决策速览（源自 grill 会话）

| # | 决策 | 选项 |
|---|------|------|
| Q1 | Remote 模式**跳过** bare repo 创建。每个 clone 直连 github | 不保留本地镜像 |
| Q2 | 首发只支持 **GitHub**，架构留 provider 抽象 | GitLab/Gitea 未来增量 |
| Q3 | 只 **PAT 粘贴**。不做 `gh` 快捷复用、不做 OAuth、不做 SSH | Fine-grained PAT 单 repo scope |
| Q4 | 身份**纯 API 自动**：token → `GET /user` → handler = login.lower(), display_name = name 或 login | v1 无手工覆盖 UI |
| Q5 | Token 落盘**两处**：`.gitim-runtime/config.json`（中心 source）+ 各 clone `.git/config` URL（派生）。chmod 0600 | 更新入口唯一 |
| Q6 | 只支持 **clone 已有 repo**。不做 "Create new repo" UI。空 repo clone 靠 daemon `ensure_repo` 首次 push 撑起结构 | 用户先去 github 建 repo |
| Q7 | 强制 **fresh clone**。不支持 "import existing local clone" | workspace 目录自包含 |
| Q8 | 所有 agent **共用 workspace PAT**。agent 提交 author 各自 handler（与凭证分离）。github 模式下 `add_agent` 必须做 **handler 冲突检查** | 阻止 split-brain |

**其他实现细节定调**：
- Init 反馈 UX：单响应 + UI 文案轮播（"Verifying... / Cloning... / Onboarding..."），不做 SSE 流式
- 错误分类：返回 `error_code` 字段（`invalid_token` / `repo_not_found` / `repo_access_denied` / `network_error` / `clone_failed` / `onboard_failed`）
- 代码复用：runtime 内直接重写 GitHub clone/verify 逻辑，不抽共享库（YAGNI，加第二个 provider 时再抽）
- 日志脱敏：统一 `redacted_url()` helper，所有 log URL 前过一遍
- Token 过期：v1 不做 UI 恢复，仅日志警告；v2 加 banner + "Update token"

---

## 文件结构

**新增：**

- `crates/gitim-runtime/src/github.rs` — GitHub API client（token verify、identity 提取）
- `crates/gitim-runtime/src/git_config.rs` — `$workspace/.gitim-runtime/config.json` 的读写/schema/redact
- `crates/gitim-runtime/tests/github_init.rs` — github 模式集成测试（用 mockito + 本地 bare repo 伪装 github）
- `crates/gitim-runtime/tests/config_schema.rs` — config schema 读写、redact、chmod 测试
- `webui-v2/src/components/setup/github-setup-form.tsx` — Remote URL + PAT 表单
- `docs/plans/workspace-github-mode/` — 本 plan + 迭代笔记

**修改：**

- `crates/gitim-runtime/src/http.rs` — `GitInitRequest` 扩字段、`git_init` 分支、`add_agent` 读 config 取 provider/token、handler 冲突检查
- `crates/gitim-runtime/src/agent.rs` — `provision_human` 接受 `remote_url`、`provision_agent` 接受 origin URL 而非固定 bare path
- `crates/gitim-runtime/src/bin/runtime.rs` — 初始化 config.json 的新 schema 兼容
- `crates/gitim-runtime/Cargo.toml` — 确认 `reqwest` dep（现有）、`mockito` dev-dep（需要加）
- `webui-v2/src/components/setup/git-provider-form.tsx` — GitHub 按钮 enabled，GitLab 暂隐
- `webui-v2/src/components/setup/setup-gate.tsx` — 加 `github_setup` 状态
- `webui-v2/src/hooks/use-connection-store.ts` — 加 github setup 相关 state
- `CLAUDE.md` — Onboard 流程小节补 runtime github 路径
- `docs/runtime-architecture.md` — 补 remote 模式盘面图、agent provisioning 差异

---

## Tasks

### Task 1: Config schema + redact helper

**Files:**
- Create: `crates/gitim-runtime/src/git_config.rs`
- Create: `crates/gitim-runtime/tests/config_schema.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`（暴露新模块）

**职责：** 抽出 `.gitim-runtime/config.json` 的序列化/读写逻辑，扩 schema 支持 `git` 字段，加 URL redact helper。

- [ ] **Step 1: 写 schema 失败测试**

新文件 `tests/config_schema.rs`，写三个测试：
- `local_mode_roundtrip`：构造只含 `provider: "local"` 的 GitConfig，序列化→反序列化后字段一致
- `github_mode_roundtrip`：构造含 `provider, remote_url, token` 的 GitConfig，roundtrip 一致
- `legacy_config_without_git_field_loads_as_local`：旧版 config.json（无 `git` 字段）反序列化后得到默认 `provider: "local"`，确保向后兼容

**Acceptance：** 测试文件存在，编译未通过（引用了未定义的 `GitConfig` / `GitProvider`）

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p gitim-runtime --test config_schema`
Expected: 编译失败 `unresolved import gitim_runtime::git_config::GitConfig`

- [ ] **Step 3: 实现 GitConfig schema**

在 `git_config.rs` 中定义：
- 枚举 `GitProvider` 含 `Local` 和 `Github` 两个变体，derive `Serialize, Deserialize, Clone, Debug, PartialEq`，serde rename lowercase
- 结构体 `GitConfig { provider: GitProvider, remote_url: Option<String>, token: Option<String> }`，`#[serde(default)]` 保证字段缺失时 provider 默 `Local`
- 结构体 `WorkspaceConfig { workspace: String, created_at: String, git: GitConfig }`（替代现有 http.rs 里的 ad-hoc json）
- 在 `lib.rs` 里 `pub mod git_config;`

**Acceptance：** 三个测试通过

- [ ] **Step 4: 写 redact 测试**

在 `tests/config_schema.rs` 追加：
- `redact_github_token_in_url`：输入 `https://x-access-token:ghp_abc123@github.com/owner/repo.git`，输出 `https://x-access-token:<REDACTED>@github.com/owner/repo.git`
- `redact_leaves_non_token_url_untouched`：输入 `https://github.com/owner/repo.git`（无 token）不变
- `redact_handles_oauth2_prefix`：`https://oauth2:TOKEN@github.com/...` 也被 redact（覆盖 GitLab 的 URL 形式，以备将来）

- [ ] **Step 5: 运行测试验证失败**

Run: `cargo test -p gitim-runtime --test config_schema redact`
Expected: `redacted_url` 未定义

- [ ] **Step 6: 实现 redacted_url**

在 `git_config.rs` 中加 `pub fn redacted_url(url: &str) -> String`：regex 匹配 `(https?://)([^:]+):[^@]+@` → 替换为 `$1$2:<REDACTED>@`。

**Acceptance：** 所有 redact 测试通过

- [ ] **Step 7: 写读写 helper 测试**

追加两个 tokio test：
- `write_config_sets_0600_perms`（仅 unix）：写 config 到 tempdir，`fs::metadata` 读 mode，断言 `mode & 0o777 == 0o600`
- `read_config_from_fresh_workspace`：`WorkspaceConfig::read(path)` 不存在文件 → 返回 `Err(NotFound)`；存在且合法 → 返回正确 struct

- [ ] **Step 8: 实现读写 helper**

在 `git_config.rs` 中加 `impl WorkspaceConfig`：
- `pub fn read(workspace: &Path) -> Result<Self, ConfigError>` 读 `.gitim-runtime/config.json`
- `pub fn write(&self, workspace: &Path) -> Result<(), ConfigError>` 写文件后 `#[cfg(unix)] std::fs::set_permissions(path, Permissions::from_mode(0o600))`
- `ConfigError` 用 thiserror 包装 io / serde 错误

**Acceptance：** 所有 Task 1 测试通过

- [ ] **Step 9: Commit**

Run: `git add crates/gitim-runtime/src/git_config.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/tests/config_schema.rs`
Run: `git commit -m "feat(runtime): add workspace config schema with git provider + redact helper"`

---

### Task 2: GitHub API client（token verify）

**Files:**
- Create: `crates/gitim-runtime/src/github.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`
- Modify: `crates/gitim-runtime/Cargo.toml`（确认 reqwest 可用，加 mockito dev-dep）

**职责：** 封装 GitHub API 调用。现在只用一个端点 `GET /user`，但把"一个 provider 的 API adapter"做成独立模块以便将来扩展。

- [ ] **Step 1: 确认 / 添加依赖**

检查 `Cargo.toml`：
- `reqwest` 已存在（http client）—— 确认 `features = ["json", "rustls-tls"]`
- `mockito` 在 `[dev-dependencies]` 里不存在则添加 `mockito = "1"`

- [ ] **Step 2: 写失败测试 token 无效**

在 `github.rs` 的内联 `#[cfg(test)] mod tests` 里：
- `verify_token_returns_invalid_when_401`：mockito 起 server，stub `GET /user` 返回 401 → 调 `verify_token(token, base_url)` → 断言返回 `Err(GithubError::InvalidToken)`

**Acceptance：** 测试文件存在，编译失败（`verify_token` / `GithubError` 未定义）

- [ ] **Step 3: 实现 GithubError + verify_token**

`github.rs` 里：
- 枚举 `GithubError { InvalidToken, InsufficientScope, NetworkError(String), ParseError(String) }`，derive `Debug`
- 结构体 `GithubIdentity { pub login: String, pub name: Option<String> }` derive `Deserialize`
- `pub async fn verify_token(token: &str, api_base: &str) -> Result<GithubIdentity, GithubError>` 用 reqwest 调 `{api_base}/user`，header `Authorization: Bearer {token}` + `User-Agent: gitim-runtime`，响应 200 → parse `{login, name}`，401 → InvalidToken，403 → InsufficientScope，其他 → NetworkError
- `api_base` 参数化是为了测试时指向 mockito

**Acceptance：** 401 测试通过

- [ ] **Step 4: 补全测试集**

追加：
- `verify_token_returns_identity_on_200`：stub 200 `{"login":"alice","name":"Alice Wang"}` → 断言返回 `Ok(GithubIdentity { login: "alice", name: Some("Alice Wang") })`
- `verify_token_handles_null_name`：stub 200 `{"login":"bob","name":null}` → 断言 `name: None`
- `verify_token_returns_insufficient_scope_on_403`
- `verify_token_returns_network_error_on_unreachable`：mockito server drop 掉再调 → NetworkError

**Acceptance：** 5 个测试全过

- [ ] **Step 5: Commit**

Run: `git add crates/gitim-runtime/src/github.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/Cargo.toml`
Run: `git commit -m "feat(runtime): add github API client with token verify"`

---

### Task 3: Generalize provision_human to accept remote URL

**Files:**
- Modify: `crates/gitim-runtime/src/agent.rs:39-110`（`provision_human` 函数）
- Modify: `crates/gitim-runtime/src/http.rs`（调用点）

**职责：** 把 `provision_human` 内部硬编码的 `workspace.join("repo.git")` 参数化成 `remote_url: &str`，local 模式调用方传 bare 路径，github 模式传 token URL。不引入新功能，纯重构。

- [ ] **Step 1: 跑现有 provision 测试建立基线**

Run: `cargo test -p gitim-runtime --test provision`
Expected: 全过（确认改前 baseline）

- [ ] **Step 2: 改 provision_human 签名**

`agent.rs:39` 处：
- 签名从 `pub async fn provision_human(workspace: &Path) -> Result<PathBuf, RuntimeError>` 改为 `pub async fn provision_human(workspace: &Path, remote_url: &str, git_server: &str, auth: serde_json::Value) -> Result<PathBuf, RuntimeError>`
- 内部 `git clone` 用 `remote_url` 而不是 `bare_repo.to_string_lossy()`
- `onboard` 调用里的 `"git"` 参数替换为传入的 `git_server`，`auth` 参数替换为传入的 `auth`
- `detect_git_config(workspace)` 推断身份那段，只在 local 模式下走；github 模式下用调用方传入的 auth（body 里 `{ "token": "..." }`）

- [ ] **Step 3: 修 local 模式调用点**

`http.rs:195` 的 `git_init` handler 里 local 分支，传 `bare_repo.to_string_lossy().as_ref()` 作为 `remote_url`，`git_server="git"`，`auth=json!({"handler": handler, "display_name": display_name})` —— 从 `detect_git_config` 结果构造。

**注意：** local 模式下身份推断**保持在调用方** —— `git_init` 调 `detect_git_config` 得到 handler/display_name，再传给 `provision_human`。这让 `provision_human` 职责变纯：接 URL 和 auth，不管推断。

- [ ] **Step 4: 编译 + 跑既有测试**

Run: `cargo test -p gitim-runtime`
Expected: provision、idle_exit 等已有测试全过，行为无变化（纯重构）

- [ ] **Step 5: Commit**

Run: `git add crates/gitim-runtime/src/agent.rs crates/gitim-runtime/src/http.rs`
Run: `git commit -m "refactor(runtime): provision_human takes remote_url + auth parameters"`

---

### Task 4: `/git/init` 支持 github 分支

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:190-267`（`GitInitRequest` 结构 + `git_init` handler）

**职责：** 端到端打通 github 模式的 init 流程：接受 `remote_url + token` → pre-verify token → clone → provision human → 返回身份信息。同时写 config.json 持久化。

- [ ] **Step 1: 扩 GitInitRequest struct**

`http.rs:190` 的 `struct GitInitRequest` 改为：
- `provider: String`
- `remote_url: Option<String>`（github 模式必填，local 模式必空）
- `token: Option<String>`（同上）

**Acceptance：** 结构定义通过编译，对 local 模式的请求向后兼容（`remote_url/token` 缺失时反序列化为 None）

- [ ] **Step 2: 写 local 模式回归测试**

新文件 `crates/gitim-runtime/tests/git_init_local.rs`（若不存在）或扩展已有：
- 起 runtime、POST `/workspace` 设 path、POST `/git/init` with `{provider: "local"}` → 断言 ok，`$workspace/repo.git` 存在，`$workspace/.gitim-runtime/human/` 存在

**Acceptance：** 测试通过（说明重构没破坏 local 模式）

- [ ] **Step 3: 写 github 模式失败测试**

新文件 `crates/gitim-runtime/tests/github_init.rs`：
- `github_init_rejects_missing_token`：POST `{provider: "github", remote_url: "...", token: null}` → 400 + `error_code: "missing_token"`

**Acceptance：** 编译失败（`error_code` 字段未定义 in response）

- [ ] **Step 4: 改 git_init handler 分支**

`http.rs:195` 的 `git_init`：
- 旧 `if req.provider != "local" { error }` 改为 `match req.provider.as_str()`
- `"local"` 分支：保持现有行为 + 在 provision 成功后调 `WorkspaceConfig::write()` 写入 `{provider: Local, ..}`
- `"github"` 分支：
  - 校验 `remote_url` 和 `token` 非空 → 否则 `error_code: "missing_token"` / `"missing_remote_url"`
  - 调 `github::verify_token(token, "https://api.github.com")` → 失败映射 `InvalidToken` → `error_code: "invalid_token"`；`InsufficientScope` → `"insufficient_scope"`；`NetworkError` → `"network_error"`
  - 从返回的 `GithubIdentity` 得到 `handler = login.to_lowercase()`, `display_name = name.unwrap_or(login)`
  - 构造 token URL：`https://x-access-token:{token}@github.com/{owner}/{repo}.git`（从用户给的 `remote_url` 提取 owner/repo）
  - 调 `provision_human(workspace, token_url, "git", json!({"handler": handler, "display_name": display_name}))` —— 注意：daemon 的 onboard 对于"已给定 handler + display_name"的 auth 会走 git 变体，所以这里 `git_server` 传 `"git"`（daemon 的 `build_auth` 里已经有这个分支）
  - 写 `WorkspaceConfig` 到 config.json（provider: Github, remote_url: 用户原始 URL 不含 token, token）
  - 返回 `{ok: true, handler, display_name}`
- 其他 provider：返回 `error_code: "provider_not_supported"`

**Acceptance：** `github_init_rejects_missing_token` 测试通过

- [ ] **Step 5: 写 github 模式完整 happy path 测试**

在 `tests/github_init.rs` 追加 `github_init_full_flow`：
- mockito server stub `GET /user` 返回 `{"login": "alice", "name": "Alice"}`
- 在 tempdir 下 `git init --bare fake-github.git` 作为假 github remote
- 在 fake-github.git 里 seed 一个初始 commit（空 repo 不能 clone，daemon 无法 rebase 到 origin）
- 修改 runtime 代码让 `verify_token` 的 `api_base` 可以被**测试注入**（通过 env var `GITHUB_API_BASE` override，默认 `https://api.github.com`）
- POST `/git/init` with `{provider: "github", remote_url: "file:///...fake-github.git", token: "fake"}` + env `GITHUB_API_BASE=<mockito_url>`
- 断言：响应 ok, handler=alice；`.gitim-runtime/human/` 存在且是合法 git clone；`.gitim-runtime/config.json` 里 `git.provider == "github"`，token 字段存在但 redacted in logs（下个 task 做 log redact）

**注意：** 测试里构造 token URL 时用 `file://` 协议，跳过 `x-access-token:` 前缀（本地 git bare 不需要 auth）。这需要在代码里判断：`remote_url.starts_with("file://")` 时跳过 token 嵌入，直接用原 URL。这是**测试专用兼容**，生产 github URL 一定是 `https://`。

**Acceptance：** 完整流程测试通过，workspace 目录结构正确

- [ ] **Step 6: 写 github 错误路径测试**

追加：
- `github_init_fails_on_invalid_token`：mockito 返回 401 → `error_code: "invalid_token"`
- `github_init_fails_on_clone_error`：token ok 但 remote_url 不存在 → `error_code: "clone_failed"`
- `github_init_writes_no_config_on_failure`：clone 失败后 `.gitim-runtime/config.json` 不应该存在（事务性：要么完全成功，要么回滚）

**Acceptance：** 三个错误路径测试通过

- [ ] **Step 7: 清理失败状态**

在 git_init 的 github 分支，clone 失败 / provision 失败时，**删除已创建的 `.gitim-runtime/human/` 目录**（如果部分存在），不写 config.json。避免半成品状态卡住下次 init。

**Acceptance：** `github_init_writes_no_config_on_failure` 测试覆盖

- [ ] **Step 8: Commit**

Run: `git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/github_init.rs crates/gitim-runtime/tests/git_init_local.rs`
Run: `git commit -m "feat(runtime): git_init supports github provider with PAT"`

---

### Task 5: Agent provisioning in github mode + handler 冲突检查

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`add_agent` handler 附近，约 line 540-620）
- Modify: `crates/gitim-runtime/src/agent.rs`（`provision_agent` 若存在，或新建 helper）
- Create: `crates/gitim-runtime/tests/github_add_agent.rs`

**职责：** 让 `add_agent` 兼容 github 模式 —— 从 config.json 读 provider 决定 remote URL 怎么拼；在 provision 前读 human clone 的 `users/<handler>.meta.yaml` 检查 handler 冲突，避免 split-brain。

- [ ] **Step 1: 写 handler 冲突失败测试**

新文件 `tests/github_add_agent.rs`：
- `add_agent_rejects_existing_handler_in_github_mode`：手工在 tempdir 构造一个 github 模式的 workspace（config.json 含 provider: github + human clone 下有 `users/agent-a.meta.yaml`）→ POST `/agents` with `{handler: "agent-a", ...}` → 断言 400 + `error_code: "handler_conflict"`

**Acceptance：** 编译通过但测试失败（当前代码没做冲突检查）

- [ ] **Step 2: 实现 handler 冲突检查**

在 `http.rs` 的 `add_agent` handler 里，**在调 `provision_agent` 之前**：
- 读 `state.human_repo` 得到 human clone 路径
- 检查 `<human>/users/<handler>.meta.yaml` 是否存在
- 存在 → 返回 `{ok: false, error_code: "handler_conflict", error: "handler @{h} is already registered in this workspace"}`
- 这个检查**同时对 local 和 github 模式生效**（local 模式下也能防止同 workspace 添加同名 agent）

**Acceptance：** 上步测试通过

- [ ] **Step 3: 写 github 模式 add_agent 成功路径测试**

追加 `add_agent_github_mode_clones_with_token`：
- 构造 github 模式 workspace（用 file:// bare repo 伪装）
- POST `/agents` with `{handler: "agent-b", display_name: "Agent B", provider: "claude", ...}`
- 断言：
  - 响应 ok
  - `$workspace/agent-b/` 目录存在，是 git clone
  - `$workspace/agent-b/.git/config` 里 remote origin URL 包含 token（或 file:// 直连，取决于测试环境）
  - 新 agent 的 daemon onboard 成功（`users/agent-b.meta.yaml` 写回 remote）

**Acceptance：** 测试编译失败（`provision_agent` 不接 remote_url）

- [ ] **Step 4: 改 add_agent 的 remote URL 构造**

`http.rs:548` 的 `bare_repo = workspace.join("repo.git")` 改为：
- 读 `WorkspaceConfig::read(workspace)`
- `GitProvider::Local` → `remote_url = workspace.join("repo.git").to_string_lossy()`
- `GitProvider::Github` → `remote_url = build_token_url(config.remote_url, config.token)` —— 新 helper 在 `git_config.rs` 里
- 把 `remote_url` 传给 `provision_agent`

`provision_agent`（若签名已固定为 bare path）改签名为接受 `remote_url: &str`。

- [ ] **Step 5: 运行 github 成功路径测试**

Run: `cargo test -p gitim-runtime --test github_add_agent`
Expected: 2 个测试全过

- [ ] **Step 6: Local 模式回归**

Run: `cargo test -p gitim-runtime`
Expected: 既有 provision 测试全过（local 模式行为不变）

- [ ] **Step 7: Commit**

Run: `git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/agent.rs crates/gitim-runtime/src/git_config.rs crates/gitim-runtime/tests/github_add_agent.rs`
Run: `git commit -m "feat(runtime): add_agent supports github mode with shared token, rejects handler conflict"`

---

### Task 6: 日志 URL 脱敏审计 + 应用

**Files:**
- Modify: `crates/gitim-runtime/src/agent.rs`、`crates/gitim-runtime/src/http.rs`、`crates/gitim-daemon/src/onboard.rs`、`crates/gitim-sync/src/git.rs` 等所有 log URL 的位置

**职责：** 全仓库扫一遍 `tracing::info!` / `tracing::warn!` / `eprintln!` 等日志调用，凡是参数里带 URL（clone / fetch / push 的 URL）的，统一过 `redacted_url()` helper。daemon 和 sync 也要做（它们后续 push 会 log URL）。

- [ ] **Step 1: Grep 所有 URL log 点**

Run: `cargo build -p gitim-runtime && rg -n "clone|fetch|push.*url|remote_url" crates/gitim-runtime crates/gitim-sync crates/gitim-daemon --type rust | grep -i -E "info!|warn!|eprintln|println"`
目标：列出所有"打印 URL"的位置。记录到一个临时笔记。

- [ ] **Step 2: 写脱敏验收测试**

在 `crates/gitim-runtime/tests/github_init.rs` 追加：
- `github_init_logs_never_contain_token`：捕获 tracing 输出（用 `tracing_subscriber::fmt().with_writer` 重定向到 `Vec<u8>` buffer），跑一次 full flow，然后 grep 捕获的 log 确认**没有任何**原始 token 字符串

**Acceptance：** 测试很可能失败（至少有一个地方 log 完整 URL）

- [ ] **Step 3: 导出 redacted_url 为 pub**

在 `git_config.rs` 里 `redacted_url` 改成 `pub`，并从 runtime 和 daemon 都能引用（daemon 可以依赖 runtime 吗？不能 —— daemon 不依赖 runtime）。

**解法：** `redacted_url` 放到 `gitim-core` 或单独的 `gitim-util` 里。把它移到 `gitim-core` 的一个新模块 `url_redact`，runtime 和 daemon 都 depend on gitim-core（已有依赖）。

- [ ] **Step 4: 移动 redacted_url 到 gitim-core**

- 从 `crates/gitim-runtime/src/git_config.rs` 删除 `redacted_url`
- 新建 `crates/gitim-core/src/url_redact.rs`，导出 `pub fn redacted_url(url: &str) -> String`
- 在 `crates/gitim-core/src/lib.rs` 里 `pub mod url_redact;`
- runtime / daemon 都改用 `gitim_core::url_redact::redacted_url`

- [ ] **Step 5: 替换所有 URL log 调用**

Step 1 列出的每一个点：改为 `redacted_url(&url)` 传给 log。尤其注意：
- `agent.rs` 里 `git clone` 失败的 stderr 输出（可能含 URL）—— 虽然 stderr 不一定进 tracing，但要处理
- daemon `onboard.rs` 和 `sync/git.rs` 里所有 URL 相关 log

- [ ] **Step 6: 运行脱敏测试**

Run: `cargo test -p gitim-runtime --test github_init github_init_logs_never_contain_token`
Expected: 通过

- [ ] **Step 7: 全量测试**

Run: `cargo test`
Expected: 全过

- [ ] **Step 8: Commit**

Run: `git add -A`（多文件改动）
Run: `git commit -m "feat(core): move redacted_url to gitim-core and apply across runtime/daemon/sync logs"`

---

### Task 7: WebUI — 启用 GitHub 按钮 + 新表单

**Files:**
- Modify: `webui-v2/src/components/setup/git-provider-form.tsx`
- Create: `webui-v2/src/components/setup/github-setup-form.tsx`
- Modify: `webui-v2/src/components/setup/setup-gate.tsx`
- Modify: `webui-v2/src/hooks/use-connection-store.ts`

**职责：** WebUI 流程从"选 provider → local 就完了"扩为"选 provider → local 完或 github 再填表单"。github 表单：Remote URL + PAT + PAT deeplink + loading 文案轮播 + 错误分类显示。

- [ ] **Step 1: 改 git-provider-form.tsx**

`providers` 数组：
- `{ id: "local", label: "Git Local", enabled: true }` 不变
- `{ id: "github", label: "GitHub", enabled: true }` —— 从 false 改为 true，描述改 "Clone from an existing GitHub repository"
- `{ id: "gitlab", ... }` —— 删除（或标 Coming Soon 保留视觉位 —— **我选删除**，让 UI 干净）

改 `handleSelect`：
- `"local"` → 继续调 `/git/init {provider: "local"}` 然后 `setStatus("ready")`（不变）
- `"github"` → 只 `setStatus("github_setup")`，不发 API；等下一个表单提交时发

- [ ] **Step 2: 在 connection store 加状态**

`use-connection-store.ts`：
- `ConnectionStatus` union 加 `"github_setup"` 值，位于 `workspace_set` 和 `ready` 之间
- 不需要新增其他 state（表单自己管 input/loading）

- [ ] **Step 3: setup-gate.tsx 加路由**

`screens` 对象：
- 加 `github_setup: <GithubSetupForm />`
- import `GithubSetupForm` from `./github-setup-form`

- [ ] **Step 4: 写 GithubSetupForm 组件**

新文件 `github-setup-form.tsx`：
- 头部：GitIM 标题 + workspace path 小字
- 表单字段：
  - "Remote URL" input，placeholder `https://github.com/org/repo`，校验：必须以 `https://github.com/` 开头（前端只做格式提示，后端是真校验）
  - "Personal Access Token" input，`type="password"`，placeholder `ghp_...` 或 `github_pat_...`
- 按钮：
  - "Connect" submit 按钮
  - "Generate PAT on GitHub ↗" 链接按钮（`target="_blank"`），URL 形如 `https://github.com/settings/personal-access-tokens/new?name=GitIM%20runtime&description=...`
- Submit 行为：
  - setSubmitting(true)，启动文案轮播（见 Step 5）
  - `fetch(${baseUrl()}/git/init, {method: POST, body: {provider: "github", remote_url, token}})`
  - 响应 ok → `setStatus("ready")`
  - 响应错误：根据 `error_code` 展示不同文案（见 Step 6）
  - finally: setSubmitting(false)

- [ ] **Step 5: 实现 loading 文案轮播**

GithubSetupForm 内，当 `submitting === true` 时：
- `useState` + `useEffect`：每 1.5s 切一次文案，顺序 `["Verifying token…", "Cloning repo…", "Initializing workspace…"]`，到末尾停留在最后一个
- 按钮里渲染：`{submitting ? rotatingMessage : "Connect"}`

**Acceptance：** 功能性验收先放到 Step 7 E2E test

- [ ] **Step 6: 实现错误分类显示**

响应里的 `error_code` → 用户文案映射（hardcoded table）：
- `invalid_token` → "Token was rejected. Make sure the PAT is valid and not expired."
- `insufficient_scope` → "Token is missing required scopes. You need `repo` for classic PAT, or `Contents: R/W` + `Metadata: R` for Fine-grained PAT."
- `repo_not_found` / `repo_access_denied` → "Cannot access this repository. Check the URL and that your token has access."
- `network_error` → "Cannot reach GitHub. Check your internet connection."
- `clone_failed` → "Failed to clone the repository. See runtime logs for details."
- `handler_conflict`（不太可能发生在 init 阶段，但防御） / 其他 → 直接显示 `error` 字段

显示在 input 下方红色小字。

- [ ] **Step 7: 手测一遍**

启动 runtime + webui dev server，跑 github 模式完整流程：
- 正常 token + URL → 进 ready 页
- 错误 token → 显示 "Token was rejected"
- 错误 URL → 显示 "Cannot access this repository"

**注意：** 这一步需要真 GitHub（或 mock runtime）。可选：直接用 curl 对 runtime 发请求验证后端，前端 happy path 留到 Task 9 的 E2E。

- [ ] **Step 8: Commit**

Run: `git add webui-v2/src/components/setup/git-provider-form.tsx webui-v2/src/components/setup/github-setup-form.tsx webui-v2/src/components/setup/setup-gate.tsx webui-v2/src/hooks/use-connection-store.ts`
Run: `git commit -m "feat(webui): add github setup form with PAT input and error classification"`

---

### Task 8: 空 repo 边缘情况验证 + 修复（如需）

**Files:**
- 可能 Modify: `crates/gitim-daemon/src/onboard.rs::ensure_repo`
- 可能 Modify: `crates/gitim-sync/src/git.rs`
- Create: `crates/gitim-daemon/tests/empty_remote_onboard.rs`（或在现有 onboard 测试里加）

**职责：** 验证 daemon `ensure_repo` 能否处理"刚 clone 一个空 github repo → HEAD 不存在 / 默认分支未定"的情况。如不能，则修。

- [ ] **Step 1: 写"空 remote"集成测试**

新 test：
- 起 `git init --bare empty.git`（不 seed 任何 commit）
- `git clone empty.git human/` → 得到空 clone（git warn "cloned empty repository"）
- 在 human/ 里起 daemon，调 `handle_onboard` with git 模式
- 断言：ensure_repo 成功，`channels/general.*` 存在，第一次 push 推到 empty.git 的某个分支（main）

**Acceptance：** 大概率失败，因为：
- 空 clone 没有 HEAD → 很多 git 命令会报 `fatal: no HEAD`
- `state.git_storage.push()` 可能不知道推到哪个分支
- `state.git_storage.add_and_commit()` 在空 repo 里行为未知

- [ ] **Step 2: 定位失败点**

根据 Step 1 测试的实际错误，定位：
- GitStorage 初始化是否要求 HEAD 存在？
- add_and_commit 在空 clone 上是否正常？（应该能：git 允许 no-parent commit）
- push 时有没有 `--set-upstream origin main`？

记录失败栈，整理到 Task 8 的实现笔记。

- [ ] **Step 3: 修复 path 1 —— 默认分支**

若 GitStorage 没有明确设置默认分支：
- 在 clone 后 `git symbolic-ref HEAD refs/heads/main` 或 `git checkout -b main`
- 这让首次 commit 自动落到 main 分支
- 修改位置：`agent.rs::provision_human` 或 `sync/git.rs::init`

- [ ] **Step 4: 修复 path 2 —— 首次 push 带 upstream**

若 push 报 `no upstream configured`：
- GitStorage::push 在 has_remote 为 true 时，检查是否有 upstream，没有则用 `git push -u origin HEAD`
- 位置：`crates/gitim-sync/src/git.rs`

- [ ] **Step 5: 验证测试通过**

Run: `cargo test -p gitim-daemon empty_remote`
Expected: 通过

- [ ] **Step 6: 集成到 github_init 测试**

修改 Task 4 的 `github_init_full_flow` 测试：fake-github.git 从"seed 一个初始 commit"改成"完全空"，验证端到端也通。

- [ ] **Step 7: Commit**

Run: `git add -A`
Run: `git commit -m "fix(daemon,sync): handle empty remote clone during onboard"`

---

### Task 9: E2E test for github mode（Playwright）

**Files:**
- Create: `e2e/tests/github-onboard.spec.ts`
- Possibly Modify: `e2e/helpers/runtime-env.ts`（启动 runtime 的 helper）

**职责：** 用 Playwright 端到端测 webui → runtime → daemon → git 的整条链路。mock GitHub API（node stub），用 tempdir 下的 file:// bare repo 当 remote。

- [ ] **Step 1: 看已有 e2e 测试**

Read: `e2e/tests/startup.spec.ts`、`e2e/helpers/runtime-env.ts`（已存在）了解怎么起 runtime + webui。

- [ ] **Step 2: 写 github-onboard.spec.ts 骨架**

- 起一个 node 小 server 伪装 GitHub API `/user`
- 设置 runtime env `GITHUB_API_BASE=http://localhost:<stub_port>`
- Playwright 打开 webui，走 connect → set workspace → 点 GitHub → 填 URL + 伪 PAT → submit
- 断言：进 ready 页，聊天 UI 可见

- [ ] **Step 3: 跑 E2E**

Run: `npx playwright test e2e/tests/github-onboard.spec.ts`
Expected: 通过

- [ ] **Step 4: Commit**

Run: `git add e2e/tests/github-onboard.spec.ts`
Run: `git commit -m "test(e2e): add github mode onboard happy path"`

---

### Task 10: 文档更新

**Files:**
- Modify: `CLAUDE.md`（"Onboard 流程" 小节）
- Modify: `docs/runtime-architecture.md`（加 remote 模式章节）

**职责：** 让 CLAUDE.md 和架构文档反映 github 模式已落地。

- [ ] **Step 1: 更 CLAUDE.md Onboard 流程**

现有章节讲了 CLI 的 onboard。补 runtime 路径：
- Runtime `/git/init` 支持 local / github
- github 模式流程：PAT → verify → clone → config.json → provision_human
- config.json 的 git 字段 schema
- Handler 冲突检查（add_agent）

- [ ] **Step 2: 更 docs/runtime-architecture.md**

加新章节 "Git 模型 —— Remote 模式"：
- ASCII 图：local vs github 的盘面对比
- Token 存储（.gitim-runtime/config.json + 各 clone .git/config）
- 多机场景说明（agent 锚定机器，消息共享）
- 已知 tension（rate limit、token 过期、GitHub 审计归因丢失）

- [ ] **Step 3: Commit**

Run: `git add CLAUDE.md docs/runtime-architecture.md`
Run: `git commit -m "docs: document workspace github mode"`

---

## Open Questions / 留给实现阶段定

| 点 | 说明 |
|----|------|
| GitHub API base URL 注入机制 | Task 2/4 提到的 env var `GITHUB_API_BASE` 是测试专用。生产用硬编码常量还是 config 可覆盖？—— **倾向**：硬编码 `https://api.github.com`，测试用 `#[cfg(test)]` + 参数注入，不走 env var |
| Token URL 构造细节 | github URL 的 owner/repo 提取（从 `https://github.com/owner/repo` 或 `https://github.com/owner/repo.git` 两种形式都要支持），再拼成 `https://x-access-token:TOK@github.com/owner/repo.git` |
| PAT deeplink 的 scope query | GitHub 的 PAT 生成页支持预填 name / description 但 scope 要用户手选。deeplink 仅能预填部分参数，实际 scope 用户需在页面勾选 |
| 空 repo push 的具体修复方式 | Task 8 可能需要多步调整。实现时按实际错误栈走 |
| CORS / HTTPS 证书验证 | Runtime 直接跑本机，WebUI 也本机，不跨域。GitHub HTTPS 证书 reqwest 用 rustls 默认 bundle 的 CA，应该无问题。**这一项不需要特殊处理** |

---

## Testing Matrix

| 层 | 覆盖点 | 测试文件 |
|-----|-------|----------|
| Unit | GitConfig schema roundtrip、redact、perms | `tests/config_schema.rs` |
| Unit | GitHub API client verify_token 四类响应 | `src/github.rs` 内联 |
| Integration | /git/init local 模式回归 | `tests/git_init_local.rs` |
| Integration | /git/init github happy path + 错误 | `tests/github_init.rs` |
| Integration | add_agent handler 冲突 + github 成功 | `tests/github_add_agent.rs` |
| Integration | 空 remote onboard | `crates/gitim-daemon/tests/empty_remote_onboard.rs` |
| E2E | WebUI github 模式完整流程 | `e2e/tests/github-onboard.spec.ts` |

---

## Self-Review 结果

- ✅ Spec coverage：8 个 Q + 额外细节，每个都有对应 Task
- ✅ No code blocks in plan（follow feedback_plan_no_code）
- ✅ Exact file paths throughout
- ✅ Type / 结构名称在 tasks 间保持一致（GitConfig / GitProvider / GithubIdentity / GithubError / error_code 命名）
- ⚠️ Task 8（空 repo）是"先诊断后修复"型，步骤取决于实际失败栈 —— 保留为探索性 task，不是硬性 TDD
- ⚠️ Task 6（日志脱敏）需要跨 crate 改动（gitim-core 新增模块），提前识别了依赖方向
