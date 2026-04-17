# Workspace GitHub Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Runtime 的 `/git/init` 端点支持 GitHub provider，让 WebUI 用户能用 PAT 从 github remote 起一个 GitIM workspace，后续多 agent 共用 workspace 级 PAT。

**Architecture:** Remote 模式下跳过本地 bare repo，human/agent clone 直接挂 github 为 origin。Token 集中存 `$workspace/.gitim-runtime/config.json`（source of truth），派生到每个 clone 的 `.git/config` URL。身份由 **daemon** 从 GitHub `/user` API 自动推断（runtime 不做身份绕过）。sync_loop 在连续 auth 失败时进入熔断，避免烧 rate limit。

**Tech Stack:** Rust（runtime、reqwest、mockito），React / TypeScript（webui-v2）

---

## 设计决策速览（来自 grill 会话 + eng review）

| # | 决策 | 选项 |
|---|------|------|
| Q1 | Remote 模式**跳过** bare repo 创建，每个 clone 直连 github | 不保留本地镜像 |
| Q2 | 首发只支持 **GitHub**，架构留 provider 抽象 | GitLab/Gitea 未来增量 |
| Q3 | 只 **PAT 粘贴**，不做 `gh` 快捷复用、不做 OAuth、不做 SSH | Fine-grained PAT 单 repo scope |
| Q4 | 身份由 **daemon** 从 GitHub `/user` API 自动推断（而非 runtime 提前推断后伪装）| 保持 daemon 作为 identity source of truth |
| Q5 | Token 落盘**两处**：`.gitim-runtime/config.json`（中心 source）+ 各 clone `.git/config` URL（派生）| 更新入口唯一；launch/provision 时重新 propagate |
| Q6 | 只支持 **clone 已有 repo**，不做 "Create new repo" UI | 空 repo clone 靠 `ensure_repo` + 显式 `push -u` 撑起 |
| Q7 | 强制 **fresh clone**，不支持 "import existing local clone" | workspace 目录自包含 |
| Q8 | 所有 agent **共用 workspace PAT**；`add_agent` 检查 handler 冲突（先强制 fetch 再查）| GitHub audit 归因 PAT owner，webui 要给用户知情同意提示 |

**其他实现细节定调（已从 open questions 固化）：**

- **Init 反馈 UX**：单响应 + UI 文案轮播（"Verifying... / Checking repo access... / Cloning... / Onboarding..."）
- **错误分类**：返回 `error_code` 字段。枚举：`missing_token` / `missing_remote_url` / `invalid_token` / `insufficient_scope` / `token_lacks_repo_access` / `network_error` / `clone_failed` / `onboard_failed` / `handler_conflict` / `cloud_sync_path_rejected` / `provider_not_supported`
- **代码复用**：`redacted_url` helper 放 `gitim-sync`（不放 `gitim-core`，不破坏 core 职责纯净）。runtime / daemon 都依赖 sync，复用零成本
- **日志脱敏**：`redacted_url` helper 作为**前置依赖**（Task 2），所有新写的 log 点从一开始就用，避免中间 commit 的泄漏窗口
- **Token 过期 / revoke**：v1 给 sync_loop 加 **auth_failed 熔断**（连续 3 次 401/403 → 停 push/fetch，标记 workspace 状态），不打 UI 恢复 —— 但保证不烧 rate limit。runtime 暴露状态到 `/health`
- **Token URL 构造**：单独单元测试 + `#[ignore]` 标记的 `ls-remote dryrun` 测试验 URL 语法能被 git 解析
- **平台**：macOS / Linux v1 一等公民。Windows **明确 v1 out-of-scope**（chmod 0600 + xattr 等机制不兼容），WebUI 检测到 Windows 显示 "GitHub mode not yet supported on Windows"
- **云同步路径**：workspace 路径若在 iCloud Drive / Dropbox / Google Drive / Time Machine 目录下 → init 拒绝（error_code: `cloud_sync_path_rejected`），防止 token 跟着上云

---

## Non-goals (v1)

明确**不**做以下场景，避免暗坑：

| 不做 | 原因 | 用户 workaround |
|------|------|----------------|
| local 模式 workspace 切换到 github 模式 | 需要迁移 bare repo 内容到 github remote，涉及 force push，风险大 | rm -rf workspace，重新 onboard github 模式 |
| 换 remote URL（公司切账号） | 涉及更新 config.json + 所有 agent `.git/config`，边缘情况多 | rm -rf workspace，重建 |
| Token rotate UI | v1 没有 "Update token" 入口 | 手工改 config.json 后重启 runtime（Task 7 的 propagation 会扫一遍）|
| Windows 支持 | 权限模型、云同步检测都不适配 | Mac / Linux 为第一优先级 |
| Agent 独立 GitHub 身份 | 每个 agent 自带 PAT 会让配置复杂度翻倍；audit 需求可以用 commit author 覆盖 | Commit author 保留 agent handler，GitHub audit 看 author 字段 |
| OAuth Device Flow | 要为 GitHub/Gitea/GitLab 各注册 App，不值 | 手工生成 PAT |
| 删除 workspace 清理 `~/.gitim/runtime.json` 的引用 | 目录删了运行时发现回退到"无 workspace"态，下次 connect 重新设置即可 | 够用 |

---

## What already exists（避免重造）

| 已有能力 | 位置 | 用法 |
|---------|------|-----|
| Daemon 的 GitHub API 身份推断 | `crates/gitim-daemon/src/identity.rs::infer_identity` + `AuthData::GitHub { token }` 分支 | runtime 传 `{type:"github", token}` 给 daemon onboard，daemon 自己调 `/user` 推断 handler/name，不用 runtime 重复做 |
| `GitStorage` 的 push/fetch/rebase | `crates/gitim-sync/src/git.rs` | provision 后让 daemon 的 sync_loop 正常管。无需 runtime 直接动 git 状态 |
| `ensure_repo` 幂等初始化 | `crates/gitim-daemon/src/onboard.rs::ensure_repo` | 空 repo 场景下首次 push 会创建 `channels/general.*`。但需要修 `has_unpushed` 在无 upstream 时的行为（Task 11）|
| `register_user` + `auto_join_general` | 同上 | 新 handler 注册、自动加入 general，已经是幂等的 |
| `idle_exit` watchdog | `crates/gitim-runtime/src/http.rs::RuntimeState.last_activity` | token 过期场景下的熔断可以复用 last_activity 机制做状态标记 |

---

## 文件结构

**新增：**

- `crates/gitim-sync/src/url_redact.rs` — `redacted_url` helper（移到这里，非 core）
- `crates/gitim-runtime/src/github.rs` — GitHub API client（`verify_token` + `check_repo_access` + timeout + 边缘处理）
- `crates/gitim-runtime/src/git_config.rs` — `$workspace/.gitim-runtime/config.json` schema 读写 + 路径安全检测
- `crates/gitim-runtime/src/token_propagation.rs` — 启动 / provision / update 时把 config.json token 同步到所有 agent clone 的 `.git/config`
- `crates/gitim-runtime/tests/config_schema.rs` — schema + chmod + 云路径拒绝
- `crates/gitim-runtime/tests/github_api.rs` — verify_token 和 check_repo_access 的边缘响应
- `crates/gitim-runtime/tests/github_init.rs` — `/git/init` github 模式集成测试
- `crates/gitim-runtime/tests/github_add_agent.rs` — add_agent 冲突 + 正常路径
- `crates/gitim-runtime/tests/token_propagation.rs` — 重启 / update 传播
- `crates/gitim-runtime/tests/sync_auth_circuit.rs` — sync_loop 熔断
- `crates/gitim-sync/tests/url_redact.rs` — redact helper 单元测试
- `crates/gitim-sync/tests/empty_remote_push.rs` — 空 repo 首次 push 设 upstream
- `webui-v2/src/components/setup/github-setup-form.tsx` — Remote URL + PAT 表单
- `e2e/tests/github-onboard.spec.ts` — E2E Playwright
- `docs/plans/workspace-github-mode/` — 本 plan + 迭代笔记

