# Sync Commit & Push Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 daemon sync loop 的 commit/push 缺失，实现 handle_send 立即 commit + sync_loop 乐观 push + thread-aware 冲突解决的完整投递闭环。

**Architecture:** handle_send 写入文件后立即 git add + commit，sync_loop 每 1s 执行一次乐观 push（push 优先，失败则 fetch + rebase），rebase 冲突时使用已有的 renumber_batch 重编号本地消息后重试。通过 pending_push 追踪和新事件类型实现两阶段投递确认。

**Tech Stack:** Rust, tokio, gitim-sync (git ops), gitim-core (parser/formatter/renumber)

**Spec:** `docs/superpowers/specs/2026-03-17-gitim-sync-commit-push-design.md`

---

## File Structure

| 文件 | 职责 | 操作 |
|------|------|------|
| `crates/gitim-core/src/types/config.rs` | sync_interval 默认值 30→1 | Modify |
| `crates/gitim-sync/src/git.rs` | 新增 fetch、has_unpushed_commits、diff_unpushed_thread_additions、rebase_abort、reset_hard_origin 方法 | Modify |
| `crates/gitim-sync/src/conflict.rs` | thread-aware 冲突解决：提取本地消息、reset、renumber、重新追加 | Create |
| `crates/gitim-sync/src/sync_loop.rs` | 完整重写：乐观 push + pull + 冲突解决，接收 SharedState 以广播事件 | Modify |
| `crates/gitim-sync/src/lib.rs` | 导出 conflict 模块 | Modify |
| `crates/gitim-daemon/src/api.rs` | Event 从纯 struct 改为 enum，增加 MessagesPushed 和 MessageRenumbered 变体 | Modify |
| `crates/gitim-daemon/src/state.rs` | AppState 新增 pending_push: RwLock<Vec<PendingMessage>> | Modify |
| `crates/gitim-daemon/src/handlers.rs` | handle_send 写入后 git add + commit，记入 pending_push，返回 status: "committed" | Modify |
| `crates/gitim-daemon/src/main.rs` | sync_loop 启动时传入 SharedState（而非仅 repo_root） | Modify |
| `crates/gitim-sync/tests/git_ops_test.rs` | git.rs 新方法的单元测试 | Create |
| `crates/gitim-sync/tests/conflict_test.rs` | 冲突解决逻辑的单元测试 | Create |
| `crates/gitim-daemon/tests/commit_test.rs` | handle_send 后验证 git commit 产生 | Create |

---

## Chunk 1: 基础设施层

### Task 1: sync_interval 默认值 30→1

**Files:**
- Modify: `crates/gitim-core/src/types/config.rs`

- [ ] **Step 1:** 修改 `default_sync_interval()` 返回值从 30 改为 1，同时修改 `Default for DaemonConfig` 中的 sync_interval 为 1
- [ ] **Step 2:** 运行 `cargo test -p gitim-core`，确认 config 相关测试通过（现有测试 `test_config_endpoint_defaults` 可能需要更新断言）
- [ ] **Step 3:** Commit `fix: change default sync_interval from 30s to 1s`

---

### Task 2: git.rs 扩展——新增底层 git 操作

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Create: `crates/gitim-sync/tests/git_ops_test.rs`

需要给 GitRepo 新增以下方法（每个方法封装一个 git 命令）：

- [ ] **Step 1:** 写测试 `test_fetch_succeeds_with_remote` — 在 tempdir 中 init 一个 bare repo 作为 remote，clone 后调用 fetch，验证无报错
- [ ] **Step 2:** 运行测试，确认失败（方法不存在）
- [ ] **Step 3:** 实现 `fetch()` 方法 — 执行 `git fetch origin`
- [ ] **Step 4:** 运行测试，确认通过
- [ ] **Step 5:** 写测试 `test_has_unpushed_commits` — clone repo 后本地 commit 一个文件，验证返回 true；push 后验证返回 false
- [ ] **Step 6:** 运行测试，确认失败
- [ ] **Step 7:** 实现 `has_unpushed_commits()` 方法 — 执行 `git rev-list --count origin/main..HEAD`，返回 count > 0
- [ ] **Step 8:** 运行测试，确认通过
- [ ] **Step 9:** 写测试 `test_diff_unpushed_thread_additions` — 本地追加内容到一个 .thread 文件并 commit，验证方法返回 `HashMap<文件相对路径, 新增内容>`
- [ ] **Step 10:** 运行测试，确认失败
- [ ] **Step 11:** 实现 `diff_unpushed_thread_additions()` — 对每个 `git diff origin/main..HEAD -- '*.thread'` 中的文件，提取新增行（`+` 开头、非 `+++` 的行），返回 `HashMap<PathBuf, String>`
- [ ] **Step 12:** 运行测试，确认通过
- [ ] **Step 13:** 实现 `rebase_abort()` — 执行 `git rebase --abort`；实现 `reset_hard_origin()` — 执行 `git reset --hard origin/main`。这两个方法逻辑简单，直接实现即可
- [ ] **Step 14:** 运行 `cargo test -p gitim-sync`，确认所有测试通过
- [ ] **Step 15:** Commit `feat(sync): add git fetch, has_unpushed, diff_thread, rebase_abort, reset_hard methods`

