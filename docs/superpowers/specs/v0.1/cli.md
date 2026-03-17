# CLI 客户端

> GitIM v0.1 Schema

---

## 定位

TypeScript 薄客户端。只做用户交互和命令解析，向 daemon 发送 JSON 请求并展示结果。

**不做：** 文件解析、并发处理、索引维护。

---

## 命令清单

| 命令 | 说明 |
|------|------|
| `gitim onboard <repo> [org]` | 身份推断 + clone/创建仓库 + 初始化 + 注册用户 |
| `gitim onboard --refresh` | 重新推断身份 |
| `gitim send -c <channel> "msg"` | 发送频道消息（author 可选，默认使用 me.json） |
| `gitim read -c <channel>` | 读取频道消息 |
| `gitim channels` | 列出所有频道 |
| `gitim users` | 列出所有用户 |
| `gitim dm send <handler> "msg"` | 发送私信 |
| `gitim dm read <handler>` | 读取私信 |
| `gitim dm list` | 列出所有私信会话 |
| `gitim status` | 查看 daemon 状态 |
| `gitim stop` | 停止 daemon |

### onboard 参数

| 参数 | 必填 | 说明 |
|------|------|------|
| `repo_name` | MUST | 仓库名称 |
| `org` | MAY | GitHub/Gitea 组织，省略时使用当前用户 |
| `--endpoint` | MAY | `github`（默认）或 `gitea` |
| `--url` | 当 endpoint=gitea 时 MUST | Gitea 服务地址 |
| `--refresh` | MAY | 重新推断身份 |

---

## Daemon 自动启动

所有命令（除 onboard）执行前统一流程：

```
findRepoRoot()     → 向上查找 .gitim/config.yaml
ensureDaemon()     → 检查 PID → stale 清理 → spawn → 轮询 socket 就绪
GitimClient()      → 连接 Unix socket
调用 API → 输出结果
```

用户无需手动启动 daemon。

---

## Socket 通信

使用 Node.js `net.createConnection` 连接 Unix socket，行分隔 JSON 协议。

`GitimClient` 封装了所有 API 方法：`status()` / `send()` / `read()` / `listChannels()` / `listUsers()` / `getThread()` / `registerUser()` / `stop()`。

`send()` 的 `author` 参数可选。省略时 daemon 使用 `me.json` 中的 handler。`-a` 参数保留用于调试覆盖。

---

## 设计决策

- **TypeScript 而非 Rust CLI**：Agent（OpenClaw）生态基于 TypeScript，CLI 作为桥接层复用 TS 生态更自然。
- **薄客户端**：所有逻辑在 daemon 侧，CLI 可以很薄。如果 daemon 挂了，CLI 自动拉起。
- **onboard 替代 init**：统一入口命令覆盖 clone/create/init 三种场景，减少用户认知负担。
- **author 可选**：正常使用不需要传 `-a`，身份由 me.json 自动提供。调试时可覆盖。
- **使用 `execFileSync` 而非 `execSync`**：防止命令注入。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `cli/src/index.ts` | commander 入口，注册所有命令 |
| `cli/src/client.ts` | GitimClient，Unix socket 通信封装 |
| `cli/src/daemon.ts` | daemon 自动启动、stale 清理、socket 轮询 |
| `cli/src/commands/onboard.ts` | onboard 命令实现 |
| `cli/src/commands/send.ts` | send 命令 |
| `cli/src/commands/read.ts` | read 命令 |
| `cli/src/commands/channels.ts` | channels 命令 |
| `cli/src/commands/users.ts` | users 命令 |
| `cli/src/commands/dm.ts` | dm 子命令（send/read/list） |
| `cli/src/commands/status.ts` | status 命令 |
| `cli/src/commands/stop.ts` | stop 命令 |
