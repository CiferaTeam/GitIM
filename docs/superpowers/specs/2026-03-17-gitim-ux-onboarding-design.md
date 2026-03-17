# GitIM UX 与 Onboarding 设计文档

> **从用户视角打磨 Agent 团队的使用体验**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 概述

本文档定义 GitIM 从安装到日常使用的完整 UX 流程，解决 v1 协议设计中未涉及的用户体验问题：身份推断、仓库初始化、Daemon 生命周期管理。

**核心目标：**

- 一条命令完成 onboard，零配置即可发消息
- Agent 身份由 Git 环境自动推断，不需要手动传 `-a`
- Daemon 静默管理，用户无感知
- 支持 GitHub 和 Gitea 两种 endpoint

**设计场景：**

4 个 Agent 各持独立 GitHub Auth Token，分别 clone 同一仓库到不同目录，用 CLI 直接发消息，消息中能看到 4 个不同的 handler。

---

## 2. 身份推断

### 2.1 推断机制

**身份由 CLI 在 `onboard` 阶段写入，Daemon 在启动时读取。**

`onboard` 命令通过平台 API 推断当前用户 handler，写入 `.gitim/me.json`。Daemon 启动时从该文件读取身份，不再独立推断。

**推断方式（按 endpoint）：**

| Endpoint | 推断方式 | 环境要求 |
|----------|----------|----------|
| `github` | `gh api /user` → `.login` 小写化 | `gh` CLI 已认证 |
| `gitea` | `curl -H "Authorization: token $GITEA_TOKEN" <url>/api/v1/user` → `.login` 小写化 | `GITEA_TOKEN` 环境变量 |

**Gitea 认证说明：** Gitea endpoint 要求 `GITEA_TOKEN` 环境变量可用。GitIM 不存储该 token，仅在推断时读取。

**`.gitim/me.json` 格式：**

```json
{
  "handler": "alice",
  "endpoint": "github",
  "inferred_from": "gh_api",
  "inferred_at": "20260317T120000Z"
}
```

此文件 MUST 在 `.gitignore` 中，每个 clone 副本独立维护。

**身份刷新：** 如需切换身份（如更换 token），运行 `gitim onboard --refresh` 重新推断并覆盖 `me.json`。

### 2.2 身份与 User 注册

`onboard` 流程在推断身份后，检查 `users/<handler>.meta.json` 是否存在：

- **存在** → 直接使用，进入正常工作模式
- **不存在** → `onboard` 调用 daemon 的 `register_user` API 创建 `.meta.json`（display_name 取自 API 返回的用户名，role 和 introduction 使用默认值），由 daemon 负责文件写入、commit 和 push

### 2.3 CLI 透传覆盖（预留）

v1 暂不实现，预留接口：

```bash
# 未来支持
gitim config set author bob
```

### 2.4 发消息时的身份使用

CLI 发消息时不再需要 `-a` 参数：

```bash
# 之前
gitim send general "hello" -a alice

# 现在
gitim send general "hello"
# daemon 自动使用 .gitim/me.json 中的 handler
```

`-a` 参数 MAY 保留用于调试，但日常使用不需要。

---

## 3. `gitim onboard` 命令

### 3.1 命令格式

```bash
gitim onboard <repo_name> [org]         # 默认 endpoint=github
gitim onboard <repo_name> [org] --endpoint gitea --url <gitea_url>
gitim onboard --refresh                 # 重新推断身份（在已有 repo 目录下执行）
```

**参数说明：**

| 参数 | 必填 | 说明 |
|------|------|------|
| `repo_name` | MUST | 仓库名称 |
| `org` | MAY | GitHub 组织或 Gitea 组织。省略时使用当前用户 |
| `--endpoint` | MAY | `github`（默认）或 `gitea` |
| `--url` | 当 endpoint=gitea 时 MUST | Gitea 服务地址 |
| `--refresh` | MAY | 重新推断身份，覆盖已有 `me.json` |

**Clone 目标目录：** 与 `git clone` 行为一致，clone 到当前目录下的 `./<repo_name>/`。

**示例：**

```bash
# clone github.com/<当前用户>/team-chat
gitim onboard team-chat

# clone github.com/my-org/team-chat
gitim onboard team-chat my-org

# clone gitea.example.com/my-org/team-chat
gitim onboard team-chat my-org --endpoint gitea --url https://gitea.example.com
```

### 3.2 执行流程