---

### Task 3: Event 模型扩展

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/tests/push_test.rs`（现有事件测试需适配新 Event 结构）

- [ ] **Step 1:** 写测试 — `messages_pushed` 和 `message_renumbered` 事件能序列化为预期的 JSON 格式
- [ ] **Step 2:** 运行测试，确认失败
- [ ] **Step 3:** 将 Event 从单一 struct 改为 tagged enum（`#[serde(tag = "event")]`），包含三个变体：ThreadChanged { channel, kind }、MessagesPushed { channel, line_numbers: Vec<u64> }、MessageRenumbered { channel, old_line: u64, new_line: u64 }
- [ ] **Step 4:** 更新所有现有代码中构造 Event 的位置（handlers.rs 的 thread_changed 事件、watcher 事件、push_test.rs 中的断言）以适配新 enum
- [ ] **Step 5:** 运行 `cargo test -p gitim-daemon`，确认所有测试通过（包括现有的 push_test）
- [ ] **Step 6:** Commit `feat(api): extend Event enum with messages_pushed and message_renumbered`

---

### Task 4: PendingPush 追踪

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs`

- [ ] **Step 1:** 在 state.rs 中定义 `PendingMessage { channel: String, line_number: u64 }` struct，AppState 新增 `pending_push: RwLock<Vec<PendingMessage>>` 字段
- [ ] **Step 2:** 更新 `AppState::new()` 初始化 pending_push 为空 Vec
- [ ] **Step 3:** 更新 main.rs 和 push_test.rs 中所有 `AppState::new()` 调用以适配新签名（如果签名不变则无需改）
- [ ] **Step 4:** 运行 `cargo test`，确认全部通过
- [ ] **Step 5:** Commit `feat(state): add pending_push tracking to AppState`

---

## Chunk 2: 核心逻辑层

### Task 5: handle_send 立即 commit

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`
- Create: `crates/gitim-daemon/tests/commit_test.rs`

- [ ] **Step 1:** 写测试 `test_handle_send_creates_git_commit` — 在 tempdir 中创建一个 git 仓库（git init + initial commit），设置 AppState，调用 handle_send，然后用 `git log --oneline` 验证新增了一个 commit，且 commit message 匹配 `msg: @alice -> general L000001`
- [ ] **Step 2:** 运行测试，确认失败（当前 handle_send 不 commit）
- [ ] **Step 3:** 在 handle_send 中，写入文件成功后，调用 GitRepo::add_and_commit，路径为 thread 文件的相对路径，commit message 格式为 `msg: @{author} -> {channel} L{line:06}`。如果 commit 失败（如无 git repo），log warn 但不影响 send 的返回——消息已写入文件，commit 失败是可恢复的
- [ ] **Step 4:** 在返回的 JSON 中增加 `"status": "committed"` 字段
- [ ] **Step 5:** 将消息记入 `state.pending_push`
- [ ] **Step 6:** 运行测试，确认通过
- [ ] **Step 7:** 运行 `cargo test`，确认全部通过
- [ ] **Step 8:** Commit `feat(handlers): handle_send immediately commits after write`

---

### Task 6: Thread-aware 冲突解决

**Files:**
- Create: `crates/gitim-sync/src/conflict.rs`
- Modify: `crates/gitim-sync/src/lib.rs`
- Create: `crates/gitim-sync/tests/conflict_test.rs`

这是最核心的新模块。职责：当 rebase 冲突时，提取本地消息 → reset 到远端 → renumber → 重新追加 → commit。

- [ ] **Step 1:** 写测试 `test_resolve_conflict_renumbers_local_messages` — 构造场景：tempdir 中有一个 bare repo 作为 remote，clone 两份。在 clone-A 中追加 2 条消息到 general.thread 并 commit+push。在 clone-B 中也追加 2 条消息并 commit（未 push）。在 clone-B 上调用 conflict resolver，验证：(a) general.thread 包含 4 条消息，(b) 前 2 条行号为 L1/L2（来自 A），后 2 条行号为 L3/L4（来自 B 重编号后），(c) 可以成功 push
- [ ] **Step 2:** 运行测试，确认失败
- [ ] **Step 3:** 实现 `resolve_thread_conflicts(repo: &GitRepo) -> Result<Vec<(String, u64, u64)>, ...>` 函数，返回 `Vec<(channel, old_line, new_line)>` 重编号映射。流程：
  1. 调用 `repo.diff_unpushed_thread_additions()` 获取本地 .thread 新增内容
  2. 调用 `repo.rebase_abort()`（如果有进行中的 rebase）
  3. 调用 `repo.reset_hard_origin()`
  4. 对每个有变更的 .thread 文件：读取当前内容 → parse 获取 max line → `renumber_batch(local_additions, max_line)` → 追加到文件
  5. 调用 `repo.add_and_commit(paths, "msg: sync N messages after rebase")`
  6. 返回重编号映射