**修改：**

- `crates/gitim-sync/src/lib.rs` — 暴露 `url_redact`
- `crates/gitim-runtime/src/http.rs` — `GitInitRequest` 扩字段、`git_init` 分支、`add_agent`、`workspace_status` 端点、Windows 拒绝
- `crates/gitim-runtime/src/agent.rs` — `provision_human` / `provision_agent` 接受 `remote_url + raw_auth`、pid kill 失败清理
- `crates/gitim-runtime/src/bin/runtime.rs` — recover 时调 token propagation
- `crates/gitim-runtime/Cargo.toml` — 确认 reqwest、mockito dev-dep、`#[cfg(unix)]` 相关 dep
- `crates/gitim-sync/src/sync_loop.rs` — auth 失败熔断
- `crates/gitim-sync/src/git.rs` — stderr redact、首次 push `-u origin HEAD`、auth error 分类
- `crates/gitim-daemon/src/onboard.rs` — `ensure_repo` 首次 push 显式设 upstream
- `webui-v2/src/components/setup/git-provider-form.tsx` — GitHub enabled，GitLab 移除
- `webui-v2/src/components/setup/setup-gate.tsx` — 加 `github_setup` 状态
- `webui-v2/src/hooks/use-connection-store.ts` — github setup state + workspace status 轮询
- `CLAUDE.md` — Onboard 流程小节补 github 路径
- `docs/runtime-architecture.md` — 加 remote 模式章节 + Non-goals

---

## Tasks（13 个，含依赖关系）

**依赖图：**

```
Task 1 (config) ─┐
                 ├─→ Task 3 (github API) ─→ Task 5 (/git/init) ─┐
Task 2 (redact)──┤                                                ├─→ Task 10 (webui) ─┐
                 ├─→ Task 4 (provision_human refactor) ───────────┤                    ├─→ Task 12 (E2E)
                 └─→ Task 6 (add_agent) ─→ Task 7 (propagation) ──┘                    │
                                                                                       │
Task 8 (sync 熔断)  [独立，可并行]                                                     │
Task 9 (log audit) [最后做 sweep，待 5/6/7 完成] ──────────────────────────────────────┤
Task 11 (empty repo) [独立，可并行]                                                    │
                                                                                       ↓
                                                                                 Task 13 (docs)
```

**并行 lane 建议**：
- Lane A: 1 → 3 → 5 → 10
- Lane B: 2 → 4（重构型，独立于 1）
- Lane B 续: 6 → 7
- Lane C: 8（独立）
- Lane D: 11（独立）
- 收尾: 9 → 12 → 13

---

### Task 1: Config schema + 平台安全检测

**Files:**
- Create: `crates/gitim-runtime/src/git_config.rs`
- Create: `crates/gitim-runtime/tests/config_schema.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`

**职责：** `.gitim-runtime/config.json` 扩 schema 支持 github 模式。chmod 0600。workspace 路径若位于云同步 / 备份目录 → 拒绝。macOS 下给 `.gitim-runtime/` 加 Time Machine exclusion xattr。Windows 不支持 github 模式（返回明确错误）。

- [ ] **Step 1: 写 schema 失败测试**

`tests/config_schema.rs` 写测试：
- `local_mode_roundtrip`：`WorkspaceConfig { provider: Local, .. }` 序列化反序列化一致
- `github_mode_roundtrip`：含 `provider: Github, remote_url, token` 的配置 roundtrip 一致
- `legacy_config_without_git_field_loads_as_local`：旧版 config（无 `git` 字段）反序列化得到 `provider: Local`

**Acceptance：** 编译失败（`GitConfig` / `GitProvider` 未定义）

- [ ] **Step 2: 实现 GitConfig schema**

`git_config.rs` 定义：枚举 `GitProvider { Local, Github }` 带 serde lowercase；结构 `GitConfig { provider, remote_url: Option, token: Option }`，`#[serde(default)]` 向后兼容；结构 `WorkspaceConfig { workspace, created_at, git: GitConfig }` 替代现 http.rs 里 ad-hoc json；`ConfigError` 用 thiserror 包装 io + serde。

**Acceptance：** 3 个 roundtrip 测试通过

- [ ] **Step 3: 写 chmod + 读写测试**

追加：
- `write_config_sets_0600_perms`（`#[cfg(unix)]`）：写到 tempdir → fs::metadata → 断言 `mode & 0o777 == 0o600`
- `read_config_from_nonexistent_returns_not_found`
- `read_config_from_fresh_workspace_returns_valid_struct`

**Acceptance：** 编译失败

- [ ] **Step 4: 实现 read/write helper**

`impl WorkspaceConfig`：
- `pub fn read(workspace: &Path) -> Result<Self, ConfigError>`
- `pub fn write(&self, workspace: &Path) -> Result<(), ConfigError>`：先写 tempfile → `fs::rename`（原子性）→ `#[cfg(unix)] set_permissions(0o600)`
- Windows 下 write 返回 `ConfigError::UnsupportedPlatform`（github 模式不支持）。local 模式用的 config 可以在 Windows 下写（只存 workspace + created_at），这个分支要允许

**Acceptance：** read/write 测试通过

- [ ] **Step 5: 写云同步路径拒绝测试**

追加：
- `reject_icloud_drive_path`：用 `~/Library/Mobile Documents/com~apple~CloudDocs/test-workspace` 调 `validate_workspace_path` → `Err(WorkspacePathError::CloudSyncDetected("iCloud Drive"))`
- `reject_dropbox_path`：`~/Dropbox/test` → `Err(.., "Dropbox")`
- `reject_time_machine_local_backups`：`/.MobileBackups/*` 或 `.Trashes` → reject
- `accept_normal_path`：`~/projects/test-workspace` → Ok

**Acceptance：** 编译失败

- [ ] **Step 6: 实现 validate_workspace_path**

`git_config.rs` 加 `pub fn validate_workspace_path(path: &Path) -> Result<(), WorkspacePathError>`：
- 读 `HOME` 环境变量
- 黑名单前缀：`$HOME/Library/Mobile Documents/`, `$HOME/Dropbox`, `$HOME/Google Drive`, `$HOME/OneDrive`
- macOS 额外：`/.MobileBackups`, `/.Trashes`
- 匹配任一 → `Err` 带上对应服务名
- 其他 → Ok

**Acceptance：** 拒绝测试通过，正常路径通过

- [ ] **Step 7: macOS xattr 测试**

追加 `#[cfg(target_os = "macos")]`：
- `exclude_from_time_machine_sets_xattr`：在 tempdir 创建 `.gitim-runtime/` → 调 `mark_excluded_from_backups(path)` → 用 `xattr::get(path, "com.apple.metadata:com_apple_backup_excludeItem")` 读 → 断言存在且值为 bplist 形式 `<true/>`

**Acceptance：** 编译失败（`mark_excluded_from_backups` 未定义）

- [ ] **Step 8: 实现 Time Machine exclusion**

`git_config.rs` 加 `#[cfg(target_os = "macos")] pub fn mark_excluded_from_backups(dir: &Path) -> io::Result<()>`：
- 调 `std::process::Command::new("xattr").args(["-w", "com.apple.metadata:com_apple_backup_excludeItem", "<?xml...<true/>", path])` 或用 `xattr` crate
- Linux / Windows：no-op（函数存在但空实现）