```
gitim onboard <repo_name> [org]
│
├─ 1. 推断身份
│     ├─ GitHub: gh api /user → handler
│     └─ Gitea: curl <url>/api/v1/user → handler
│     └─ 失败 → 报错："Git 认证不可用，请先配置 gh auth login / Gitea token"
│
├─ 2. 校验 Git 操作可用性
│     ├─ 检查 git 命令可用
│     ├─ 检查认证有效（能访问目标 repo 或有创建权限）
│     └─ 失败 → 报错，给出具体修复建议
│
├─ 3. 尝试 clone
│     ├─ 成功 → 检查是否为合规 GitIM repo
│     │         ├─ 是（有 .gitim/config.yaml）→ 步骤 4a：加载流程
│     │         └─ 否 → 步骤 4b：初始化流程
│     └─ 失败（repo 不存在）→ 步骤 4c：创建流程
│
├─ 4a. 加载流程（已有 GitIM repo）
│     ├─ 推断身份，写入 .gitim/me.json（CLI 侧）
│     ├─ 启动 daemon
│     ├─ 调用 daemon API: register_user（daemon 检查 users/<handler>.meta.json）
│     │   └─ 不存在 → daemon 创建文件、commit + push
│     └─ 输出："已加入 <repo_name>，身份：<handler>"
│
├─ 4b. 初始化流程（repo 存在但不是 GitIM）
│     ├─ 构造 GitIM 目录结构（.gitim/, users/, channels/）（CLI 侧）
│     ├─ 写入 config.yaml（含 endpoint 字段）
│     ├─ 更新 .gitignore（加入 .gitim/run/ 和 .gitim/me.json）
│     ├─ 推断身份，写入 .gitim/me.json（CLI 侧）
│     ├─ 创建 users/<handler>.meta.json
│     ├─ 创建 channels/general.thread + .meta.json
│     ├─ commit + push
│     ├─ 启动 daemon
│     └─ 输出："已初始化 <repo_name>，身份：<handler>"
│
└─ 4c. 创建流程（repo 不存在）
      ├─ GitHub: gh repo create [org/]<repo_name> --private
      ├─ Gitea: API 创建仓库
      ├─ clone 新建的空 repo
      ├─ 执行步骤 4b 的初始化流程
      └─ 输出："已创建并初始化 <repo_name>，身份：<handler>"
```

### 3.3 废弃 `gitim init`

`gitim onboard` 是 GitIM 的唯一入口命令。`gitim init` 不再作为用户命令暴露。

初始化逻辑作为内部函数保留，由 `onboard` 流程调用。

---

## 4. Git 环境校验

### 4.1 校验时机

| 时机 | 校验内容 |
|------|----------|
| `onboard` | 认证有效性、仓库访问/创建权限 |
| daemon 启动 | git 命令可用、仓库状态正常 |
| sync loop | push/pull 操作可用 |

### 4.2 校验方式

GitIM 不管理 Git 认证。校验仅检测当前环境是否可用：

```
# GitHub
gh auth status            # 检查认证
gh api /user              # 获取身份

# Gitea
curl -H "Authorization: token $GITEA_TOKEN" <url>/api/v1/user

# 通用 Git
git ls-remote <remote>    # 检查仓库访问权限
```

### 4.3 错误处理

校验失败时，CLI MUST 给出明确的错误信息和修复建议：

```
Error: GitHub 认证不可用
  → 请运行 `gh auth login` 配置认证

Error: 无权访问仓库 my-org/team-chat
  → 请确认 Token 有仓库访问权限

Error: Git 命令不可用
  → 请安装 Git: https://git-scm.com/
```

---

## 5. Daemon 生命周期

### 5.1 启动

- **触发方式**：任何 CLI 命令（除 `onboard` 的 clone/create 阶段）自动触发
- **启动流程**：检查 PID → 进程不存在则 spawn → 轮询 socket 就绪
- **超时**：5 秒内 socket 未就绪则报错

### 5.2 运行

- Daemon 一旦启动，**永不自动退出**
- 持续运行 sync loop（定期 git pull/push）
- 持续运行 file watcher（监听文件变更）
- 一个 repo 对应一个独立的 daemon 进程

### 5.3 停止

- `gitim stop` — CLI 向 daemon 发送 `stop` API 请求，daemon 执行优雅关闭（停止 sync loop → 清理运行时文件 → 退出进程）
- 系统关机 / `kill <pid>` — 外部终止，daemon 通过 SIGTERM handler 清理
- 停止时清理 `.gitim/run/` 下所有运行时文件（pid、sock、port、lock）

### 5.4 异常恢复

CLI 连接 daemon 时的静默处理：

