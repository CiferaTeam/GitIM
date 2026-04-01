# Daemon 引擎

> GitIM v0.1 Schema

---

## Crate 架构

```
crates/
├── gitim-core/     # 类型定义、解析器、格式化器、验证器（无 IO）
├── gitim-daemon/   # 服务器、API、生命周期管理（依赖 core + sync）
└── gitim-sync/     # Git 操作、文件监听、同步循环（依赖 core）
```

Daemon 是唯一的二进制文件，链接所有三个 crate。

---

## 合规性检查

验证逻辑统一在 daemon 中实现，分两层：

### 写入验证（主防线）

通过 CLI/SDK 写入消息时，daemon 在写文件前执行检查，任一失败则拒绝写入：

| 检查项 | 规则 |
|--------|------|
| 行号连续性 | 新行号 MUST 从文件最大行号 +1 开始，严格递增且连续 |
| 行号格式 | MUST 匹配 `\[L\d{6,}\]` |
| 消息格式 | 起始行 MUST 匹配完整前缀正则 |
| 作者验证 | handler MUST 在 `users/` 中存在 |
| P 引用有效性 | 引用的行号 MUST 已存在（`P000000` 除外） |
| Mention 验证 | `<@handler>` 中的 handler MUST 已注册 |
| 追加式约束 | 已有行 MUST NOT 被修改或删除 |

### 读取检测（第二防线）

每次 git pull 拉取新内容时，daemon 增量解析并执行相同检查。不合规的行：

1. 标记为 `corrupted`，不纳入正常消息索引。
2. 输出告警日志。
3. 保留原始数据不丢弃。

确保绕过 SDK 直接 git commit/push 的不合规内容也能被发现。

---

## 并发冲突解决

乐观锁策略：

```
1. 读取文件尾部，获取当前最大行号 N
2. 生成消息，行号从 N+1 开始
3. git add + commit
4. git push
   - 成功 → 完成
   - 失败 → git pull --rebase
     → 重新读取最大行号
     → 重新分配行号（renumber）
     → 更新批次内的 P 字段引用
     → 引用已提交消息的 P 值保持不变
     → 重新 commit + push
   - 最多重试 3 次，仍失败则返回错误
```

冲突重试由 daemon 负责执行。

---

## Daemon 生命周期

### Lazy 启动

```
CLI 命令执行
  → 检查 .gitim/run/gitim.pid 是否存在且进程存活
    → 是：读取 socket 路径，发送请求
    → 否：fork daemon → 等待 socket 就绪（5 秒超时）→ 发送请求
```

Daemon 一旦启动**永不自动退出**，持续运行 sync loop 和 file watcher。一个 repo 对应一个独立进程。

### 异常恢复

| 状态 | 处理 |
|------|------|
| PID 存在，进程存活，socket 可连接 | 正常使用 |
| PID 存在，进程存活，socket 连接失败 | 等待重试（最多 5 秒） |
| PID 存在，进程不存在 | 清理 stale 文件，重新启动 |
| PID 不存在 | 启动新 daemon |
| Socket 存在但无 PID | 清理 stale socket，启动新 daemon |

所有恢复操作对用户静默。

### 停止

- `gitim stop` — 发送 `stop` API，daemon 优雅关闭（清理运行时文件后延迟 100ms 退出）
- 系统关机 / `kill <pid>` — SIGTERM handler 清理

### 运行时文件

| 文件 | 说明 |
|------|------|
| `gitim.pid` | daemon 进程 ID |
| `gitim.sock` | Unix Domain Socket |
| `gitim.port` | HTTP 端口号（仅调试模式） |
| `gitim.lock` | 文件锁，防止重复启动 |

---

## API 协议

JSON 请求 / JSON 响应，通过 Unix Domain Socket 通信（行分隔 JSON）。

### 方法

| 方法 | 说明 |
|------|------|
| `send` | 发送消息（author 可选，缺省使用 current_user） |
| `read` | 读取频道/DM 消息（支持 limit / since 过滤） |
| `channels` | 列出所有频道 |
| `users` | 列出所有用户 |
| `thread` | 获取单个线程 |
| `status` | daemon 状态 |
| `register_user` | 注册新用户（创建 meta.json + git commit） |
| `stop` | 优雅停止 daemon |
| `subscribe` | 进入推送模式 |