**Acceptance：** macOS 测试通过

- [ ] **Step 9: Commit**

Run: `git add crates/gitim-runtime/src/git_config.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/tests/config_schema.rs crates/gitim-runtime/Cargo.toml`
Run: `git commit -m "feat(runtime): workspace config schema + platform safety checks"`

---

### Task 2: `redacted_url` helper in `gitim-sync`

**Files:**
- Create: `crates/gitim-sync/src/url_redact.rs`
- Create: `crates/gitim-sync/tests/url_redact.rs`
- Modify: `crates/gitim-sync/src/lib.rs`

**职责：** URL 脱敏 helper，所有 log / error return 路径过一遍。前置依赖，避免后续 Task 留泄漏窗口。放在 `gitim-sync` 而非 `gitim-core`（core 是类型/解析，URL 处理不属于它）。

- [ ] **Step 1: 写失败测试（多种 URL 形式）**

`tests/url_redact.rs`：
- `redact_github_x_access_token`：`https://x-access-token:ghp_abc123@github.com/o/r.git` → `https://x-access-token:<REDACTED>@github.com/o/r.git`
- `redact_classic_username_password`：`https://user:pat@host/path` → `https://user:<REDACTED>@host/path`
- `redact_gitlab_oauth2`：`https://oauth2:token@gitlab.com/o/r.git` → `https://oauth2:<REDACTED>@gitlab.com/o/r.git`
- `redact_ssh_form_leaves_untouched`：`git@github.com:o/r.git` 不变
- `redact_no_credential_untouched`：`https://github.com/o/r.git` 不变
- `redact_multiline_text_handles_all_urls`：一段包含多个 URL 的日志文本 → 所有 credential 都被 redact
- `redact_preserves_sentinel_outside_credential`：`ghp_TESTSENTINEL` 作为字符串出现但不在 URL auth 段 → 不变

**Acceptance：** 编译失败（`redacted_url` 未定义）

- [ ] **Step 2: 实现 redacted_url**

`url_redact.rs` 用 regex：`(https?://)([^:/@\s]+):[^@\s]+@` → `$1$2:<REDACTED>@`。用 `regex::Regex::replace_all`。函数签名 `pub fn redacted_url(text: &str) -> String`（接任意文本，不仅 URL —— 支持 redact "stderr/log 文本中混入的 URL"）。

**Acceptance：** 7 个测试通过

- [ ] **Step 3: 在 lib.rs 暴露**

`crates/gitim-sync/src/lib.rs` 加 `pub mod url_redact;`

- [ ] **Step 4: Commit**

Run: `git add crates/gitim-sync/src/url_redact.rs crates/gitim-sync/src/lib.rs crates/gitim-sync/tests/url_redact.rs`
Run: `git commit -m "feat(sync): add redacted_url helper for token-in-url log scrubbing"`

---

### Task 3: GitHub API client（verify_token + check_repo_access + 边缘）

**Files:**
- Create: `crates/gitim-runtime/src/github.rs`
- Create: `crates/gitim-runtime/tests/github_api.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`
- Modify: `crates/gitim-runtime/Cargo.toml`

**职责：** 封装 runtime 对 GitHub API 的调用。`verify_token` 做 pre-flight（只为用户快速失败反馈，不做身份推断 —— 身份推断留给 daemon）；`check_repo_access` 在 clone 前精确区分 "token 无效 vs token 无此 repo 权限"，解决 github 对 private repo 返回 404 的诊断困难。超时 10s。覆盖边缘响应（非 JSON、字段缺失、429）。

- [ ] **Step 1: 确认 / 添加依赖**

检查 `Cargo.toml`：reqwest 现有，features `["json", "rustls-tls"]`；dev-dependencies 加 `mockito = "1"`。

- [ ] **Step 2: 写 verify_token 测试集**

`tests/github_api.rs`：
- `verify_token_401_returns_invalid_token`
- `verify_token_200_returns_ok_unit`（不返回 identity，仅 Ok(()) 表示 token 可用；身份推断不是 runtime 的事）
- `verify_token_403_returns_insufficient_scope`
- `verify_token_timeout_returns_network_error`（mockito 起 server 然后 sleep 11s 后回 → 10s 超时触发）
- `verify_token_non_json_200_returns_parse_error`（`"not a json response"` 返回）—— 但既然我们不 parse body，这个测试改为 `verify_token_200_succeeds_regardless_of_body`（不解析 body，只看 status code）
- `verify_token_429_returns_rate_limited`

**Acceptance：** 编译失败

- [ ] **Step 3: 实现 verify_token**

`github.rs`：
- `pub enum GithubError { InvalidToken, InsufficientScope, NetworkError(String), RateLimited, RepoNotFoundOrNoAccess, ParseError(String) }`
- `pub async fn verify_token(token: &str, api_base: &str) -> Result<(), GithubError>`：reqwest `GET {api_base}/user` + `Authorization: Bearer` + `User-Agent: gitim-runtime` + `.timeout(Duration::from_secs(10))`。只看 status：200 → Ok(())；401 → InvalidToken；403 → InsufficientScope；429 → RateLimited；其他 2xx/3xx → Ok(())；404 在这个端点不应出现；连接失败 → NetworkError

**Acceptance：** Step 2 测试通过

- [ ] **Step 4: 写 check_repo_access 测试集**

追加：
- `check_repo_access_200_returns_ok`
- `check_repo_access_404_returns_repo_not_found_or_no_access`（private repo 的未授权也返回 404，这是 GitHub 故意设计）
- `check_repo_access_403_returns_insufficient_scope`
- `check_repo_access_parses_owner_repo_from_https_url`：测 `parse_github_url("https://github.com/owner/repo")` → `("owner", "repo")`
- `check_repo_access_parses_owner_repo_from_dot_git_url`：`https://github.com/owner/repo.git` → `("owner", "repo")`
- `check_repo_access_parses_trailing_slash`：`https://github.com/owner/repo/` → `("owner", "repo")`
- `check_repo_access_rejects_non_github_host`：`https://gitlab.com/owner/repo` → `Err(ParseError)`

**Acceptance：** 编译失败

- [ ] **Step 5: 实现 check_repo_access + parse_github_url**

- `pub fn parse_github_url(url: &str) -> Result<(String, String), GithubError>`：regex 或 url crate 提取 owner/repo，验证 host 是 `github.com`
- `pub async fn check_repo_access(owner: &str, repo: &str, token: &str, api_base: &str) -> Result<(), GithubError>`：reqwest `GET {api_base}/repos/{owner}/{repo}` + Bearer。200 → Ok；404 → `RepoNotFoundOrNoAccess`；403 → `InsufficientScope`；其他按 verify_token 同样处理

**Acceptance：** Step 4 测试通过

- [ ] **Step 6: 暴露 + commit**

`lib.rs` 加 `pub mod github;`

Run: `git add crates/gitim-runtime/src/github.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/tests/github_api.rs crates/gitim-runtime/Cargo.toml`
Run: `git commit -m "feat(runtime): github API client with verify_token + check_repo_access + edge cases"`

---

### Task 4: Refactor `provision_human` — 纯参数化，不篡改 auth

**Files:**
- Modify: `crates/gitim-runtime/src/agent.rs::provision_human`
- Modify: `crates/gitim-runtime/src/http.rs`（调用点）

**职责：** `provision_human` 当前硬编码 `workspace/repo.git` 作 clone URL + 用 local git config 推断身份。改为接收 `remote_url: &str`、`git_server: &str`、`auth: serde_json::Value` —— **原样传给 daemon**，daemon 自己走 AuthData 的对应分支推断身份。local 模式由调用方传 `{type:"git", handler, display_name}`；github 模式传 `{type:"github", token}`。`provision_human` 不再区分身份推断的分支，**纯 transport**。

