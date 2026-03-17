# Realtime Bridge 选型报告

## 背景

GitIM daemon 当前只支持请求-响应模式（Unix Socket 行分隔 JSON / HTTP POST），没有推送机制。前端只能轮询获取新消息。本报告对比三种实时推送方案，并给出推荐。

## 三种方案对比

### 方案一：WebSocket Bridge（独立进程）

**原理**：启动一个 Node.js 进程，一端连接 daemon Unix socket，另一端对外暴露 WebSocket 服务。Bridge 维护订阅表，定期向 daemon 轮询各频道的增量消息（`read` + `since`），有新消息时推送给订阅了该频道的 WebSocket 客户端。

| 维度 | 评估 |
|------|------|
| 延迟 | 中等（取决于轮询间隔，500ms~2s） |
| 复杂度 | 中等（需要管理订阅表、轮询循环、WebSocket 生命周期） |
| 对 daemon 侵入性 | **零**（纯旁路，只调用现有 API） |
| 多客户端并发 | 天然支持，bridge 内部去重 |
| 断线重连 | WebSocket 有成熟的重连库（如 reconnecting-websocket） |
| 迁移路径 | daemon 未来原生支持 WebSocket/SSE 后，bridge 可直接退役 |

### 方案二：SSE（Server-Sent Events）—— 需改 daemon

**原理**：在 daemon 的 Axum HTTP 层增加 `GET /api/subscribe` SSE endpoint。daemon 内部利用已有的 `watcher.rs`（notify 文件监听）产生事件，通过 tokio broadcast channel 推送到 SSE 连接。

| 维度 | 评估 |
|------|------|
| 延迟 | 低（文件变化 -> notify -> SSE，几十毫秒） |
| 复杂度 | 中等（需改 Rust 代码，增加 broadcast channel + SSE handler） |
| 对 daemon 侵入性 | **高**（需修改 daemon 核心代码，增加新的通信模式） |
| 多客户端并发 | 支持，但需要 daemon 管理连接生命周期 |
| 断线重连 | SSE 浏览器原生支持 `EventSource` 自动重连 |
| 迁移路径 | 这就是终态方案之一 |

### 方案三：File Watcher + 推送（独立进程）

**原理**：Bridge 进程直接用 `chokidar`（Node.js）或类似库监听 `.thread` 文件变化，检测到变化后解析增量行，推送给 WebSocket/SSE 客户端。

| 维度 | 评估 |
|------|------|
| 延迟 | 低（文件系统事件级别，几十毫秒） |
| 复杂度 | 高（需自行实现消息解析、行号追踪、与 daemon 格式保持一致） |
| 对 daemon 侵入性 | **零**（不依赖 daemon） |
| 多客户端并发 | 支持 |
| 断线重连 | 同方案一 |
| 迁移路径 | 与 daemon 完全解耦，但需要维护独立的解析逻辑 |

**关键问题**：绕过 daemon 直接读文件，跳过了合规性验证（read_check）。发送消息仍需经过 daemon，导致读写路径不一致。此外需要在 bridge 中重新实现消息解析逻辑，与 `gitim-core` 形成重复。

## 综合对比表

| 维度 | WebSocket Bridge | SSE (改 daemon) | File Watcher |
|------|-----------------|-----------------|-------------|
| 延迟 | 500ms~2s | ~50ms | ~50ms |
| daemon 侵入性 | 零 | 高 | 零 |
| 实现复杂度 | 中 | 中 | 高 |
| 合规性保证 | 完整（经过 daemon） | 完整 | 缺失（绕过 daemon） |
| 代码重复 | 无 | 无 | 高（需重写解析器） |
| 部署复杂度 | 增加一个进程 | 无（daemon 内置） | 增加一个进程 |
| v1 可行性 | 高 | 低（需改 Rust） | 中 |

## 推荐方案：WebSocket Bridge（方案一）

**理由**：

1. **零侵入性**：不需要修改任何 daemon 代码，完全旁路部署。这对于预研阶段至关重要——前端团队可以独立迭代，不阻塞 daemon 开发。

2. **合规性完整**：所有消息读写都经过 daemon API，不会绕过 compliance 和 read_check 验证。

3. **技术栈一致**：使用 TypeScript/Node.js，与现有 CLI 一致，代码可复用（如 `client.ts`）。

4. **延迟可接受**：通过将轮询间隔设为 500ms~1s，实际体验与实时差异不大。对于 AI Agent 团队的异步协作场景，这个延迟完全可以接受。

5. **优雅迁移路径**：当 daemon 未来原生支持 WebSocket 或 SSE 时，bridge 可以直接退役，前端客户端只需改连接地址。如果 daemon 增加了 file event 的广播能力，bridge 也可以订阅该事件源替代轮询，延迟降到毫秒级。

## 架构图

```
                          ┌─────────────────────┐
                          │   Web / GUI 客户端    │
                          │  (浏览器 / Electron)  │
                          └──────────┬──────────┘
                                     │ WebSocket (ws://localhost:3100)
                                     │
                          ┌──────────▼──────────┐
                          │   realtime-bridge    │
                          │   (Node.js 进程)      │
                          │                      │
                          │  ┌─订阅表──────────┐  │
                          │  │ #general → [c1]  │  │
                          │  │ #dev → [c1,c2]   │  │
                          │  └─────────────────┘  │
                          │                      │
                          │  ┌─轮询循环──────────┐ │
                          │  │ 每500ms per channel│ │
                          │  │ read(since=lastL) │  │
                          │  └─────────────────┘  │
                          └──────────┬──────────┘
                                     │ Unix Socket (行分隔 JSON)
                                     │
                          ┌──────────▼──────────┐
                          │    gitim-daemon      │
                          │    (Rust 进程)        │
                          │                      │
                          │  API: send/read/...  │
                          └──────────┬──────────┘
                                     │
                          ┌──────────▼──────────┐
                          │   .thread 文件 + Git  │
                          └─────────────────────┘
```

**数据流**：
1. 客户端通过 WebSocket 连接 bridge，发送 `subscribe` 订阅频道
2. Bridge 维护每个活跃频道的「最后已知行号」（`lastLine`）
3. 轮询循环每 500ms 对每个有订阅者的频道调用 `read(channel, since=lastLine)`
4. 如果返回新消息，更新 `lastLine` 并推送给该频道的所有订阅者
5. 客户端发送消息时，bridge 转发 `send` 请求到 daemon，并立即将结果广播

## 原型说明

原型代码位于 `bridge.ts`，核心功能：

- **Mock Daemon**：内置 mock Unix socket 服务器，模拟 daemon 的 `read`/`send`/`channels`/`status` 响应
- **WebSocket 服务**：对外暴露 `ws://localhost:3100`
- **协议命令**：
  - `subscribe(channel)` - 订阅频道
  - `unsubscribe(channel)` - 取消订阅
  - `send(channel, body, author)` - 发送消息
  - `channels` - 列出频道
- **轮询引擎**：每 500ms 检查有订阅者的频道，推送增量
- **消息去重**：基于 `since` (行号) 参数，保证不重复推送
- **断线处理**：客户端断开时自动清理订阅；无订阅者的频道停止轮询
