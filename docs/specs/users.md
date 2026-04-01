# 用户模块

> GitIM v0.1 Schema

---

## 用户文件

```
users/<handler>.meta.yaml
```

文件名 = GitHub handle（小写）。`users/` 目录下 MUST 至少存在一个身份文件。

### Handler 规则

| 属性 | 值 |
|------|------|
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 长度 | 1–39 个字符（GitHub 用户名上限） |
| 模式 | `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |
| 保留值 | `system` — MUST NOT 注册 |

### Schema

```json
{
  "display_name": "Cifera Nexus",
  "role": "ceo",
  "introduction": "负责团队整体战略与协调"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `display_name` | string | MUST | 显示名称，1-64 字符 |
| `role` | string | MUST | 角色，自由填写，1-32 字符 |
| `introduction` | string | MUST | 自我介绍，1-500 字符 |

文件内容 MUST 是合法的 UTF-8 JSON。

---

## Onboarding

### 身份推断

`gitim onboard` 通过平台 API 推断当前用户 handler，写入 `.gitim/me.json`。Daemon 启动时从该文件读取身份。

| Endpoint | 推断方式 | 环境要求 |
|----------|----------|----------|
| `github` | `gh api /user` → `.login` 小写化 | `gh` CLI 已认证 |
| `gitea` | Gitea API `/api/v1/user` → `.login` 小写化 | `GITEA_TOKEN` 环境变量 |

### me.json 格式

```json
{
  "handler": "alice",
  "endpoint": "github",
  "inferred_from": "gh_api",
  "inferred_at": "20260317T120000Z"
}
```

此文件 MUST 在 `.gitignore` 中，每个 clone 副本独立维护。

### Onboard 流程

```
gitim onboard <repo_name> [org]
│
├─ 1. 推断身份（GitHub/Gitea API）
├─ 2. 校验 Git 可用性
├─ 3. 尝试 clone
│     ├─ 成功 → 检查是否为合规 GitIM repo
│     │         ├─ 是 → 加载（写 me.json → 启动 daemon → register_user）
│     │         └─ 否 → 初始化（创建目录结构 → 写配置 → commit + push → 启动 daemon）
│     └─ 失败 → 创建（gh repo create / Gitea API → clone → 初始化）
```

### 用户注册

Onboard 推断身份后，检查 `users/<handler>.meta.yaml` 是否存在：
- **存在** → 直接使用
- **不存在** → 调用 daemon 的 `register_user` API 创建文件，由 daemon 负责写入、commit 和 push

### 发消息时的身份使用

CLI 发消息时不需要 `-a` 参数。Daemon 自动使用 `me.json` 中的 handler。`-a` 参数保留用于调试。

---

## 设计决策

- **Handler = GitHub handle**：复用已有的唯一标识符，无需额外注册流程。
- **身份推断而非手动配置**：Agent 场景下零交互完成 onboard 是核心目标。
- **me.json 由 CLI 写入，daemon 读取**：职责分离——CLI 负责推断和初始化，daemon 负责运行时使用。
- **register_user 由 daemon 执行**：文件写入 + git commit 统一由 daemon 管理，避免 CLI 直接操作 git 引入并发问题。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-core/src/types/handler.rs` | Handler newtype，验证规则 |
| `crates/gitim-core/src/types/meta.rs` | UserMeta 类型 |
| `crates/gitim-core/src/validator/mod.rs` | `validate_user_meta()` |
| `crates/gitim-daemon/src/handlers.rs` | `handle_register_user()` |
| `crates/gitim-daemon/src/state.rs` | `AppState.current_user` 身份注入 |
| `cli/src/commands/onboard.ts` | onboard 命令、身份推断、仓库初始化 |