**关键：** 此处不做 eng review 初版 plan 里的 "runtime 推断后伪装成 git 骗 daemon"。daemon 保持作为身份推断的 source of truth。runtime 的 verify_token 只是 UX 的早失败 helper，不喂给 daemon。

- [ ] **Step 1: 跑现有 provision 测试建立基线**

Run: `cargo test -p gitim-runtime --test provision`
Expected: 全过（重构前 baseline）

- [ ] **Step 2: 改 provision_human 签名**

`agent.rs::provision_human` 签名改为 `pub async fn provision_human(workspace: &Path, remote_url: &str, git_server: &str, auth: serde_json::Value) -> Result<PathBuf, RuntimeError>`：
- `git clone remote_url` 替换硬编码 bare 路径
- 原先的 `detect_git_config(workspace)` 推断调用点**上移到 http.rs**，local 分支在 http.rs 里先算出 handler+display_name 再传进来
- `onboard` 调 `client.onboard(git_server, auth, admin=true, guest=false)`，不再内部构造 auth object

- [ ] **Step 3: 修 http.rs::git_init 的 local 分支**

local 分支：
- 保持创建 `repo.git` bare
- 调 `detect_git_config(workspace)` 得到 handler/display_name
- 调 `provision_human(workspace, bare_repo_path, "git", json!({"handler": handler, "display_name": display_name}))`
- 成功后调 `WorkspaceConfig::write({ provider: Local, ..})`

- [ ] **Step 4: 编译 + 跑 provision 和 idle_exit 测试**

Run: `cargo test -p gitim-runtime`
Expected: 既有 provision、idle_exit 测试全过（纯重构，行为不变）

- [ ] **Step 5: Commit**

Run: `git add crates/gitim-runtime/src/agent.rs crates/gitim-runtime/src/http.rs`
Run: `git commit -m "refactor(runtime): provision_human takes remote_url + raw auth (daemon owns identity inference)"`

---

### Task 5: `/git/init` 支持 github 分支

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`GitInitRequest` + `git_init` handler）
- Modify: `crates/gitim-runtime/src/agent.rs`（失败清理：kill daemon 进程）
- Create: `crates/gitim-runtime/tests/github_init.rs`

**职责：** 打通 github 模式端到端：verify_token → check_repo_access → clone（token URL） → provision_human（原样传 auth 给 daemon）→ 写 config.json。失败清理要 kill daemon 进程、rm 目录、不写 config。错误返回必须 redacted（过 `redacted_url`）。测试用 **trait injection** 注入 github API base（不用 env var、不用 `file://` hack）。

- [ ] **Step 1: 扩 GitInitRequest + 加 trait 注入**

`http.rs`：
- `struct GitInitRequest { provider: String, remote_url: Option<String>, token: Option<String> }`
- 定义 `trait GithubApiClient: Send + Sync { async fn verify_token(&self, token: &str) -> Result<(), GithubError>; async fn check_repo_access(&self, owner, repo, token) -> Result<(), GithubError>; }`
- 默认实现：`struct DefaultGithubApi { base_url: String }` 指向 `https://api.github.com`
- `RuntimeState` 加 `github_api: Arc<dyn GithubApiClient>` 字段，生产默认实例化 Default，测试可以注入 mock

**Acceptance：** 编译通过

- [ ] **Step 2: 写 local 模式回归测试**

`tests/git_init_local.rs`（若不存在则新建，或扩已有）：POST `/workspace` → POST `/git/init {provider: "local"}` → 断言 ok，`repo.git` + `.gitim-runtime/human/` + `config.json` 存在，config 里 `git.provider == "local"`。

**Acceptance：** 通过（确认重构没破坏 local）

- [ ] **Step 3: 写 github 失败测试**

`tests/github_init.rs`：
- `github_init_rejects_missing_token` → 400 `error_code: "missing_token"`
- `github_init_rejects_missing_remote_url` → 400 `error_code: "missing_remote_url"`
- `github_init_rejects_non_github_host` → 400 `error_code: "clone_failed"` 或 `provider_not_supported`（用 parse_github_url 的错误映射）
- `github_init_rejects_windows`（用 `#[cfg(target_os = "windows")]`）→ 400 `error_code: "provider_not_supported"`（Windows 下 config.write 返回 UnsupportedPlatform）
- `github_init_rejects_cloud_sync_workspace_path` → 400 `error_code: "cloud_sync_path_rejected"`（若 workspace 在 iCloud Drive 下，需 mock HOME）

**Acceptance：** 编译失败

- [ ] **Step 4: 改 git_init handler 的 github 分支**

`git_init`：
- `match req.provider.as_str()`
- `"local"` 分支：保持 Task 4 的行为（ensure bare + provision_human + write config）
- `"github"` 分支：
  1. 校验 `remote_url + token` 非空
  2. `validate_workspace_path(workspace)` —— 云同步路径拒绝
  3. `#[cfg(target_os = "windows")]` 直接返回 `provider_not_supported`
  4. `state.github_api.verify_token(token)` → 映射 GithubError → error_code
  5. `parse_github_url(remote_url)` → owner/repo
  6. `state.github_api.check_repo_access(owner, repo, token)` → 映射错误
  7. 构造 token URL：`https://x-access-token:{token}@github.com/{owner}/{repo}.git`
  8. 调 `provision_human(workspace, token_url, "github", json!({"type":"github", "token": token}))` —— daemon 走 GitHub auth 变体，自己调 /user 推断身份
  9. 从 onboard 响应拿 `handler`（daemon 返回的）
  10. macOS：`mark_excluded_from_backups(workspace.join(".gitim-runtime"))`
  11. 写 `WorkspaceConfig { provider: Github, remote_url: <原始 URL 不含 token>, token: Some(token) }` 到 `config.json`（原子 rename + chmod 0600）
  12. 返回 `{ok: true, handler, display_name}`
- 其他 provider：`provider_not_supported`
- **所有错误返回**：error 字段的任何 string 先过 `redacted_url`

**Acceptance：** Step 3 失败测试通过

- [ ] **Step 5: 写 github happy path 测试**

`github_init_full_flow_with_mock_api`：
- 创建 `DefaultGithubApi` 的 mock impl 返回 Ok（不用 mockito HTTP，直接 trait mock —— 因为 RuntimeState 注入）
- 本地 `git init --bare fake-remote.git`（空或 seed 一个 commit 都测）
- 注入 mock 的 RuntimeState
- POST `/git/init` with `{provider: "github", remote_url: "https://github.com/fake/fake", token: "ghp_TESTSENTINEL_abc"}` —— 但实际 clone 的 URL 要是本地 `file://...fake-remote.git`
- **问题**：git_init 代码固定用 `https://x-access-token:...@github.com` 拼 URL，怎么让它用 file:// ？
- **解法**：引入第二个 trait 或 fn injection：`CloneExecutor` trait，默认实现调 `git clone`，测试注入用 file:// 路径替换的版本。或更简单：`RuntimeState.clone_url_override: Option<String>` 测试时 set 为 `file://...`，生产永远为 None
- 断言：响应 ok，`.gitim-runtime/human/` 存在且是 git clone，`.gitim-runtime/config.json` 里 `provider == "github"`，token 字段与输入一致

**Acceptance：** 测试通过

- [ ] **Step 6: 写 token URL 拼接单元测试**

`tests/github_init.rs` 或 `src/http.rs` 内联：
- `build_token_url_standard_repo`：`("owner", "repo", "ghp_abc")` → `https://x-access-token:ghp_abc@github.com/owner/repo.git`
- `build_token_url_handles_dot_git_suffix_stripped`：repo 不应重复 `.git`
- `build_token_url_handles_hyphens_in_owner_repo`：`owner-org/my-repo` 正常
- 独立的 `build_token_url` 函数便于测试

