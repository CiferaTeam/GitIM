# GitIM Daemon Push Layer 设计文档

> **实时事件推送机制**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 概述

为 GitIM daemon 增加实时事件推送能力，让订阅客户端无需轮询即可获知消息变更。

**核心目标：**

- 消息写入或文件变更时，所有订阅者立即收到事件通知
- Unix socket 客户端通过 `subscribe` 方法进入推送模式
- HTTP 调试模式通过 SSE（Server-Sent Events）推送
- 事件轻量：仅通知"哪个频道/DM 有变更"，不携带消息内容

**不在范围：**

- 消息内容推送（客户端收到事件后需自行 `read`）
- 事件持久化或重放
- 客户端过滤（按频道/DM 订阅）

---

## 2. Event 模型

### 2.1 Event 结构

```json
{
  "event": "thread_changed",
  "channel": "general",
  "kind": "channel"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `event` | string | 事件类型，v1 仅 `"thread_changed"` |
| `channel` | string | 频道名或 DM 文件名（如 `alice--bob`） |
| `kind` | string | `"channel"` 或 `"dm"`，由文件名中是否含 `--` 判断 |

### 2.2 kind 判断规则

handler 和 channel 名称 MUST NOT 包含连续连字符（spec §3.2, §4.1），因此 `--` 仅出现在 DM 文件名的分隔符位置，可作为判断依据。

---

## 3. 广播机制

### 3.1 AppState 扩展

AppState 增加 `event_tx: broadcast::Sender<Event>`，所有需要推送事件的组件共享同一个 sender。

### 3.2 事件触发点

| 触发源 | 时机 | 说明 |
|--------|------|------|
| `handle_send` | 消息成功写入文件后 | 本地写入触发 |
| File watcher | 检测到 `.thread` 文件变更 | 外部变更触发（如 git pull 后文件更新） |

两个触发源互补：`handle_send` 保证本地写入的即时性，file watcher 保证远端同步过来的变更也能推送。

---

## 4. Unix Socket 订阅模式

### 4.1 协议扩展

新增 `subscribe` 请求方法：

```json
{"method": "subscribe"}
```

响应：

```json
{"ok": true, "data": {"subscribed": true}}
```

### 4.2 连接状态机

```
普通模式（request-response）
  │
  ├─ 收到 subscribe 请求 → 返回响应 → 进入推送模式
  │
推送模式
  ├─ 收到广播事件 → 推送 Event JSON 行
  ├─ 收到客户端请求 → 正常处理并响应
  └─ 客户端断开 → 清理
```

推送模式下客户端仍可发送请求，通过 `tokio::select!` 同时监听广播和客户端输入。

### 4.3 未订阅的连接

不发送 `subscribe` 的连接不会收到任何推送事件，行为与之前完全一致。

### 4.4 Lag 处理

使用 tokio `broadcast` channel（容量 256）。当订阅者消费速度跟不上时，`RecvError::Lagged` 静默跳过丢失的事件，不断开连接。

---

## 5. HTTP SSE 端点

### 5.1 端点

```
GET /api/events
```

仅在 `debug_http: true` 时可用。

### 5.2 行为

- 建立 SSE 连接后立即开始接收事件
- 无需显式 subscribe（连接即订阅）
- 每个事件作为一个 SSE `data` 字段发送，内容为 Event 的 JSON 序列化
- Lag 时静默跳过，channel closed 时断开

---

## 6. 边界情况

| 条件 | 规则 |
|------|------|
| 无订阅者时广播 | `event_tx.send()` 返回 Err（无接收者），忽略即可 |
| 订阅者断开 | 写入失败时清理连接，不影响其他订阅者 |
| 多个订阅者 | 每个订阅者独立接收，互不干扰 |
| 同一事件被 handle_send 和 watcher 双重触发 | 可能发生，客户端 SHOULD 做幂等处理 |
| daemon 重启 | 所有订阅断开，客户端需重新连接和订阅 |