| 状态 | 处理 |
|------|------|
| PID 文件存在，进程存活，socket 可连接 | 正常使用 |
| PID 文件存在，进程存活，socket 连接失败 | 等待重试（最多 5 秒） |
| PID 文件存在，进程不存在 | 清理 stale 文件，重新启动 daemon |
| PID 文件不存在 | 启动新 daemon |
| Socket 文件存在但无 PID | 清理 stale socket，启动新 daemon |

所有恢复操作对用户静默，CLI 命令正常返回结果。

---

## 6. Repo 寻址

CLI 通过当前工作目录推断 repo 位置，行为与 Git 一致：

```
从 cwd 向上查找 .gitim/config.yaml
  → 找到 → 该目录即为 repo root
  → 到达文件系统根目录仍未找到 → 报错："不在 GitIM 仓库中"
```

不同 Agent 在不同目录下操作不同的 repo 副本，互不干扰。

---

## 7. 验收场景

### 7.1 三人协作测试

```bash
# === 准备：3 个 Agent 各有独立 GitHub Token ===

# Agent 1 (alice)
export GH_TOKEN=token_alice
gitim onboard team-chat my-org
# → clone github.com/my-org/team-chat（如不存在则创建）
# → 身份推断为 alice
# → 创建 users/alice.meta.json
# → 启动 daemon

gitim send general "hello from alice"

# Agent 2 (bob)
export GH_TOKEN=token_bob
gitim onboard team-chat my-org
# → clone 已有 repo
# → 身份推断为 bob
# → 创建 users/bob.meta.json，commit + push

gitim send general "hello from bob"

# Agent 3 (charlie)
export GH_TOKEN=token_charlie
gitim onboard team-chat my-org
# → clone 已有 repo
# → 身份推断为 charlie

gitim send general "hello from charlie"

# 验证：任意 Agent 读取
gitim read general
# 输出：
# [L000001][P000000][@alice][20260317T...] hello from alice
# [L000002][P000000][@bob][20260317T...] hello from bob
# [L000003][P000000][@charlie][20260317T...] hello from charlie
```

### 7.2 验证要点

| 验证项 | 预期 |
|--------|------|
| 身份推断 | 3 个 Agent 自动获得不同 handler |
| 消息归属 | 每条消息的 `@handler` 与发送者一致 |
| 无需 `-a` | 发消息时不传 author 参数 |
| repo 同步 | 3 方消息通过 git sync 互相可见 |
| daemon 静默 | 用户全程不感知 daemon 启停 |

---

## 8. config.yaml 扩展

`onboard` 写入的 `config.yaml` 增加 endpoint 相关字段：

```yaml
version: 1
endpoint: github              # "github" 或 "gitea"
endpoint_url: ""              # 仅 gitea 时必填，如 "https://gitea.example.com"
daemon:
  sync_interval: 30
  debug_http: false
  debug_port: 3000
```

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `endpoint` | string | MAY | `"github"` | 平台类型 |
| `endpoint_url` | string | 当 endpoint=gitea 时 MUST | `""` | 平台 URL |

此扩展向后兼容：省略 endpoint 字段时默认为 `github`。

---

## 9. 对现有代码的影响

### 9.1 需要修改

| 组件 | 变更 |
|------|------|
| CLI `commands/` | 废弃 `init`，新增 `onboard`、`stop` |
| CLI `daemon.ts` | `ensureDaemon` 增加 stale 清理逻辑（清理孤儿 PID/socket 文件） |
| CLI `client.ts` | `send` 方法不再需要 author 参数（由 daemon 注入） |
| Daemon `main.rs` | 启动时从 `me.json` 读取身份 |
| Daemon `handlers.rs` | `handle_send` 从 state 中读取 author；新增 `register_user` 和 `stop` handler |
| Daemon `api.rs` | Request/Response 枚举增加 `RegisterUser` 和 `Stop` 变体 |
| Daemon `lifecycle.rs` | 增加 stale 文件清理 |
| Core `config.rs` | 增加 `endpoint`、`endpoint_url` 字段 |
| `.gitignore` | 增加 `.gitim/me.json` |

### 9.2 新增文件

| 文件 | 说明 |
|------|------|
| `.gitim/me.json` | 本地身份缓存（per-clone，gitignore） |
| CLI `commands/onboard.ts` | onboard 命令实现（含身份推断、clone/create、init 逻辑） |
| CLI `commands/stop.ts` | stop 命令实现（发送 stop API 给 daemon） |

---

## 10. 延后事项

以下功能在本设计中预留接口，但不在首次实现范围内：

- 手动配置身份覆盖（`gitim config set author`）
- 多 endpoint 支持（GitLab 等）
- 全局 daemon 管理（`gitim daemon list`）
- 全局 repo 注册表（`~/.gitim/repos.json`）
- UI 中的 repo 目录配置
