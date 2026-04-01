# Daemon 引擎

> GitIM 当前实现（daemon / API / sync）

---

## Crate 架构

```text
crates/
├── gitim-core/    # 类型、解析、格式化、验证（无 IO）
├── gitim-daemon/  # API、onboard、生命周期、服务入口
├── gitim-sync/    # Git 操作、冲突解决、watcher、sync loop
└── gitim-index/   # SQLite FTS5 搜索索引
```

`gitim-daemon` 是唯一二进制，链接其余三个 crate。

---

## 合规性检查

### 写入验证（已接入）

daemon 在写入消息前会执行 `validate_append()`，拒绝以下情况：

- 行号不连续
- 作者未注册
- `P` 引用不存在
- 协议级 mention 指向未知用户
- 空消息正文
- 非成员向受限频道发消息

### 读取侧完整性检测（当前状态）

`gitim-core` 提供 `read_check` helper，可用于发现 gap、未知作者、非法引用等问题。

当前运行时的实际行为是：

- `read` / `poll` / 索引路径遇到无法解析的内容时，返回错误或输出 `warn`
- 搜索索引会跳过损坏的 `.thread` 内容
- 尚未把 `corrupted` 作为统一 API 类型暴露给客户端

---

## 消息写入与并发

当前写入路径：

```text
1. 解析现有线程，计算 next_line
2. format_message() 生成新增内容
3. validate_append() 校验
4. 追加写入文件
5. best-effort git add + commit
6. 若存在 remote 且 sync loop 已启动，则等待 push 结果
```

冲突时由 `gitim-sync` 执行 rebase + renumber，保持：

- 本批次内 `P` 引用跟随新行号更新
- 指向远端已有消息的 `P` 保持不变

---

## Daemon 生命周期

### Lazy 启动

```text
CLI 命令执行
  → 检查 .gitim/run/gitim.pid
  → 如有必要清理 stale 文件
  → spawn gitim-daemon
  → 轮询 gitim.sock 就绪
```

daemon 启动后持续运行 sync loop、watcher、搜索索引和 socket/HTTP 服务。

### 停止

- `gitim stop`：发送 `stop` API，daemon 清理运行时文件后延迟退出
- `Ctrl-C` / 进程终止：daemon 尽量清理 `.gitim/run/` 下的 PID / socket / lock

### 运行时文件

| 文件 | 说明 |
|------|------|
| `gitim.pid` | daemon 进程 ID |
| `gitim.sock` | Unix Domain Socket |
| `gitim.port` | HTTP 调试端口（仅开启 debug_http 时） |
| `gitim.lock` | 预留锁文件 |

---

## API 协议

Unix socket 上使用行分隔 JSON。

### Request 方法

| 方法 | 说明 |
|------|------|
| `status` | daemon 状态 |
| `send` | 发送消息 |
| `read` | 读取线程内容 |
| `channels` | 列出频道和 DM 会话 |
| `users` | 列出用户 |
| `thread` | 获取某个根消息的完整线程树 |
| `subscribe` | 进入事件推送模式 |
| `stop` | 停止 daemon |
| `poll` | 基于 commit hash 拉取增量变更 |
| `register_user` | 注册用户（创建 `users/<handler>.meta.yaml`） |
| `onboard` | 执行完整 onboard 编排 |
| `join_channel` | 频道加人 / 自加入 |
| `leave_channel` | 频道移除 / 自离开 |
| `create_channel` | 创建新频道 |
| `search` | 搜索消息 |
| `reindex` | 重建搜索索引 |

### HTTP 调试模式

启用 `debug_http: true` 后，daemon 还会暴露：

- `POST /api`：与 Unix socket 相同的 JSON API
- `GET /api/events`：SSE 事件流

CLI WebUI bridge 额外提供 `/api/poll` 等 HTTP 包装接口，供浏览器使用。

---

## 事件模型

### 当前事件类型

| 事件 | 字段 | 说明 |
|------|------|------|
| `thread_changed` | `channel`, `kind` | 本地写入或 watcher 观察到线程变更 |
| `messages_pushed` | `channel`, `line_numbers` | sync loop 成功 push 后确认哪些行已到远端 |
| `message_renumbered` | `channel`, `old_line`, `new_line` | rebase 冲突后本地消息被重编号 |
| `membership_changed` | `channel`, `event_type`, `author`, `targets` | join / leave 等成员变更 |

### 广播机制

daemon 通过 tokio `broadcast` channel 推送事件：

- `handle_send` 本地写入后立即发送 `thread_changed`
- sync loop push 成功后发送 `messages_pushed`
- rebase 重编号时发送 `message_renumbered`
- watcher 观察到线程文件变更时再次发送 `thread_changed`

### 订阅

- Unix socket：先发 `{"method":"subscribe"}`，之后可同时接收事件和继续发请求
- HTTP：直接连接 `GET /api/events`

Lag 时会跳过旧事件而不是阻塞整个系统。

---

## Git 同步

### 主要操作

| 操作 | 说明 |
|------|------|
| `add_and_commit[_as]` | 写入后提交本地 commit |
| `push` | 推送到远端 |
| `pull_rebase` | 拉取并 rebase |
| `discard_unpushed` | 冲突恢复时丢弃本地未推送 commit |
| `diff_range` / `diff_unpushed` | 生成增量内容供 `poll` 或索引使用 |

### Sync Loop

- 默认 `sync_interval = 1`
- `0` 表示禁用后台定时同步
- 无 remote 时不会等待 push 结果

### File Watcher

watcher 监控 `channels/` 和 `dm/` 目录下的 `.thread` / `.meta.yaml` 变化。
当前持久化的 DM 数据只有 `.thread`；`.meta.yaml` 主要用于频道元信息。

---

## 设计决策

- **Rust daemon + TS CLI 分离**：避免多进程直接并发写文本文件。
- **Unix socket 为主，HTTP 仅调试/桥接**：保持本地通信默认安全边界。
- **事件只推“变了什么”，不推“完整内容”**：让客户端按需 `read` 或 `poll`。
- **搜索索引本地化**：SQLite 只服务当前工作副本，不污染共享 Git 数据。

## 涉及源文件

| 文件 | 职责 |
|------|------|
| `crates/gitim-daemon/src/main.rs` | daemon 入口与组件组装 |
| `crates/gitim-daemon/src/server.rs` | Unix socket 服务与 subscribe 状态机 |
| `crates/gitim-daemon/src/http.rs` | HTTP 调试服务与 SSE |
| `crates/gitim-daemon/src/api.rs` | Request / Response / Event 类型 |
| `crates/gitim-daemon/src/handlers.rs` | API 方法处理逻辑 |
| `crates/gitim-daemon/src/onboard.rs` | onboard 编排 |
| `crates/gitim-daemon/src/state.rs` | AppState、sync loop 协调、索引初始化 |
| `crates/gitim-sync/src/git.rs` | Git 操作封装 |
| `crates/gitim-sync/src/sync_loop.rs` | 后台同步循环 |
| `crates/gitim-sync/src/conflict.rs` | 冲突解决与成员列表并集合并 |
| `crates/gitim-sync/src/watcher.rs` | 文件监听 |
| `crates/gitim-index/src/lib.rs` | FTS5 索引 |
