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

身份由 CLI 在 `onboard` 阶段写入，Daemon 在启动时读取。

`onboard` 命令通过平台 API 推断当前用户 handler，写入 `.gitim/me.json`。Daemon 启动时从该文件读取身份，不再独立推断。

| Endpoint | 推断方式 | 环境要求 |
|----------|----------|----------|
| `github` | `gh api /user` → `.login` 小写化 | `gh` CLI 已认证 |
| `gitea` | Gitea API `/api/v1/user` → `.login` 小写化 | `GITEA_TOKEN` 环境变量 |

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

**身份刷新：** `gitim onboard --refresh` 重新推断并覆盖 `me.json`。

### 2.2 身份与 User 注册

`onboard` 流程在推断身份后，检查 `users/<handler>.meta.json` 是否存在：

- **存在** → 直接使用
- **不存在** → 调用 daemon 的 `register_user` API 创建文件，由 daemon 负责文件写入、commit 和 push

### 2.3 发消息时的身份使用

CLI 发消息时不再需要 `-a` 参数。daemon 自动使用 `.gitim/me.json` 中的 handler。`-a` 参数保留用于调试。

---

## 3. `gitim onboard` 命令

### 3.1 命令格式

```bash
gitim onboard <repo_name> [org]         # 默认 endpoint=github
gitim onboard <repo_name> [org] --endpoint gitea --url <gitea_url>
gitim onboard --refresh                 # 重新推断身份
```

| 参数 | 必填 | 说明 |
|------|------|------|
| `repo_name` | MUST | 仓库名称 |
| `org` | MAY | GitHub/Gitea 组织。省略时使用当前用户 |
| `--endpoint` | MAY | `github`（默认）或 `gitea` |
| `--url` | 当 endpoint=gitea 时 MUST | Gitea 服务地址 |
| `--refresh` | MAY | 重新推断身份 |

### 3.2 执行流程

```
gitim onboard <repo_name> [org]
│
├─ 1. 推断身份（GitHub/Gitea API）
├─ 2. 校验 Git 可用性
├─ 3. 尝试 clone
│     ├─ 成功 → 检查是否为合规 GitIM repo
│     │         ├─ 是 → 4a: 加载流程（写 me.json → 启动 daemon → register_user）
│     │         └─ 否 → 4b: 初始化流程（创建目录结构 → 写配置 → commit + push → 启动 daemon）
│     └─ 失败 → 4c: 创建流程（gh repo create / Gitea API → clone → 初始化）
```

### 3.3 废弃 `gitim init`

`gitim onboard` 是唯一入口命令。初始化逻辑作为内部函数保留。

---

## 4. Daemon 生命周期

### 4.1 启动

任何 CLI 命令自动触发。检查 PID → 进程不存在则 spawn → 轮询 socket 就绪（5 秒超时）。

### 4.2 运行

Daemon 一旦启动，**永不自动退出**。持续运行 sync loop 和 file watcher。一个 repo 对应一个独立进程。

### 4.3 停止

- `gitim stop` — 发送 `stop` API，daemon 优雅关闭
- 系统关机 / `kill <pid>` — SIGTERM handler 清理

### 4.4 异常恢复

| 状态 | 处理 |
|------|------|
| PID 存在，进程存活，socket 可连接 | 正常使用 |
| PID 存在，进程存活，socket 连接失败 | 等待重试（最多 5 秒） |
| PID 存在，进程不存在 | 清理 stale 文件，重新启动 |
| PID 不存在 | 启动新 daemon |
| Socket 存在但无 PID | 清理 stale socket，启动新 daemon |

所有恢复操作对用户静默。

---

## 5. config.yaml 扩展

```yaml
version: 1
endpoint: github              # "github" 或 "gitea"
endpoint_url: ""              # 仅 gitea 时必填
daemon:
  sync_interval: 30
  debug_http: false
  debug_port: 3000
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoint` | string | `"github"` | 平台类型 |
| `endpoint_url` | string | `""` | 平台 URL（gitea 时必填） |

向后兼容：省略 endpoint 字段时默认为 `github`。

---

## 6. 延后事项

- 手动配置身份覆盖（`gitim config set author`）
- 多 endpoint 支持（GitLab 等）
- 全局 daemon 管理（`gitim daemon list`）
- 全局 repo 注册表（`~/.gitim/repos.json`）
