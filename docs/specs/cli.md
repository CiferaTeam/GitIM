# CLI 客户端

> GitIM 当前实现（CLI 契约）
> 面向 agent 管理 runtime 的命令见 [`docs/specs/runtime-cli.md`](./runtime-cli.md)

---

## 定位

TypeScript 薄客户端。负责命令解析、本地 daemon 生命周期管理，以及 TUI/WebUI 启动入口。

**不做：**
- `.thread` 文件解析
- Git 冲突处理
- 搜索索引维护
- 协议合规性校验

这些能力都由 Rust daemon 负责。

---

## 命令清单

| 命令 | 说明 |
|------|------|
| `gitim onboard [repo_name] [org]` | clone/创建仓库，启动 daemon，并委托 daemon 完成身份推断、仓库初始化、用户注册 |
| `gitim status` | 查看 daemon 状态 |
| `gitim send <channel> <body>` | 发送频道消息 |
| `gitim read <channel>` | 读取频道消息 |
| `gitim channels` | 列出频道和 DM 会话 |
| `gitim create-channel <name>` | 创建新频道 |
| `gitim users` | 列出用户 |
| `gitim search [query]` | 搜索消息 |
| `gitim reindex` | 重建搜索索引 |
| `gitim dm send <handler> <body>` | 发送私信 |
| `gitim dm read <handler>` | 读取私信 |
| `gitim dm list` | 列出私信会话 |
| `gitim tui` | 启动终端界面 |
| `gitim webui` | 启动浏览器界面 |
| `gitim stop` | 停止 daemon |

### `onboard` 参数

| 参数 | 必填 | 说明 |
|------|------|------|
| `repo_name` | 非 `--refresh` 时 MUST | 仓库名称 |
| `org` | MAY | GitHub/Gitea/GitLab 上的 owner 或组织名 |
| `--git-server <type>` | MAY | `git` / `github` / `gitea` / `gitlab`，默认 `github` |
| `--token <token>` | `github` / `gitea` / `gitlab` 时 MUST | 平台 API token |
| `--handler <handler>` | `git` 模式 MUST | 本地模式下直接指定 handler |
| `--display-name <name>` | `git` 模式 MUST | 本地模式下直接指定显示名 |
| `--url <url>` | `gitea` / `gitlab` 时 MUST | 平台服务地址 |
| `--refresh` | MAY | 在已有工作副本里重新推断身份 |
| `--debug-http` | MAY | 开启 daemon HTTP 调试端口 |
| `--admin` | MAY | admin 模式，`poll` 返回所有内容 |
| `--with-webui` | MAY | onboard 完成后直接启动 WebUI |
| `--webui-port <port>` | MAY | WebUI 端口，默认 `6868` |
| `--webui-dev` | MAY | WebUI 开发模式（启用 Vite HMR） |

### 常用命令参数

- `gitim send <channel> <body> [-a handler] [-r line]`
- `gitim read <channel> [-l limit] [-s since]`
- `gitim search [query] [-a author] [-c channel] [-t channel|dm] [--offset n]`
- `gitim create-channel <name> [--display-name text] [--introduction text]`
- `gitim dm send <handler> <body> [-a handler] [-r line]`

---

## Daemon 自动启动

除首次 `onboard` 外，所有依赖 daemon 的命令都会先执行：

```text
findRepoRoot()  → 向上查找本地 .gitim/ 目录
ensureDaemon()  → 检查 PID → 清理 stale 文件 → spawn daemon → 轮询 socket 就绪
GitimClient()   → 连接 Unix socket
调用 API → 输出结果
```

用户通常不需要手动启动 daemon。

---

## Socket 通信

CLI 通过 Node.js `net.createConnection()` 连接 Unix socket，使用行分隔 JSON 协议。

`GitimClient` 当前封装的 API 方法包括：

- `status()`
- `send()`
- `read()`
- `listChannels()`
- `listUsers()`
- `getThread()`
- `registerUser()`
- `onboard()`
- `joinChannel()`
- `leaveChannel()`
- `createChannel()`
- `stop()`
- `poll()`
- `search()`
- `reindex()`

`send()` 的 `author` 参数可选。省略时 daemon 使用 `.gitim/me.json` 中的当前身份；`-a` 主要用于调试覆盖。

---

## 设计决策

- **薄客户端**：文件写入、Git 同步、索引、权限校验统一收敛到 daemon。
- **`onboard` 是统一入口**：覆盖 clone、创建仓库、身份注册三类场景。
- **位置参数优先**：`send` / `read` 用位置参数传 channel，减少常用命令噪音。
- **CLI 负责本地体验，daemon 负责协议正确性**：前者偏 UX，后者偏一致性。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `cli/src/index.ts` | commander 入口，注册所有命令 |
| `cli/src/client.ts` | GitimClient，Unix socket 通信封装 |
| `cli/src/daemon.ts` | daemon 自动启动、stale 清理、socket 轮询 |
| `cli/src/commands/onboard.ts` | onboard 命令实现 |
| `cli/src/commands/send.ts` | send 命令 |
| `cli/src/commands/read.ts` | read 命令 |
| `cli/src/commands/create-channel.ts` | create-channel 命令 |
| `cli/src/commands/search.ts` | search 命令 |
| `cli/src/commands/reindex.ts` | reindex 命令 |
| `cli/src/commands/dm.ts` | dm 子命令（send/read/list） |
| `cli/src/commands/tui.ts` | TUI 启动入口 |
| `cli/src/commands/webui.ts` | WebUI 启动入口 |
