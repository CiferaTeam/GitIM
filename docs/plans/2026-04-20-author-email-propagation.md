# Author Email Propagation — workspace → agent → commit

## 背景

接续 [2026-04-20-author-email-unify.md](2026-04-20-author-email-unify.md)。第一轮把 daemon 单 clone 的 author email pipeline 跑通了(me.json 有 `github_email` 就会用)。实盘验证后三个残留问题暴露:

1. **Agent me.json 没 email**:runtime 的 `provision_agent` 走 `"type": "git"` onboard agent,只传 handler + display_name。workspace owner 的 email 没有 propagation 通道
2. **me.json 被 re-onboard 抹掉**:daemon 的 `write_me_json` 整文件覆盖,已有 `github_email` 会被一并抹(用户重启时看到 onboarded_at 变但 email 字段丢失就是这个坑)
3. **Sync rebase commit 的 author name 是 lewis**:`sync_loop.rs:459` 调 `add_and_commit(...)` 不带 author,git 退本地 global config(`user.name=lewis`),结果 `framer-opus` 的 daemon 产出的 rebased commit author 写成 `lewis`

## 目标

- 所有 daemon(human + agents)的 author email 自动一致 workspace owner,不靠手动编辑 me.json
- me.json 被任何路径重写都不会抹掉已有 `github_email`
- sync rebase 的 commit author name 归属该 daemon 的 current_user(比如 framer-opus),不再退 git config

## 非目标

- 不重写历史 commit
- 不改 Path B 语义(rebase commit 依然 committer = 本地 git config,只改 author name/email)
- 不做 "per-agent 独立 GitHub 身份"(共用 workspace owner email)
- 不处理 Gitea / GitLab 的 email 自动拉取(这些渠道的 identity 源不同,单独处理)

## 设计

### #3 merge 语义 —— write_me_json

`crates/gitim-daemon/src/onboard.rs` 的 `write_me_json` 改成"读旧文件 → merge → 写":

- 读现有 me.json(存在时)
- 如果 caller 没传 `github_email` 且旧文件里有,保留旧值
- 等价地,未来要加新字段(例如 display_name_override)也走相同路径不丢字段

`write_guest_me_json` 不处理 email(guest 没身份),但也走 merge 以免抹掉手动写入的额外字段。

### 2b WorkspaceConfig.git.github_email —— workspace → agent propagation

**数据流**:

```
github mode /git/init
  → runtime 调 GitHub /user 拉 email (扩展现有 github.rs)
  → 写进 WorkspaceConfig.git.github_email (新字段)

provision_agent
  → 读 workspace 的 github_email
  → 放进 agent onboard auth payload: AuthData::Git { handler, display_name, github_email }
  → daemon identity::infer_identity(Git variant) 把它填入 InferredIdentity.email
  → daemon write_me_json 写进 agent .gitim/me.json
  → agent daemon 下次 spawn / 现在的 AppState.github_email 读它
```

**key 点**:

- `GitConfig.github_email: Option<String>`(`#[serde(default, skip_serializing_if = "Option::is_none")]`),向后兼容
- `github.rs` 扩展:新增 `fetch_user_email(token, api_base) -> Result<Option<String>, GithubError>`,复用 send_github_get
- `/git/init` github 成功路径:在 `verify_token` 之后 拉 email,写进 config(best-effort —— 拉取失败不阻断 init,commit 会 fallback `<handler>@gitim`)
- `AuthData::Git` 新字段 `github_email: Option<String>`(daemon 侧,`#[serde(default)]`)
- `identity.rs:infer_identity` 的 Git variant 把字段填入 `InferredIdentity.email`
- `provision_agent` 接参数 `workspace_github_email: Option<&str>`,`http.rs` 调用处读 `WorkspaceContext.git_config.github_email` 传入

### X 单 author rebase commit —— sync_loop

`start_sync_loop` 和 `run_sync_cycle` 新增参数 `author: Option<(String, String)>`,snapshot from daemon 的 `(current_user, email)`。传到 `sync_loop.rs:459` 那行:

- 有 author → `add_and_commit_as(paths, msg, Some((name, email)))`
- 没有(guest / None) → `add_and_commit(paths, msg)`,保持旧行为

**daemon 侧 spawn_sync_loop 改动**:

```
current_user_snapshot = state.current_user.read().await.clone()
email_snapshot = state.github_email.read().clone()
author = current_user_snapshot.map(|u| state.author_for(&u))
start_sync_loop(..., author, ...)
```

snapshot 的 staleness 风险:当前 sync_loop 在 `handle_onboard` 末尾启动,此时身份已就位,不会再变。Guest → auth 迁移场景(极少)需要重启 daemon 才 pick 新值 —— 可接受。

## 实施步骤

### 阶段 1:#3 write_me_json merge

**文件**:`crates/gitim-daemon/src/onboard.rs`

- `write_me_json` 先读现有文件为 `serde_json::Value`(失败退回空对象),把新字段覆盖进去,github_email caller 无值时保留旧值
- `write_guest_me_json` 同样先读,merge(guest 不涉 email 字段但保留未来 extensibility)
- 新测试:`write_me_json_preserves_existing_github_email`(git 模式 onboard 保留之前的 email)

### 阶段 2:2b daemon 侧

**文件**:`crates/gitim-daemon/src/identity.rs`

- `AuthData::Git` 加 `#[serde(default)] github_email: Option<String>`
- `infer_identity` Git variant 把字段填入 `InferredIdentity.email`
- 测试:`git_mode_with_github_email_propagates_it`

### 阶段 3:2b runtime 侧

**文件**:`crates/gitim-runtime/src/git_config.rs`、`github.rs`、`http.rs`、`agent.rs`

- `GitConfig` 加 `#[serde(default, skip_serializing_if = "Option::is_none")] github_email: Option<String>`
- `github.rs` 加 `fetch_user_email(token, api_base) -> Result<Option<String>, GithubError>`
- `http.rs` `/git/init` github mode 成功路径 调 `fetch_user_email` 写进 WorkspaceConfig.git.github_email(best-effort)
- `agent.rs::provision_agent` 签名加 `workspace_github_email: Option<&str>`,在 onboard auth json 里加 `github_email` 字段
- `http.rs` 所有 `provision_agent` 调用点从 `WorkspaceContext` 读 email 传入
- `http.rs` 的 workspace recover 路径:读 WorkspaceConfig 时把 github_email 灌入 WorkspaceContext

### 阶段 4:X sync_loop single-author rebase

**文件**:`crates/gitim-sync/src/sync_loop.rs`、`crates/gitim-daemon/src/state.rs`

- `start_sync_loop` 加参数 `author: Option<(String, String)>`
- `run_sync_cycle` 同步
- `sync_loop.rs:459` 那行改成根据 author 选择 `add_and_commit_as` 或 `add_and_commit`
- `state.rs::spawn_sync_loop` 启动时 snapshot `(current_user, github_email)` 并传入
- 测试:`conflict_rebase_commit_uses_daemon_author`(构造 rebase 场景,断言 commit author 是 current_user handler)

### 阶段 5:文档

**文件**:`CLAUDE.md`

- WorkspaceConfig schema 段落加 `git.github_email` 字段
- Onboard 流程段落补一句 github mode 自动写 workspace email + agent propagation

## 测试策略

- daemon 单元测试:merge 语义 + AuthData::Git 带 email 解析 + identity 填入 email
- daemon 集成:commit_test.rs 已覆盖 "有 email / 无 email" 两路
- runtime 单元/集成:`fetch_user_email` 对照 mockito 验证 200/401/空 email 三路;`/git/init` github 模式写 config 时含 github_email
- runtime e2e:provision_agent 收到 workspace email → agent me.json 含 github_email
- sync:新测试覆盖 rebase commit author 从 "退 git config" 变为 "handler+workspace email"

## Rollout

- 单次主干合并,无 feature flag
- 向后兼容:
  - 老 WorkspaceConfig 无 github_email 字段 → deserialize 默认 None → fallback 到 @gitim
  - 老 me.json 无 github_email → merge 不会强加 → fallback 到 @gitim
  - 老 AuthData::Git payload 无 github_email → `#[serde(default)]` 吞下,identity.email = None
- 对 flame4 workspace:合并后重装 binary,重启 runtime → 下一次 onboard 或重启会自动从 workspace config 或直接从 /user 填 email

## 回退

- 全 revert:me.json / WorkspaceConfig 里多余的 `github_email` 字段被旧代码忽略,无副作用
- 局部失败:WorkspaceConfig 没成功写 email → 行为等同上一版本(每个 agent 手写 me.json 的 email)