**Acceptance：** 通过

- [ ] **Step 7: `#[ignore]` 标记的真实 URL dryrun 测试**

`github_init_token_url_syntax_is_git_parseable`（`#[ignore]`）：
- 构造 `build_token_url("fake", "fake", "invalid-token")` 得到完整 `x-access-token:...@github.com` URL
- `Command::new("git").args(["ls-remote", &url]).output()` —— 认证会失败但 **git 会先解析 URL 再尝试认证**
- stderr 断言：不包含 "fatal: invalid git URL" / "not a valid refspec"
- 接受 stderr 包含 "Authentication failed" 或 "Could not resolve host"（网络问题也 OK，只要不是语法错）

**Acceptance：** `cargo test -p gitim-runtime --test github_init -- --ignored` 通过

- [ ] **Step 8: 写错误路径 + 脱敏测试**

追加：
- `github_init_fails_on_invalid_token`：mock api verify_token 返回 InvalidToken → `error_code: "invalid_token"`
- `github_init_fails_on_token_lacks_repo_access`：verify 过但 check_repo_access 返回 RepoNotFoundOrNoAccess → `error_code: "token_lacks_repo_access"`
- `github_init_fails_on_network_timeout`：mock 超时 → `error_code: "network_error"`
- `github_init_fails_on_clone_error_cleans_up`：clone 失败后 `.gitim-runtime/human/` 不存在 + `.gitim-runtime/config.json` 不存在
- `github_init_response_body_never_contains_token`：用 sentinel token `ghp_TESTSENTINEL_xyz`，触发各种失败，grep 响应 JSON 不含 sentinel（即使是 git stderr 也要 redact）
- `github_init_logs_never_contain_token`：捕获 tracing 输出，不含 sentinel

**Acceptance：** 所有测试通过

- [ ] **Step 9: 实现失败清理（kill daemon + rm）**

`agent.rs::provision_human` 加一个 cleanup helper 或 `http.rs::git_init` 的 github 分支捕获 `Err(_)` 时：
- 若 `workspace/.gitim-runtime/human/.gitim/run/gitim.pid` 存在 → 读 PID → `signal::kill(pid, SIGTERM)`；等 500ms 再 SIGKILL
- `std::fs::remove_dir_all(workspace/.gitim-runtime/human)`（忽略 NotFound）
- 不写 config.json
- 测试 `github_init_fails_on_clone_error_cleans_up` 覆盖此

**Acceptance：** 清理测试通过

- [ ] **Step 10: Commit**

Run: `git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/agent.rs crates/gitim-runtime/tests/github_init.rs crates/gitim-runtime/tests/git_init_local.rs`
Run: `git commit -m "feat(runtime): git_init supports github with PAT verify + repo access check + failure cleanup"`

---

### Task 6: `add_agent` github 兼容 + 强制 fetch + handler 冲突检查

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`add_agent`）
- Modify: `crates/gitim-runtime/src/agent.rs`（`provision_agent`）
- Create: `crates/gitim-runtime/tests/github_add_agent.rs`

**职责：** github 模式下，add_agent 从 config.json 读 provider/token/remote_url 拼 agent 的 origin URL。conflict check 前**强制 fetch** human 的 clone，尽可能收敛并发竞态。冲突时清理半成品 agent 目录。

- [ ] **Step 1: 写 handler 冲突测试（含并发）**

`tests/github_add_agent.rs`：
- `add_agent_rejects_existing_handler_in_github_mode`：预置 workspace config + human clone 含 `users/agent-a.meta.yaml` → POST `/agents {handler: "agent-a"}` → 400 `error_code: "handler_conflict"`
- `add_agent_rejects_handler_existing_only_on_remote`：local `users/` 不含，但模拟"远端有"场景 —— 触发 fetch → 发现 → 拒绝
- `add_agent_github_mode_clones_with_token_url`：config.json 含 github provider → `/agents {handler: "agent-b"}` → 成功，`agent-b/.git/config` 的 origin URL 包含 token（或 file:// 在测试下）

**Acceptance：** 编译失败

- [ ] **Step 2: 实现 handler 冲突前置检查（含强制 fetch）**

`http.rs::add_agent`：
- 先读 `WorkspaceConfig::read(workspace)` 得 git config
- 若 github 模式：调 human daemon 的 sync API 做一次 `git fetch` + `rebase_onto_origin`（或直接 `Command::new("git").args(["fetch", "origin"]).current_dir(human_repo)` —— 更简单，避免跨 daemon 协作），等 fetch 完成
- 检查 `<human>/users/<handler>.meta.yaml` 是否存在 → 存在 → `{ok: false, error_code: "handler_conflict", error: "..."}`
- local 模式：跳过 fetch 直接检查（无远端）

- [ ] **Step 3: 改 add_agent 的 remote URL 构造**

`http.rs::add_agent`：
- 原 `bare_repo = workspace.join("repo.git")` 改为：match `config.git.provider`
  - `Local` → `remote_url = workspace.join("repo.git").to_string_lossy().to_string()`
  - `Github` → `remote_url = build_token_url(&config.git.remote_url.unwrap(), &config.git.token.unwrap())`
- 传给 `provision_agent`

`provision_agent` 签名改为 `pub async fn provision_agent(agents_dir, config, remote_url: &str) -> Result<AgentHandle, ...>`。

- [ ] **Step 4: 失败清理**

provision_agent 失败时：kill 新 agent 的 daemon pid + rm `<workspace>/<handler>/`。测试 `add_agent_failure_cleans_up`。

- [ ] **Step 5: Commit**

Run: `git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/agent.rs crates/gitim-runtime/tests/github_add_agent.rs`
Run: `git commit -m "feat(runtime): add_agent supports github + force fetch before handler conflict check"`

---

### Task 7: Token propagation（runtime 启动 + update）

**Files:**
- Create: `crates/gitim-runtime/src/token_propagation.rs`
- Create: `crates/gitim-runtime/tests/token_propagation.rs`
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`（recover 时调 propagation）
- Modify: `crates/gitim-runtime/src/http.rs`（暴露 `/workspace/update-token` 端点 v1 可选，但 helper 必须存在）

**职责：** `.gitim-runtime/config.json` 是 token 的 source of truth。runtime 启动恢复 workspace 时、新增 agent 后、未来 token update 时，都要把 config.json 的 token 同步写到所有 agent clone 的 `.git/config` URL。防止 config.json 和各 clone URL 漂移。

- [ ] **Step 1: 写 propagation 测试**

`tests/token_propagation.rs`：
- `propagate_token_updates_all_clones`：构造 workspace 含 human/ + agent-a/ + agent-b/，每个 `.git/config` 里 remote origin URL 都是旧 token → 调 `propagate_token(&workspace)` → 读 config.json 里新 token → 所有 clone 的 URL 都更新为新 token
- `propagate_token_skips_local_mode`：config.json 是 local 模式 → 什么都不做（无 token 要传播）
- `propagate_token_handles_missing_clone_directory`：config.json 指向 agent-c 但目录不存在 → 跳过不报错

**Acceptance：** 编译失败

- [ ] **Step 2: 实现 propagate_token**

`token_propagation.rs`：
- `pub fn propagate_token(workspace: &Path) -> Result<(), PropagationError>`：
  - 读 `WorkspaceConfig::read(workspace)`
  - 若 provider == Local，return Ok（local 模式无需传播）
  - 构造 expected URL `https://x-access-token:{token}@github.com/{owner}/{repo}.git`
  - 遍历 workspace 下所有目录（跳过 `.gitim-runtime/`、`.git/`、隐藏目录）
  - 对每个包含 `.git/config` 的目录：调 `Command::new("git").args(["-C", dir, "config", "remote.origin.url", &expected_url])`
  - 错误（目录不是 git repo、git 命令失败）：warn 但不中断
