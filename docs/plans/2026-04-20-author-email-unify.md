# Commit Author Email 统一 —— 让 GitIM 活动算入 GitHub 贡献图

## 背景

当前 daemon 在构造 commit author 时,**author email 被硬编码为 `<handler>@gitim` 假邮箱**(`gitim-sync/src/git.rs:71`)。结果:

- **用户本人发消息、Agent 发消息 → author email 是假的 → GitHub 贡献图完全不识别**
- 仅 sync_loop 冲突修复后的 rebased commit 和 init repo commit,因为走 `add_and_commit`(不传 author,回落到本地 git config),author email 意外地是用户真邮箱,反而**意外进了贡献图**

同时,Daemon onboard 时通过 GitHub `/user` API 已经拿到了 user 的 email,但**这个字段被丢弃,没有任何地方持久化**。

用户需求:让所有 GitIM 活动(自己发的消息 + agent 发的消息)都归属到自己的 GitHub 账户(flame4),在 GitHub profile 的贡献图上亮起来。审计归因通过 git author name(保持为 handler)保留。

## 目标

1. 所有 daemon 发起的 commit,author email **可配置**为用户的 GitHub verified email,commit 就能算进 GitHub 贡献图
2. Author name 继续使用 handler,`git log --author=<handler>` 能筛出每个 agent 的 commit,审计归因不退化
3. 没配置 email 时,fallback 到当前的 `<handler>@gitim`,不破坏现有 workspace

## 非目标

- **不重写历史**。旧的 64 个 agent commit 就让它保持假邮箱,不做 `git filter-repo`
- **不统一 Path B**(sync_loop 的 rebased commit / init commit)。这些继续走本地 git config。已经在算贡献图,没必要动
- **不处理 Gitea / GitLab / Guest / local 模式的自动 email 获取**。这些目前走 fallback,本次不改
- **不做 "Update email" UI**(v2 scope)。用户要改 email 手动编辑 `.gitim/me.json` + 重启 daemon

## 设计要点

### Email 存储位置:`.gitim/me.json`

新增 `github_email: Option<String>` 字段。理由:

- **Per-clone,不入 git**:email 是隐私,不应被 agent 间共享到 git 仓库。与现有 `project_provider_config_scope` 一致
- **onboard 时自动填**:GitHub mode daemon 已经在调 `/user`,顺手把 `email` 写进 me.json
- **Fallback 策略**:`github_email.unwrap_or(format!("{}@gitim", handler))`,老 workspace 无感

### 调用链:me.json → AppState → add_and_commit_as

1. Daemon 启动时读 me.json,把 `github_email` 放进 AppState
2. `add_and_commit_as` 签名变 `Option<&str>` → `Option<(name, email)>`
3. 所有 Path A 调用点(handlers.rs / card_handlers.rs / onboard.rs)计算 `email = app_state.github_email.clone().unwrap_or_else(|| format!("{}@gitim", handler))`

### Path B 不动

`sync_loop.rs:459`、`onboard.rs:281` 等调用 `add_and_commit`(no author)的地方保持原样。这些 commit 依赖本地 git config 作为 author,行为不变。

## 实施步骤

### 阶段 1:扩展 me.json schema

**文件**:`crates/gitim-daemon/src/main.rs`(读 me.json 处)、`crates/gitim-daemon/src/state.rs`(AppState)、`crates/gitim-daemon/src/onboard.rs`(写 me.json 处)

- 定义 `Me` struct 加 optional `github_email` 字段(找到 main.rs 现有解析 me.json 的地方)
- AppState 加 `github_email: Option<String>`,启动时从 me.json 填入
- onboard(github mode)在写 me.json 时把从 `/user` 获取的 email 带上。guest / local / gitea / gitlab mode 保持 `None`

### 阶段 2:改造 git.rs

**文件**:`crates/gitim-sync/src/git.rs`

- `add_and_commit_as` 签名从 `author: Option<&str>` 改为 `author: Option<(&str, &str)>`(name, email)
- 内部 `author_str` 构造改为 `format!("{} <{}>", name, email)`
- `add_and_commit` 逻辑不变(内部仍然调用 `add_and_commit_as(..., None)`)

### 阶段 3:更新所有 Path A 调用点

**文件**:`crates/gitim-daemon/src/handlers.rs`、`crates/gitim-daemon/src/card_handlers.rs`、`crates/gitim-daemon/src/onboard.rs`

