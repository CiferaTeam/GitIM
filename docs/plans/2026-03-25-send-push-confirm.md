# Send Push Confirmation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消息发送改为同步等待 push 成功后才返回，实现最终一致性确认。

**Architecture:** send handler 写文件 + commit 后，通过 Notify 唤醒 sync_loop 立即执行一轮 push，通过 oneshot channel 等待 push 结果。无 remote 时降级为 commit 即成功。前端在发送期间显示 loading 状态并禁用输入框。

**Tech Stack:** Rust (tokio::sync::Notify, tokio::sync::oneshot), TypeScript/React (Zustand store)

---

## 背景

当前 send handler 在 git commit 后立即返回 `"committed"`，push 由 sync_loop 异步处理。用户无法确认消息是否已送达远端。

### 设计决策（来自 grill-me 会话）

| 决策 | 结论 |
|------|------|
| 无 remote 时 | commit 即成功（降级） |
| send 阻塞模式 | 同步等待 push 成功 |
| 超时/重试 | 接受 sync_loop 最多 3 轮重试 |
| push 由谁执行 | sync_loop（send 唤醒 + 等 oneshot） |
| push 失败通知 | cycle 结束后扫 pending_push，未推成功的统一通知失败 |
| 失败返回 | `"commit_only"` 状态，不回滚 |
| 前端行为 | 发送中禁用输入框，显示 "发送中..." |
| meta 冲突丢消息 | v1 不处理 |

### 关键文件地图

| 文件 | 职责 |
|------|------|
| `crates/gitim-daemon/src/state.rs` | PendingMessage 结构体、AppState（加 Notify）、spawn_sync_loop 回调 |
| `crates/gitim-sync/src/sync_loop.rs` | sync loop 主循环（加 Notify 监听）、run_sync_cycle |
| `crates/gitim-daemon/src/handlers.rs` | handle_send（加 await oneshot） |
| `crates/gitim-daemon/src/api.rs` | Response 结构体 |
| `crates/gitim-sync/src/git.rs` | has_remote() |
| `webui/src/hooks/useConnection.ts` | 前端 send 调用 |
| `webui/src/hooks/useStore.ts` | 前端状态管理 |
| `webui/src/components/InputArea.tsx` | 输入框 UI |

---

## Chunk 1: Daemon 端改造

### Task 1: PendingMessage 加 oneshot sender

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs:12-16`（PendingMessage 结构体）

**变更描述：**
- PendingMessage 新增 `result_tx: Option<oneshot::Sender<PushResult>>` 字段
- 定义 `PushResult` 枚举：`Pushed { commit_id: String }` 和 `Failed { reason: String }`
- `result_tx` 为 Option 是因为 sync_loop 拉取远端变化时产生的 pending 项不需要等待

**验收标准：**
- `cargo build` 通过
- 现有测试 `cargo test -p gitim-daemon` 通过（可能需要调整构造 PendingMessage 的地方）

**Commit:** `feat(state): add oneshot sender to PendingMessage for push confirmation`

---

### Task 2: AppState 加 Notify + has_remote 标志

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs:18-46`（AppState 结构体和 new 方法）

**变更描述：**
- AppState 新增 `pub push_notify: Arc<tokio::sync::Notify>` 字段，用于 send handler 唤醒 sync_loop
- AppState 新增 `pub has_remote: bool` 字段，daemon 启动时确定，避免每次 send 都检查
- `new()` 方法中初始化这两个字段，`has_remote` 通过 `GitStorage::has_remote()` 获取

**验收标准：**
- `cargo build` 通过
- `cargo test -p gitim-daemon` 通过

**Commit:** `feat(state): add push_notify and has_remote to AppState`

---

### Task 3: sync_loop 接收 Notify，用 select! 替换纯 ticker

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs:16-49`（start_sync_loop 签名和主循环）

**变更描述：**
- `start_sync_loop` 新增参数 `push_notify: Arc<tokio::sync::Notify>`
- 主循环从 `ticker.tick().await` 改为 `tokio::select!` 同时监听 ticker 和 notify
- 无论哪种触发方式，都调用同一个 `run_sync_cycle`
- 新增：`run_sync_cycle` 返回后，新增 `on_push_cycle_done` 回调（或复用现有回调），用于通知未推成功的等待者失败

**验收标准：**
- sync_loop 在收到 Notify 时立即执行一轮 cycle
- 周期性 tick 不受影响
- `cargo test -p gitim-sync` 通过

**Commit:** `feat(sync_loop): accept Notify for on-demand push trigger`

---

### Task 4: spawn_sync_loop 传递 Notify + 处理 push 完成通知

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs:93-196`（spawn_sync_loop 方法）

**变更描述：**
- 从 `state.push_notify.clone()` 获取 Notify，传给 `start_sync_loop`
- **on_pushed 回调改造（110-124 行区域）：** drain pending_push 时，对每个有 `result_tx` 的条目，发送 `PushResult::Pushed { commit_id }` 。需要把当前 HEAD commit_id 传入回调（可通过新增参数或在回调内 rev_parse）
- **新增 cycle 结束处理：** `run_sync_cycle` 返回后，扫描 pending_push 中仍有 `result_tx` 的条目（说明 push 未成功），发送 `PushResult::Failed`，并从 pending_push 中移除这些条目