- `.gitim-runtime/human/` 也要更新
- 日志所有 URL 都过 `redacted_url`

**Acceptance：** 测试通过

- [ ] **Step 3: 集成到 runtime 启动**

`bin/runtime.rs::run_shell` 的 `recover_from_config` 之后，若 `RuntimeState.workspace.is_some()` → 调 `token_propagation::propagate_token(workspace)`。启动失败不阻断（warn 日志即可）。

- [ ] **Step 4: 集成到 add_agent 成功后**

`http.rs::add_agent` 在 `provision_agent` 成功后 → 调 `propagate_token(workspace)`（新 agent 的 .git/config 已经在 clone 时拿到了 token，这步是 defensive，保证一致）。

- [ ] **Step 5: Commit**

Run: `git add crates/gitim-runtime/src/token_propagation.rs crates/gitim-runtime/src/bin/runtime.rs crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/lib.rs crates/gitim-runtime/tests/token_propagation.rs`
Run: `git commit -m "feat(runtime): propagate workspace token to all clone .git/config on startup + provision"`

---

### Task 8: sync_loop auth 失败熔断

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`
- Modify: `crates/gitim-sync/src/git.rs`（auth error 分类）
- Create: `crates/gitim-sync/tests/sync_auth_circuit.rs`

**职责：** daemon 的 sync_loop 当 push/fetch 连续返回 auth 错误（401/403）时触发熔断：停止 push/fetch，暴露 "auth_failed" 状态给 daemon 的 status API。避免 PAT 过期后死循环烧 rate limit。

- [ ] **Step 1: 在 GitError 里分出 AuthFailed 变体**

`git.rs::GitError` 加 `AuthFailed(String)` 变体。`push`/`fetch` 的 stderr 解析：grep "fatal: Authentication failed" / "401" / "403" → `Err(GitError::AuthFailed(redacted_url(&stderr)))`。

- [ ] **Step 2: 写熔断测试**

`tests/sync_auth_circuit.rs`：
- `sync_loop_halts_after_3_consecutive_auth_failures`：mock GitStorage 让 push 始终返回 `AuthFailed` → 运行 sync_loop 若干轮 → 断言只跑了 3 次 push（熔断后停止），状态标为 `auth_failed`
- `sync_loop_resumes_on_successful_push_after_previous_failures`：前 2 次 push AuthFailed，第 3 次 Ok → 熔断未触发，计数重置

**Acceptance：** 编译失败（mock GitStorage 需要 trait 化）

- [ ] **Step 3: 抽 GitStorage 的 trait 接口**

若 `GitStorage` 是结构体（当前是），创建 `trait GitOps` 含 push / fetch / rebase_onto_origin 方法，GitStorage 实现它，sync_loop 依赖 `Box<dyn GitOps>` 或泛型参数。便于测试注入 mock。

**Replan 备注**：如果 GitStorage 已经大量代码且 trait 化成本太高 → 折中方案：sync_loop 里做"test seam"——内部函数 `fn should_skip_push(state) -> bool` 测试时可以替换。避免大重构。

- [ ] **Step 4: 实现熔断状态**

`sync_loop.rs` 加 `auth_failure_count: AtomicU32` 字段到 sync 状态。每次 push/fetch 返回 `GitError::AuthFailed` → 计数 +1；成功 → 重置 0。计数 >= 3 → 设置 `auth_failed: AtomicBool = true`，后续循环检查这个 flag → 直接 sleep 不做 git 操作。daemon API 暴露 `GET /status` 含 `auth_failed` 字段。

- [ ] **Step 5: 集成测试跑一轮**

Run: `cargo test -p gitim-sync --test sync_auth_circuit`
Expected: 通过

- [ ] **Step 6: Commit**

Run: `git add crates/gitim-sync/src/sync_loop.rs crates/gitim-sync/src/git.rs crates/gitim-sync/tests/sync_auth_circuit.rs`
Run: `git commit -m "feat(sync): circuit-break after 3 consecutive auth failures to prevent rate limit burn"`

---

### Task 9: 日志 + error return URL 脱敏审计

**Files:**
- Modify: 所有 `tracing::info!` / `warn!` / `eprintln!` / error return 有 URL 的位置，覆盖 `crates/gitim-runtime`、`crates/gitim-sync`、`crates/gitim-daemon`

**职责：** Task 2 已经把 `redacted_url` helper 做好，Task 5/6/7 也已经在新代码里用。本 Task 做**审计 sweep**：扫全仓库旧代码里所有打印 URL 的地方，过 redact；扫所有返回 error 到 webui 的路径（HTTP response body、daemon Response::error），stderr 经过时必 redact。

- [ ] **Step 1: Grep 所有 log 点 + 生成待改清单**

Run: `rg -n 'clone|fetch|push|remote_url|origin' crates/ --type rust | rg -E 'info!|warn!|error!|eprintln!|println!'`

人工 review 每个命中，判断是否打 URL。列清单到 `/tmp/url-log-audit.txt`。

- [ ] **Step 2: 替换所有 URL log 为 redacted**

对每个命中点，改为 `redacted_url(&url_or_stderr)` 再传给 log。

**关键点：**
- `agent.rs::provision_human` 的 `stderr = String::from_utf8_lossy(&output.stderr)` → `return Err(RuntimeError::GitCloneFailed(redacted_url(&stderr)))`
- `http.rs::git_init` 的所有 `format!("... {e}")` 里 `e` 可能含 stderr 的错误字符串 → 最终 response body 的 error 字段要过 `redacted_url`
- `sync/git.rs` 的 push/fetch stderr 返回 → 同上

- [ ] **Step 3: 写 sentinel 全局测试**

`tests/github_init.rs` 的 `github_init_response_body_never_contains_token` 已经覆盖一部分。追加：
- `provision_agent_error_never_leaks_token`：构造一个故意失败的 provision_agent（URL 不可达）→ 返回 error 不含 sentinel
- `sync_loop_error_logs_never_leak_token`：sync_loop 遇到 clone 失败 → 捕获 tracing → 不含 sentinel
- daemon 侧同样：`daemon_onboard_error_never_leaks_token`

**Acceptance：** 所有 sentinel 测试通过

- [ ] **Step 4: Commit**

Run: `git add -A`
Run: `git commit -m "chore: redact credentials in all log sites and error returns"`

---

### Task 10: WebUI — github 表单 + 知情同意 + platform check

**Files:**
- Modify: `webui-v2/src/components/setup/git-provider-form.tsx`
- Create: `webui-v2/src/components/setup/github-setup-form.tsx`
- Modify: `webui-v2/src/components/setup/setup-gate.tsx`
- Modify: `webui-v2/src/hooks/use-connection-store.ts`

**职责：** WebUI 两步走 —— 选 provider → 填 URL + PAT + 同意告知项。loading 文案轮播。错误分类友好提示。Windows / 云路径的前置检查（由后端返回 error_code 即可，前端渲染友好文案）。

- [ ] **Step 1: 改 git-provider-form.tsx**

- `providers` 数组：
  - `{id: "local", label: "Git Local", enabled: true}` 保留
  - `{id: "github", label: "GitHub", enabled: true}` —— 启用
  - 删除 `gitlab` 条目（不做 Coming Soon 假位）
- `handleSelect("local")` 保持：调 `/git/init` + `setStatus("ready")`
- `handleSelect("github")`：只 `setStatus("github_setup")`，不调 API

- [ ] **Step 2: connection store 加状态**

`use-connection-store.ts`：
- `ConnectionStatus` union 加 `"github_setup"` 位于 `workspace_set` 和 `ready` 之间
- `baseUrl`、`workspacePath` 保持不变

- [ ] **Step 3: setup-gate.tsx 路由**

`screens` 对象加 `github_setup: <GithubSetupForm />`，import。

- [ ] **Step 4: 写 GithubSetupForm**

新文件 `github-setup-form.tsx`。内容：
- 头：GitIM + workspace path
- 字段：
  - "Remote URL" input，placeholder `https://github.com/org/repo`，前端格式校验只做 "must start with https://github.com/"
  - "Personal Access Token" input `type="password"`，placeholder `ghp_... or github_pat_...`