共 12 处调用 `add_and_commit_as(..., Some(&author))` 或 `add_and_commit_as(..., Some(handler))` —— 统一改为传 `Some((handler, email))`,email 从 AppState 拿(或 fallback 到 `<handler>@gitim`)

考虑提一个 helper(例如 `AppState::commit_author_for(handler) -> (String, String)`),集中 email 解析逻辑,调用点不重复写 fallback。

### 阶段 4:测试调整

**文件**:`crates/gitim-sync/tests/*.rs`、`crates/gitim-daemon/tests/*.rs`、`crates/gitim-runtime/tests/*.rs`

- 测试里 `add_and_commit_as` 调用点跟签名调整对齐
- 新增 `git_ops_test.rs` 用例:验证 author email 随 signature 参数变化
- 新增 `onboard.rs` 测试:github mode 的 me.json 应包含 `github_email`
- 新增 daemon 集成测试:配置 `github_email` 的 workspace,commit 出来的 author email 与配置一致

### 阶段 5:文档更新

**文件**:`CLAUDE.md`、`docs/plans/2026-04-20-author-email-unify.md`

- CLAUDE.md 的 "Current Orientation" 加一条 learning 说明 author email 机制
- 补充 `.gitim/me.json` schema 的说明:增加 `github_email` 字段
- WorkspaceConfig 段落附注:`github_email` 与 workspace PAT 的关系(两者互相独立,email 仅用于 author 署名,不影响 push 认证)

## 测试策略

### 单元 / Sync 层

- `git_ops_test.rs`:验证传入 `(name, email)` → 生成 commit 的 author 两字段正确
- 现有测试(不传 author)确保回落到本地 git config,行为不变

### Daemon 层

- 新 workspace 的 me.json 配 `github_email = "test@example.com"`,发消息 → 检查 `git log --format='%ae'` 等于 `test@example.com`
- me.json **不**配 `github_email`,发消息 → author email 回落 `<handler>@gitim`,跟旧行为一致

### Onboard 层

- github mode:mock `/user` 返回带 email 的 response → 验证 me.json 里 `github_email` 字段被写入
- gitea / gitlab / local / guest mode:验证 me.json 里 `github_email` 为 null / 不存在

### 手动验证(production)

- `/Users/lewisliu/ateam/gitim-company/.gitim-runtime/human/.gitim/me.json` 手动加 `"github_email": "flame0743@gmail.com"`
- 重启 daemon,发一条测试消息
- 检查:
  - `git log -1 --format='%an <%ae> | %cn <%ce>'` 应显示 `<handler> <flame0743@gmail.com> | lewis <flame0743@gmail.com>`
  - Push 上去,~1 小时后 flame4 contribution graph 今天的格子 +1(`gh api graphql` 查 `contributionCalendar` 确认)

## Rollout

- 主干一次性合并,没有 feature flag。向后兼容(fallback 保留旧行为,旧 workspace 无感)
- 已有 workspace 想启用:手动在 me.json 加 `github_email`。或者走一次重新 onboard(github mode 会自动填)
- 用户 flame4 的 gitim-company workspace:PR 合并后按"手动验证"流程走一遍

## 回退方案

- 如果出现 commit 被拒 / author email 被 GitHub 视为非法:回退 AppState.github_email 传值,效果等同于 `None`,自动回落假邮箱
- 仓库层面,旧 commit 不受影响,只影响新 commit
- 极端情况:直接 revert 这批代码变更,me.json 里的 `github_email` 字段会被旧 daemon 忽略,无副作用

## 已知权衡

- 同 workspace 多 agent 共用一个 `github_email` → git log 里所有 handler 的 commit 的 email 列都一样。**审计靠 name 字段**,不是 email。这是设计选择,不是 bug
- `github_email` 填错了(非 flame4 的 verified email)→ commit 会归给另一个 GitHub 账户或无人。用户自负。我们可以在 me.json 写入前加格式校验(简单 regex),不做 GitHub 侧 verify 检查(那要另一份 token)
- 将来加"多 agent 独立 GitHub 账户"(每个 agent 不同 email)→ 需要 me.json 改 per-handler email map,或走不同方向。本次不做,但 schema 设计留出扩展可能(`github_email` 是 workspace-level 单值;后续如要 per-handler,可新增 `github_emails: Map<handler, email>`)
