# 用户模块

> GitIM 当前实现（用户与身份）

---

## 用户文件

```text
users/<handler>.meta.yaml
```

文件名使用小写 handler。`users/` 目录下至少存在一个身份文件。

### Handler 规则

| 属性 | 值 |
|------|------|
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 长度 | 1–39 个字符 |
| 模式 | `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |
| 保留值 | `system` — MUST NOT 注册 |

### Schema

```yaml
display_name: Code Reviewer
role: reviewer
introduction: 负责代码审查与质量把关
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `display_name` | string | MUST | 显示名称，1-64 字符 |
| `role` | string | MUST | 角色，自由填写，1-32 字符 |
| `introduction` | string | MUST | 自我介绍，1-500 字符 |

文件内容是 UTF-8 YAML。

---

## Onboarding 与身份推断

当前实现的 `gitim onboard` 分成两段：

1. **CLI 阶段**：校验参数、clone/创建仓库、创建本地 `.gitim/`、启动 daemon
2. **Daemon 阶段**：推断身份、写入 `.gitim/me.json`、确保 repo 结构、注册用户、启动 sync loop

### 支持的身份推断方式

| `git_server` | 输入 | 推断方式 |
|--------------|------|----------|
| `git` | `--handler` + `--display-name` | 直接使用用户提供值 |
| `github` | `--token` | 调 GitHub `/user` API，读取 `login` / `name` |
| `gitea` | `--token` + `--url` | 调 Gitea `/api/v1/user` API，读取 `login` / `full_name` |
| `gitlab` | `--token` + `--url` | 调 GitLab `/api/v4/user` API，读取 `username` / `name` |

### `.gitim/me.json` 格式

```json
{
  "handler": "alice",
  "git_server": "github",
  "display_name": "Alice",
  "inferred_at": "20260401T120000Z"
}
```

此文件保存在本地 `.gitim/` 目录，不提交到 Git 仓库。

### 用户注册

onboard 推断身份后，daemon 会检查 `users/<handler>.meta.yaml` 是否存在：

- **存在**：直接复用
- **不存在**：创建用户文件，默认 `role=member`、`introduction=GitIM user`

对首次注册的用户，daemon 还会自动加入 `general` 频道。

### 发消息时的身份使用

CLI 发消息时通常不需要传 `-a`。daemon 会默认使用 `.gitim/me.json` 中的当前身份；`-a` 仅用于调试覆盖。

---

## 设计决策

- **handler 作为稳定主键**：用户文件名、消息作者、DM 命名都直接复用同一个标识。
- **身份推断在 daemon**：避免 CLI 和 daemon 各自维护一套推断逻辑。
- **`me.json` 本地化**：同一仓库的不同 clone 可以使用不同身份。
- **用户注册由 daemon 统一落盘**：减少 CLI 直接操作 Git 和文件系统带来的竞态。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-core/src/types/handler.rs` | Handler 验证规则 |
| `crates/gitim-core/src/types/meta.rs` | `UserMeta` 类型 |
| `crates/gitim-core/src/validator/mod.rs` | `validate_user_meta()` |
| `crates/gitim-daemon/src/identity.rs` | 各平台身份推断 |
| `crates/gitim-daemon/src/onboard.rs` | onboard 编排与 `me.json` 写入 |
| `crates/gitim-daemon/src/handlers.rs` | `handle_register_user()` |
| `cli/src/commands/onboard.ts` | onboard 命令和仓库 clone/create |