- 按钮：
  - Primary "Connect"
  - Secondary "Generate PAT on GitHub ↗" —— `target="_blank"` 跳 `https://github.com/settings/personal-access-tokens/new?name=GitIM%20runtime`
- **告知项（复选框 + 文案）**：
  - ☐ "I understand all agents will commit as this PAT owner on GitHub (agent authorship preserved in commit author field but GitHub contribution graph attributes to PAT owner)"
  - 未勾选时 Connect 按钮 disabled
- Submit：
  - setSubmitting(true)，启动文案轮播
  - `fetch(baseUrl()/git/init, POST, { provider: "github", remote_url, token })`
  - ok → `setStatus("ready")`
  - error → 展示 error_code 对应文案（见 Step 5）
  - finally: setSubmitting(false)

- [ ] **Step 5: Loading 轮播 + 错误文案映射**

- 轮播：`useState + useEffect` 每 1.5s 切，顺序 `["Verifying token…", "Checking repo access…", "Cloning repo…", "Initializing workspace…"]`，停在最后一个
- error_code → 用户文案（hardcoded 映射表）：
  - `invalid_token` → "Token was rejected. Make sure the PAT is valid and not expired."
  - `insufficient_scope` → "Token is missing required scopes. Fine-grained PAT needs Contents R/W + Metadata R on this repo. Classic PAT needs \`repo\`."
  - `token_lacks_repo_access` → "Token is valid but has no access to this repository. Grant it access in PAT settings, or check the URL."
  - `network_error` → "Cannot reach GitHub. Check your internet connection."
  - `clone_failed` → "Failed to clone the repository. See runtime logs for details."
  - `cloud_sync_path_rejected` → "Workspace is inside a cloud-sync folder (iCloud/Dropbox). Move it elsewhere to keep your PAT local."
  - `provider_not_supported` + Windows → "GitHub mode is not yet supported on Windows. Use a Mac or Linux machine."
  - `handler_conflict` → 不该在 init 阶段出现，兜底："A conflict occurred. Please retry."
  - 其他 → 直接 `error` 字段

- [ ] **Step 6: 手测一遍**

启动 runtime（本地）+ webui dev server，手工验证：
- 正常 token + URL → ready
- 错 token → "Token was rejected"
- 正常 token 但 repo 没权限 → "Token is valid but has no access"

- [ ] **Step 7: Commit**

Run: `git add webui-v2/src/components/setup/git-provider-form.tsx webui-v2/src/components/setup/github-setup-form.tsx webui-v2/src/components/setup/setup-gate.tsx webui-v2/src/hooks/use-connection-store.ts`
Run: `git commit -m "feat(webui): github setup form with PAT input, loading rotation, error classification, and PAT-owner disclosure"`

---

### Task 11: 空 repo 首次 push 设 upstream

**Files:**
- Modify: `crates/gitim-daemon/src/onboard.rs::ensure_repo`
- Modify: `crates/gitim-sync/src/git.rs`（GitStorage::push 在无 upstream 时用 `-u origin HEAD`）
- Create: `crates/gitim-sync/tests/empty_remote_push.rs`

**职责：** Eng review 精确定位：空 github repo clone 后，`has_unpushed` 走 `git rev-list @{upstream}..HEAD` 但 upstream 不存在 → Err → sync_loop warn+跳过 → 永不 push。修复：`ensure_repo` 首次 commit 后调 `push -u origin HEAD` 显式设 upstream，后续 sync 就正常。

- [ ] **Step 1: 写"空 remote onboard + send"测试**

`tests/empty_remote_push.rs`：
- `git init --bare empty.git`（空，不 seed）
- `git clone empty.git bot-a/`（本地 clone，得到 unborn HEAD）
- 起 daemon + handle_onboard with git 模式
- 断言 ensure_repo 成功
- `send` 一条消息
- 另 clone `empty.git verify/` → 断言 `verify/channels/general.thread` 存在且含消息

**Acceptance：** 大概率失败于 send 后没 push 到 remote

- [ ] **Step 2: 修 GitStorage::push**

`sync/git.rs::push`：
- 先 `git rev-parse --symbolic-full-name '@{upstream}'` 检测 upstream
- 存在 → 正常 `git push`
- 不存在 → `git push -u origin HEAD` 设 upstream
- stderr 过 redacted_url

- [ ] **Step 3: 跑测试验证**

Run: `cargo test -p gitim-sync --test empty_remote_push`
Expected: 通过

- [ ] **Step 4: 更新 Task 5 的 full_flow 测试**

`github_init_full_flow_with_mock_api` 测试的 fake-remote.git 从"seed 一个初始 commit"改为"完全空" → 验证端到端也能工作（send 一条消息后 remote 能看到）。

- [ ] **Step 5: Commit**

Run: `git add -A`
Run: `git commit -m "fix(sync,daemon): set upstream on first push to handle empty remote clone"`

---

### Task 12: E2E Playwright（github 模式完整流程）

**Files:**
- Create: `e2e/tests/github-onboard.spec.ts`
- 参考: `e2e/helpers/runtime-env.ts`

**职责：** 端到端验证 webui → runtime → daemon → git 整条链路。用 node 小 server stub GitHub API（`/user` + `/repos/owner/repo`），用 tempdir 下的 file:// bare repo 作为假 remote。

- [ ] **Step 1: 看已有 e2e 模式**

Read: `e2e/tests/startup.spec.ts`、`e2e/helpers/runtime-env.ts`。了解 runtime + webui 启动方式和 cleanup。

- [ ] **Step 2: 写 github-onboard.spec.ts**

测试内容：
- beforeEach：起 node 小 server（Express）监听 `/user` 返回 `{login:"testuser", name:"Test User"}`，`/repos/org/repo` 返回 200；起 bare repo 在 tempdir
- 设 runtime 环境：通过 `RuntimeState.clone_url_override` 或 trait injection（Task 5 的 seam）让 clone URL 指向 file:// bare repo，api_base 指向 node stub
- Playwright：打开 webui → connect → set workspace → click GitHub → fill URL `https://github.com/org/repo` + PAT "fake-token" + 勾告知复选框 → submit
- 断言：transition 到 ready 页，主 chat UI 可见，"general" channel 出现
- 追加：发一条消息 → 查看消息在列表里

- [ ] **Step 3: 跑 E2E**

Run: `npx playwright test e2e/tests/github-onboard.spec.ts`
Expected: 通过

- [ ] **Step 4: Commit**

Run: `git add e2e/tests/github-onboard.spec.ts`
Run: `git commit -m "test(e2e): github mode onboard happy path"`

---