- [ ] **Step 4:** 运行测试，确认通过
- [ ] **Step 5:** 写测试 `test_resolve_conflict_updates_p_references` — 本地消息 L2 引用本地 L1，冲突后两者重编号，验证 P 引用也更新了
- [ ] **Step 6:** 运行测试，确认通过（renumber_batch 已有此逻辑）
- [ ] **Step 7:** 写测试 `test_resolve_conflict_preserves_external_p_references` — 本地消息引用远端已有的消息，冲突后 P 值保持不变
- [ ] **Step 8:** 运行测试，确认通过
- [ ] **Step 9:** 在 `lib.rs` 中导出 `pub mod conflict;`
- [ ] **Step 10:** 运行 `cargo test -p gitim-sync`，确认全部通过
- [ ] **Step 11:** Commit `feat(sync): add thread-aware conflict resolution`

---

### Task 7: Sync loop 重写

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`
- Modify: `crates/gitim-daemon/src/main.rs`

- [ ] **Step 1:** 修改 `start_sync_loop` 函数签名，除了 `repo_root` 和 `interval_secs`，还需接收 `event_tx: broadcast::Sender<Event>` 和 `pending_push: Arc<RwLock<Vec<PendingMessage>>>` 以便广播推送事件和清理 pending 队列。需要在 gitim-sync 的 Cargo.toml 中添加对 gitim-daemon api 类型的依赖——或者更好的做法：将 Event 和 PendingMessage 的定义移到 gitim-core 中以避免循环依赖，或者让 sync_loop 接收闭包回调（`on_pushed`、`on_renumbered`）而非直接依赖 daemon 类型。**推荐回调方案**，保持 crate 间依赖方向干净
- [ ] **Step 2:** 实现 sync loop 主体逻辑，使用 `MissedTickBehavior::Delay`：
  - 如果 `has_unpushed_commits` → try push → 成功则调用 `on_pushed` 回调 → 失败则 fetch → rebase → 如果 rebase 也失败 → 调用 `conflict::resolve_thread_conflicts` → push → 如果 renumber 了则调用 `on_renumbered` 回调
  - 如果没有未推送的 commit → pull_rebase（拉取远端更新）
  - 最多重试 3 次，仍失败则 log error 放弃本轮
- [ ] **Step 3:** 更新 main.rs 中 sync_loop 的启动代码，传入回调闭包：`on_pushed` 广播 `Event::MessagesPushed` 并清理 pending_push；`on_renumbered` 广播 `Event::MessageRenumbered` 并更新 pending_push 中的行号
- [ ] **Step 4:** 运行 `cargo build`，确认编译通过
- [ ] **Step 5:** 运行 `cargo test`，确认全部通过
- [ ] **Step 6:** Commit `feat(sync): rewrite sync_loop with push-first strategy and conflict resolution`

---

## Chunk 3: 集成验证

### Task 8: 端到端集成测试

**Files:**
- Create: `crates/gitim-sync/tests/sync_e2e_test.rs`

- [ ] **Step 1:** 写测试 `test_sync_loop_pushes_committed_messages` — 创建 bare remote + clone，commit 一条消息到 .thread，手动运行一轮 sync 逻辑（不启动定时器），验证 remote 的 .thread 包含该消息
- [ ] **Step 2:** 运行测试，确认通过
- [ ] **Step 3:** 写测试 `test_sync_loop_resolves_concurrent_writes` — 创建 bare remote + 两个 clone。Clone-A commit+push 2 条消息。Clone-B commit 2 条消息（未 push）。在 Clone-B 上运行一轮 sync，验证：(a) push 成功，(b) Clone-B 的 .thread 包含 4 条消息且行号连续，(c) Clone-B 的后 2 条消息被正确 renumber
- [ ] **Step 4:** 运行测试，确认通过
- [ ] **Step 5:** 写测试 `test_sync_loop_pulls_when_nothing_to_push` — Clone-A push 新内容后，在 Clone-B（无本地变更）上运行一轮 sync，验证 Clone-B 拉取到了新内容
- [ ] **Step 6:** 运行测试，确认通过
- [ ] **Step 7:** 运行 `cargo test` 全量，确认 114+ 测试全部通过
- [ ] **Step 8:** Commit `test(sync): add e2e tests for sync loop push and conflict resolution`

---

### Task 9: 清理 handle_register_user 中的遗留 git commit

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1:** 移除 `handle_register_user` 中手动调用 `std::process::Command::new("git")` 进行 add+commit 的代码（handlers.rs:259-266），改用与 handle_send 一致的方式通过 GitRepo 的 add_and_commit 方法，commit message 格式为 `msg: register @{handler}`
- [ ] **Step 2:** 运行 `cargo test`，确认通过
- [ ] **Step 3:** Commit `refactor(handlers): unify git commit in register_user to use GitRepo`

---

**Plan complete and saved to `docs/superpowers/plans/2026-03-17-sync-commit-push.md`. Ready to execute?**