### HTTP 调试模式

`config.yaml` 中 `debug_http: true` 时，开启 `127.0.0.1:<port>`：

- `POST /api` — 与 Unix socket 相同的 JSON API
- `GET /api/events` — SSE 端点

---

## Push Layer

### Event 模型

```json
{
  "event": "thread_changed",
  "channel": "general",
  "kind": "channel"
}
```

| 字段 | 说明 |
|------|------|
| `event` | 事件类型，v0.1 仅 `"thread_changed"` |
| `channel` | 频道名或 DM 文件名（如 `alice--bob`） |
| `kind` | `"channel"` 或 `"dm"`，由文件名中是否含 `--` 判断 |

### 广播机制

基于 tokio `broadcast` channel（容量 256）。两个事件源：

| 触发源 | 时机 |
|--------|------|
| `handle_send` | 消息成功写入文件后 |
| File watcher | 检测到 `.thread` 文件变更（如 git pull 后） |

两者互补：本地写入即时推送，远端同步变更也能推送。

### Unix Socket 订阅

发送 `{"method": "subscribe"}` 后进入推送模式：

- 收到广播事件 → 推送 Event JSON 行
- 仍可发送请求（`tokio::select!` 同时监听）
- Lag 时静默跳过，不断开连接
- 未发送 subscribe 的连接不会收到推送

### HTTP SSE

`GET /api/events`，连接即订阅，每个事件作为 SSE `data` 字段发送。仅在 `debug_http: true` 时可用。

---

## Git 同步

### 操作

| 操作 | 说明 |
|------|------|
| `pull_rebase` | `git pull --rebase` |
| `add_and_commit` | `git add <files> && git commit` |
| `push` | `git push` |
| `push_with_retry` | push 失败时 pull + renumber + 重试（最多 3 次） |

### Sync Loop

后台定时 `git pull` + `git push`，间隔由 `sync_interval` 控制（默认 30 秒，0 = 禁用）。无 remote 时自动禁用。

### File Watcher

使用 `notify` crate 监控 `channels/` 和 `dm/` 目录，区分 `.thread` 和 `.meta.yaml` 事件，通过 tokio mpsc 转发给 daemon 主循环。文件变更时清除 thread cache 对应条目并广播 Event。

---

## 设计决策

- **Rust daemon + TS CLI 分离**：daemon 负责所有文件 IO 和 git 操作，CLI 是纯粹的客户端。避免多进程并发写文件。
- **Unix socket 而非 TCP**：本地通信，安全（文件系统权限控制），无网络暴露。
- **Lazy 启动**：用户无需手动管理 daemon，首次命令自动拉起。
- **broadcast channel 而非 per-subscriber queue**：简单，无需维护订阅者列表，Lag 时丢事件而非阻塞。
- **事件仅通知变更，不含内容**：轻量推送，客户端按需 read。避免推送大量消息内容。
- **三 crate 分离**：core 无 IO 依赖，可独立测试；sync 封装 git 操作；daemon 组装一切。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-daemon/src/main.rs` | daemon 入口、组装各组件 |
| `crates/gitim-daemon/src/server.rs` | Unix socket 服务器、subscribe 状态机 |
| `crates/gitim-daemon/src/http.rs` | HTTP 调试服务器、SSE 端点 |
| `crates/gitim-daemon/src/api.rs` | Request/Response/Event 类型定义 |
| `crates/gitim-daemon/src/handlers.rs` | 各 API 方法的处理逻辑 |
| `crates/gitim-daemon/src/state.rs` | AppState（配置、用户列表、thread cache、broadcast tx） |
| `crates/gitim-daemon/src/lifecycle.rs` | PID/Lock/Socket 管理 |
| `crates/gitim-daemon/src/error.rs` | 错误类型 |
| `crates/gitim-sync/src/git.rs` | Git 操作封装 |
| `crates/gitim-sync/src/watcher.rs` | 文件监听 |
| `crates/gitim-sync/src/sync_loop.rs` | 定时同步循环 |
| `crates/gitim-sync/src/renumber.rs` | 冲突时行号重编 |
| `crates/gitim-core/src/validator/compliance.rs` | 写入验证 |
| `crates/gitim-core/src/validator/read_check.rs` | 读取检测 |
