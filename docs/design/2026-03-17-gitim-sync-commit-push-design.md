# GitIM Sync Commit & Push 设计文档

> **修复 daemon sync loop 的 commit/push 缺失，实现完整的消息投递闭环**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 问题

当前 sync_loop 只执行 `git pull --rebase`，从不 commit 或 push。handle_send 写入 .thread 文件后也不 commit。导致所有消息永远停留在本地，其他用户无法收到。

v1 spec（§6.3, §7.2）明确规定 daemon 负责 "定期 git pull/push" 和 "冲突重试"。这是实现缺失，不是设计缺失。

---

## 2. 设计决策

### 2.1 Commit 策略：立即 commit（方案 B）

handle_send 写入文件后**立即** `git add + commit`。sync_loop 负责 push + pull。

选择理由：
- commit 粒度细，每条消息一个 commit，git log 可追踪到每条消息
- 1s 同步间隔下延迟可忽略
- sync_loop 职责清晰：只管 push/pull，不管 commit

### 2.2 Sync 顺序：乐观 push 优先

```
有本地未推送的 commit？
  ├── 是 → try push（乐观快路径）
  │     ├── 成功 → emit messages_pushed → done
  │     └── 失败 → fetch + rebase + push（冲突路径）
  └── 否 → pull（只拉取远端更新）
```

push 优先原因：大多数时候远端没变化，push 直接成功，省掉一次 fetch 往返。

### 2.3 冲突策略：rebase + thread-aware 重编号

rebase 而非 merge：
- 线性历史，与 .thread 行号顺序一致
- 无 merge commit 噪音
- .thread 文件 append-only，冲突模式固定：双方在末尾追加

冲突解决流程（当 rebase 失败时）：
```
1. git rebase --abort
2. 提取本地未推送的消息（diff origin/main..HEAD 中 .thread 的新增行）
3. git reset --hard origin/main
4. 对每个有本地变更的 .thread 文件：
   - 读取远端版本（当前状态），获取 max line number
   - renumber_batch(本地消息, max_line)
   - 追加到文件
5. git add -A + commit
6. push（如果又失败，重试，最多 3 次）
```

### 2.4 P 引用链处理：树状结构

已实现于 `renumber.rs`。规则：
- P=0 → 保持
- P 指向本批次内消息 → 跟随重编号
- P 指向远端已有消息 → 保持原值

多个消息可以指向同一个 P，形成树结构。

### 2.5 Commit 消息格式（daemon 统一控制）

单条消息：
```
msg: @<author> -> <channel> L<line_number>
```

冲突解决后的合并 commit：
```
msg: sync <N> messages after rebase
```

### 2.6 默认同步间隔：30s → 1s

`DaemonConfig.sync_interval` 默认值从 30 改为 1。使用 `MissedTickBehavior::Delay` 防止网络慢时 tick 堆积——确保上一轮 sync 完成后至少间隔 1s 再启动下一轮。

---

## 3. 投递状态与事件扩展

### 3.1 两阶段投递确认

handle_send 返回时消息已 committed 但未 pushed：
```json
{"ok": true, "data": {"line_number": 5, "channel": "general", "status": "committed"}}
```

sync_loop push 成功后广播：
```json
{"event": "messages_pushed", "channel": "general", "line_numbers": [5, 6]}
```

如果发生 renumber：
```json
{"event": "message_renumbered", "channel": "general", "old_line": 5, "new_line": 12}
```

### 3.2 Event 模型扩展

| 事件 | 触发时机 | 字段 |
|------|---------|------|
| `thread_changed` | 本地写入或远端 pull 后 | channel, kind |
| `messages_pushed` | push 成功后 | channel, line_numbers |
| `message_renumbered` | rebase 重编号后 | channel, old_line, new_line |

### 3.3 Pending 追踪

AppState 新增 `pending_push: RwLock<Vec<PendingMessage>>`，记录已 commit 未 push 的消息。push 成功后清空并广播 `messages_pushed`。renumber 时更新行号并广播 `message_renumbered`。

---

## 4. 变更范围

### 4.1 修改文件

| 文件 | 变更 |
|------|------|
| `crates/gitim-daemon/src/handlers.rs` | handle_send 写入后立即 git add + commit；返回 status: "committed" |
| `crates/gitim-daemon/src/state.rs` | AppState 新增 pending_push 字段 |
| `crates/gitim-daemon/src/api.rs` | Event enum 扩展新事件类型；Response 添加 status 字段 |
| `crates/gitim-sync/src/sync_loop.rs` | 完整重写：push 优先 + pull + 冲突解决 |
| `crates/gitim-sync/src/git.rs` | 新增 fetch、has_unpushed_commits、diff_unpushed_thread_changes 等方法 |
| `crates/gitim-core/src/types/config.rs` | sync_interval 默认值 30 → 1 |

### 4.2 现有资产复用

- `renumber_batch()` — 直接复用，无需修改
- `push_with_retry()` — 需改造为 thread-aware 版本
- `format_message()` — 在 renumber 后重写消息时复用

---

## 5. 边界情况

| 条件 | 处理 |
|------|------|
| 无 remote 配置 | sync_loop 不启动（已有逻辑） |
| sync_interval=0 | sync_loop 不启动，消息只在本地（已有逻辑） |
| push 连续失败 3 次 | 放弃本轮，下个 cycle 再试；log error |
| rebase 冲突涉及非 .thread 文件 | 不应发生（daemon 只写 .thread 和 .meta.json）；如果发生，abort rebase 后 log error |
| 网络断开 | push/pull 失败，下个 cycle 重试 |
| daemon 在 push 中被 kill | 本地 commit 还在，下次启动后 sync_loop 会推送 |
| 续行（continuation lines） | renumber_batch 已正确处理 |
