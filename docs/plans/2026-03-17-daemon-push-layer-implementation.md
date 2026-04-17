# Daemon Push Layer 实现计划

**状态：已完成**

**Goal:** 为 GitIM daemon 增加实时事件推送能力，支持 Unix socket subscribe 模式和 HTTP SSE。

**Architecture:** 基于 tokio broadcast channel 实现发布-订阅。AppState 持有 event_tx，handle_send 和 file watcher 作为事件源，Unix socket 和 HTTP SSE 作为消费端。

**Tech Stack:** Rust (tokio broadcast, axum SSE, async-stream, futures)

**Spec:** `docs/superpowers/specs/2026-03-17-gitim-daemon-push-layer-design.md`

---

## Dependency Graph

```
Task 1: Event 类型 + Subscribe 请求 + lib.rs
  │
  ├→ Task 2: AppState 增加 broadcast channel
  │    │
  │    ├→ Task 3: handle_send 广播事件
  │    ├→ Task 4: File watcher 广播事件
  │    ├→ Task 5: Unix socket subscribe 模式
  │    └→ Task 6: HTTP SSE 端点
  │
  └→ Task 7: 集成测试
```

---

## 文件变更清单

| 操作 | 文件 | 职责 |
|------|------|------|
| 修改 | `crates/gitim-daemon/src/api.rs` | 增加 `Event` 结构体和 `Subscribe` 请求变体 |
| 新建 | `crates/gitim-daemon/src/lib.rs` | pub mod 导出，供集成测试访问内部类型 |
| 修改 | `crates/gitim-daemon/src/state.rs` | AppState 增加 `event_tx: broadcast::Sender<Event>` |
| 修改 | `crates/gitim-daemon/src/main.rs` | 创建 broadcast channel，watcher 事件广播 |
| 修改 | `crates/gitim-daemon/src/handlers.rs` | handle_send 成功后广播 Event，Subscribe 响应 |
| 修改 | `crates/gitim-daemon/src/server.rs` | 重构为状态机：普通模式 → subscribe 后进入推送模式 |
| 修改 | `crates/gitim-daemon/src/http.rs` | 新增 `GET /api/events` SSE 端点 |
| 修改 | `crates/gitim-daemon/Cargo.toml` | 增加 async-stream, futures, reqwest(dev) 依赖 |
| 新建 | `crates/gitim-daemon/tests/push_test.rs` | 推送层集成测试 |

---

## Chunk 1: 基础设施

### Task 1: Event 类型与 Subscribe 请求

- [x] `api.rs` 新增 `Event { event, channel, kind }` 结构体（derive Serialize, Clone）
- [x] Request 枚举新增 `Subscribe` 变体
- [x] 创建 `lib.rs` 导出 pub mod api/handlers/state/server/http，供测试引用
- [x] Commit: `feat(daemon): add Event type, Subscribe request, and lib.rs for test access`

### Task 2: AppState 广播 channel

- [x] AppState 增加 `event_tx: broadcast::Sender<Event>` 字段
- [x] `AppState::new()` 签名增加 `event_tx` 参数
- [x] main.rs 创建 `broadcast::channel::<Event>(256)` 并传入
- [x] Commit: `feat(daemon): add broadcast channel to AppState`

---

## Chunk 2: 事件源

### Task 3: handle_send 广播

- [x] `handle_send` 成功写入文件后，通过 `state.event_tx.send()` 广播 Event
- [x] kind 判断：`channel.starts_with("dm:")` → "dm"，否则 → "channel"
- [x] 广播失败（无订阅者）静默忽略
- [x] Commit: `feat(daemon): broadcast event after handle_send succeeds`

### Task 4: File watcher 广播

- [x] main.rs 中 watcher 事件处理增加 Event 广播
- [x] kind 判断：文件名含 `--` → "dm"，否则 → "channel"
- [x] 同时保留原有的 cache invalidation 逻辑
- [x] Commit: `feat(daemon): broadcast event on watcher ThreadModified`

---

## Chunk 3: 消费端

### Task 5: Unix socket subscribe 模式

- [x] server.rs 重构：普通模式循环中检测 Subscribe 请求，切换到 `handle_subscribed`
- [x] `handle_subscribed` 使用 `tokio::select!` 同时监听 broadcast rx 和客户端输入
- [x] 收到广播事件 → 序列化为 JSON 行推送，写入后 flush
- [x] Lag 静默跳过，channel closed 或客户端断开时退出
- [x] Commit: `feat(daemon): implement unix socket subscribe mode with push events`

### Task 6: HTTP SSE 端点

- [x] http.rs 新增 `GET /api/events` 路由
- [x] 使用 `async_stream::stream!` + axum `Sse` 构建 SSE 响应
- [x] 连接即订阅，无需显式 subscribe
- [x] Commit: `feat(daemon): add HTTP SSE endpoint at GET /api/events`

---

## Chunk 4: 测试

### Task 7: 集成测试

- [x] 新建 `push_test.rs`，覆盖 7 个场景：
  - Event JSON 序列化
  - Event DM kind
  - Subscribe 请求反序列化
  - handle_send 广播 channel 事件
  - handle_send 广播 DM 事件
  - Unix socket subscribe 接收推送事件
  - 未 subscribe 的连接不收到推送
  - HTTP SSE 接收推送事件
- [x] Commit: `test: add push layer integration tests`