### Task 13: 文档（CLAUDE.md + runtime-architecture.md + Non-goals）

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/runtime-architecture.md`

**职责：** 文档反映 github 模式已落地，明确 Non-goals，记录 platform support。

- [ ] **Step 1: CLAUDE.md 更新 Onboard 流程章节**

补 runtime 路径：
- Runtime `/git/init` 两模式：local、github
- github 流程：verify_token → check_repo_access → clone (token URL) → provision_human (daemon 自推断) → write config
- `WorkspaceConfig` 在 `.gitim-runtime/config.json` 的新 schema
- Handler 冲突检查 + 强制 fetch（`add_agent`）
- sync_loop auth 熔断机制
- Token propagation（runtime 启动时）

- [ ] **Step 2: runtime-architecture.md 加"Git 模型 — Remote 模式"章节**

内容：
- ASCII 图：local vs github 盘面对比（保留现有 local 图 + 加 github 图 + 说明差异）
- Token 存储模型（config.json = source of truth，`.git/config` URL = 派生）
- Token propagation 触发点清单
- 多机场景：A 机器建 agent → B 机器只是人类节点，不自动拿到 agent；B 建同名 agent 被拒（handler_conflict）
- 已知 tension：
  - GitHub audit 归因 PAT owner（commit author 还是 agent handler）
  - sync_loop auth 熔断是 v1 的熔断，v2 加 UI 恢复
  - Rate limit：3-5 agent workspace 下正常运作，20+ agent 会超 5000 req/h
- Non-goals 章节复用 plan 里的表格

- [ ] **Step 3: Commit**

Run: `git add CLAUDE.md docs/runtime-architecture.md`
Run: `git commit -m "docs: workspace github mode + non-goals + platform support"`

---

## Failure Modes（每个新 codepath 至少一个）

| 路径 | 典型失败 | 测试 | Error handling | 用户看到 |
|------|----------|------|----------------|---------|
| `verify_token` | 网络超时 | `verify_token_timeout_returns_network_error` | 10s timeout + 返回 `NetworkError` | "Cannot reach GitHub" |
| `check_repo_access` | 404（private 无权限）| `check_repo_access_404_returns_repo_not_found_or_no_access` | 映射 `token_lacks_repo_access` | "Token is valid but has no access to this repository" |
| `git_init` clone 失败 | 网络断 / token URL 拼错 | `github_init_fails_on_clone_error_cleans_up` | kill daemon pid + rm dir + 不写 config | "Failed to clone the repository" |
| `add_agent` handler 冲突（远端）| sync_loop 没拉到最新 users/ | `add_agent_rejects_handler_existing_only_on_remote` | 前置强制 fetch | "Handler @X is already registered in this workspace" |
| `sync_loop` push 401 | PAT 被 revoke | `sync_loop_halts_after_3_consecutive_auth_failures` | 3 次后熔断，停 push/fetch | daemon status API 返回 `auth_failed: true`（v1 无 UI 提示，但 runtime 日志清晰）|
| `ensure_repo` 空 repo | upstream 不存在 | `empty_remote_push_sends_messages` | 首次 push 用 `-u origin HEAD` | 消息正常 push |
| `propagate_token` agent clone 不存在 | 目录被删 | `propagate_token_handles_missing_clone_directory` | warn 但不中断 | 不影响其他 clone |
| Runtime 错误 response | stderr 含 token URL | `github_init_response_body_never_contains_token` | 所有 error 字段过 `redacted_url` | 浏览器开发者工具看到的是 `<REDACTED>` |

**Critical gap check：**
- ✅ 所有失败有测试覆盖
- ✅ 所有错误都有明确用户文案
- ⚠️ Windows 的 "provider_not_supported" 错误文案是否足够清晰？Task 10 Step 5 映射里写了 "GitHub mode is not yet supported on Windows" —— OK

---

## Testing Matrix

| 层 | 覆盖点 | 文件 |
|----|-------|------|
| Unit | GitConfig schema roundtrip、chmod 0600、云路径拒绝、macOS xattr | `tests/config_schema.rs` |
| Unit | `redacted_url` 对多种 URL 形式 | `crates/gitim-sync/tests/url_redact.rs` |
| Unit | `verify_token` 四类状态码 + 超时 | `tests/github_api.rs` |
| Unit | `check_repo_access` 状态码 + URL parse | `tests/github_api.rs` |
| Unit | `build_token_url` 各种 URL 形式 | `tests/github_init.rs` 或 inline |
| Unit | `propagate_token` 各种 workspace 状态 | `tests/token_propagation.rs` |
| Integration | `/git/init` local 回归 | `tests/git_init_local.rs` |
| Integration | `/git/init` github happy path + 错误 + 清理 | `tests/github_init.rs` |
| Integration | `/agents` github 模式 + handler 冲突 + 强制 fetch | `tests/github_add_agent.rs` |
| Integration | sync_loop 熔断 | `tests/sync_auth_circuit.rs` |
| Integration | 空 remote onboard + 消息 push | `crates/gitim-sync/tests/empty_remote_push.rs` |
| Integration (gated) | token URL 真实 git 能解析 | `tests/github_init.rs::github_init_token_url_syntax_is_git_parseable` (#[ignore]) |
| Sentinel | token 不进 response body / log / error | 分散各集成测试 + daemon 侧 |
| E2E | WebUI github onboard 完整流程 | `e2e/tests/github-onboard.spec.ts` |

---

## Open Questions / 留给实现阶段

| 点 | 说明 |
|----|------|
| `xattr` crate 还是 Command 调用 | macOS Time Machine exclusion 用 `xattr` crate（更稳）或 subprocess `xattr -w`（无 dep）—— 实现时看 Cargo.toml 当前依赖倾向 |
| GitStorage trait 化还是 test seam | Task 8 Step 3：trait 化是正道但成本高。如果现有 GitStorage 方法 > 20 个，考虑 test seam 折中。实现时当场评估 |
| `RuntimeState.clone_url_override` 测试注入机制 | Task 5 Step 5 提到的 clone URL override。生产永远 None，测试注入 file:// 。可以用 atomic field 或干脆只测 `build_token_url` 纯函数 + 手工验证 E2E |
| `handle_onboard` 对 daemon 的 `AuthData::GitHub` 是否还有 bug | daemon 代码已有 `infer_identity` 的 GitHub 分支（`identity.rs`），但实际被测试覆盖了多少要跑一遍 `cargo test -p gitim-daemon identity` 确认 |
| `gitim send` 在 auth_failed 状态下的 UX | Task 8 熔断后 send 能写本地但不能 push。daemon 返回"本地写入成功，远端同步暂停"足够？还是报错？—— 实现时看 daemon 现有 api 的约定 |

---

## Self-Review 结果

- ✅ Spec coverage：8 个设计 Q + 17 个 eng review findings + 5 条实现细节，每条都有 Task 映射
- ✅ No code blocks in plan（follow feedback_plan_no_code memory）—— 仅引用函数名、结构名、文件路径
- ✅ Exact file paths throughout
- ✅ Types / 结构名称跨 task 一致性：`GitProvider` / `GitConfig` / `WorkspaceConfig` / `GithubError` / `RuntimeError` / error_code 枚举
- ✅ 依赖图清晰，并行 lane 标注
- ✅ Non-goals 明确列出，不留暗坑
- ✅ Failure modes 表覆盖所有新 codepath
- ⚠️ Task 5 Step 5 的 clone URL 注入机制未完全定稿 —— 留给实现阶段
- ⚠️ Task 8 Step 3 的 GitStorage 重构可能成本高 —— 允许实现阶段折中

---

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| Eng Review | 手工 | Architecture & tests（外部 reviewer + 本人合审）| 1 | CLEAR | 17 issues 全部整合入 plan |
| CEO Review | — | — | 0 | — | — |
| Codex Review | — | — | 0 | — | — |
| Design Review | — | — | 0 | — | — |

**VERDICT:** ENG CLEARED — ready to implement（按 Lane A/B/C/D 并行或顺序跑 13 tasks）