**验收标准：**
- push 成功时，所有带 result_tx 的 pending 条目收到 Pushed
- push 失败（cycle 结束但未推成功）时，等待者收到 Failed
- `cargo test -p gitim-daemon` 通过

**Commit:** `feat(state): wire Notify and push result notification in spawn_sync_loop`

---

### Task 5: handle_send 等待 push 结果

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs:160-275`（handle_send 函数）

**变更描述：**
- 在记录 pending_push 时（248-254 行区域），创建 oneshot channel，把 sender 放入 PendingMessage，自己持有 receiver
- commit 后检查 `state.has_remote`：
  - **有 remote：** 调 `state.push_notify.notify_one()` 唤醒 sync_loop，然后 `await receiver`，根据结果返回 `status: "pushed"` 或 `status: "commit_only"` + error 信息
  - **无 remote：** 跳过等待，直接返回 `status: "committed"`（降级行为）
- 返回体新增 `commit_id` 字段（push 成功时）

**验收标准：**
- 有 remote 时，send 阻塞直到 push 完成才返回
- 无 remote 时，send 立即返回 `"committed"`
- push 成功返回 `{ status: "pushed", line_number, channel, commit_id }`
- push 失败返回 `{ status: "commit_only", line_number, channel, error }`
- `cargo test -p gitim-daemon` 通过

**Commit:** `feat(handlers): send awaits push confirmation before returning`

---

### Task 6: Daemon 端集成测试

**Files:**
- Modify: `crates/gitim-daemon/tests/commit_test.rs`（现有 send 测试）
- Modify: `crates/gitim-sync/tests/sync_e2e_test.rs`（现有 sync 测试）

**变更描述：**
- **无 remote 测试：** 在无 remote 仓库中 send，验证立即返回 `status: "committed"`
- **有 remote 测试：** 用 local bare repo 做 remote，send 后验证返回 `status: "pushed"` 且 remote 上可见变更
- **并发 send 测试：** 连续发两条消息，验证都返回 `"pushed"` 且 line_number 正确递增
- **push 冲突测试：** 在 remote 上制造冲突（另一个 clone 先 push），验证 send 仍能通过 rebase 成功返回

**验收标准：**
- 所有新测试通过
- 所有现有测试通过：`cargo test`

**Commit:** `test: add send-push-confirm integration tests`

---

## Chunk 2: 前端适配

### Task 7: send API 响应适配

**Files:**
- Modify: `webui/src/lib/types.ts`（ApiResponse 类型）
- Modify: `webui/src/hooks/useConnection.ts:29-62`（request 方法）

**变更描述：**
- 前端 send 调用的 response 格式变化：新增 `status` 和 `commit_id` 字段
- useConnection 的 `request('send', ...)` 需要处理新的 status 值：
  - `"pushed"` → 发送成功，更新 store 中消息的 commit_id
  - `"commit_only"` → 发送未送达，标记消息状态为 failed
  - `"committed"` → 无 remote 降级，视同成功

**验收标准：**
- 三种 status 都能正确处理
- `npm run build`（webui 目录）通过

**Commit:** `feat(webui): handle push confirmation status in send response`

---

### Task 8: 发送中 UI 状态

**Files:**
- Modify: `webui/src/components/InputArea.tsx`（发送按钮和输入框）
- Modify: `webui/src/App.tsx:41-86`（handleSend 函数）

**变更描述：**
- InputArea：发送中（`sending=true`）时禁用输入框和发送按钮，显示 "发送中..." 文案
- App.tsx handleSend：目前已经有 pending message 的乐观渲染（addPendingMessage、markPendingSent、markPendingFailed）。需要确认这套流程在 send 阻塞更长时间（1-15 秒）后仍然正常工作
- 关键：send 返回 `"commit_only"` 时调用 `markPendingFailed`，让用户知道消息未送达

**验收标准：**
- 发送期间输入框和按钮不可交互
- 显示 "发送中..." 提示
- push 成功后恢复可交互状态
- push 失败后恢复可交互状态并标记消息失败
- `npm run build`（webui 目录）通过

**Commit:** `feat(webui): disable input during send, show loading state`

---

### Task 9: 端到端仿真验证

**Files:**
- Modify: `sim/webui-sim.sh`（仿真脚本，改为使用有 remote 的 bare repo）

**变更描述：**
- 修改仿真脚本：创建 local bare repo 作为 remote，clone 到工作目录
- 运行 2-agent 聊天，观察：
  - daemon 日志中 send 是否在 push 后才返回
  - WebUI 中消息状态是否正确变化（sending → sent）
  - 断开 remote 权限后 send 是否返回 `"commit_only"`

**验收标准：**
- 正常场景：消息发送后 1-2 秒内返回 pushed
- 断网场景：返回 commit_only，消息标记为失败
- 所有 Rust 测试通过：`cargo test`
- WebUI 构建通过：`cd webui && npm run build`

**Commit:** `test(sim): update webui-sim for push confirmation verification`
