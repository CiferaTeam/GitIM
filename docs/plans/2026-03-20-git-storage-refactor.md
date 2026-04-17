# GitStorage 职责分离重构

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 git 操作收敛到 GitStorage struct，IM 业务逻辑（冲突解决、消息处理）与 git 状态管理完全分离。

**Architecture:** GitStorage 拥有所有 git 原子操作（commit/push/pull/fetch/discard_unpushed）。SyncLoop 编排 push/pull 周期并管理冲突流程的 git 状态。conflict.rs 只做纯内容变换（renumber），不碰 git。handlers.rs 通过 AppState 中的共享 GitStorage 实例做持久化。

**Tech Stack:** Rust, tokio, gitim-core, gitim-sync

**Worktree:** `.worktrees/git-storage-refactor` (branch: `feature/git-storage-refactor`)

---

## 文件结构

**Modified:**
- `crates/gitim-sync/src/git.rs` — GitRepo → GitStorage 重命名，方法增删
- `crates/gitim-sync/src/conflict.rs` — 移除 git 操作，变为纯内容变换函数
- `crates/gitim-sync/src/sync_loop.rs` — 冲突流程重写，由 SyncLoop 管理 git 状态
- `crates/gitim-sync/src/lib.rs` — 更新 re-exports
- `crates/gitim-daemon/src/state.rs` — AppState 加入 GitStorage 实例
- `crates/gitim-daemon/src/handlers.rs` — 使用 state.git_storage 替代每次 GitRepo::new
- `crates/gitim-daemon/src/main.rs` — 创建 GitStorage 并传入 AppState
- `crates/gitim-sync/tests/git_ops_test.rs` — 更新方法名
- `crates/gitim-sync/tests/conflict_test.rs` — 重写为纯内容变换测试
- `crates/gitim-sync/tests/sync_e2e_test.rs` — 更新冲突流程
- `crates/gitim-daemon/tests/commit_test.rs` — 适配 shared GitStorage
- `crates/gitim-daemon/tests/push_test.rs` — 适配 shared GitStorage

---

## Chunk 1: GitStorage 底层重构

### Task 1: GitStorage 重命名 + 方法调整

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`

- [ ] **Step 1: 重命名 GitRepo → GitStorage**
  - struct 名、impl 块、所有关联方法签名中的 `GitRepo` 替换为 `GitStorage`
  - 验收：`cargo build` 通过（此时调用方会编译失败，但 crate 内部自洽）

- [ ] **Step 2: GitError 新增 Conflict variant**
  - 在 `GitError` enum 中增加 `PushConflict` variant，用于区分 push 因远端领先而失败 vs 其他错误
  - 验收：GitError 有 `PushConflict` variant

- [ ] **Step 3: diff_unpushed_thread_additions → diff_unpushed(pattern)**
  - 方法重命名为 `diff_unpushed`
  - 增加 `pattern: &str` 参数，替换硬编码的 `"*.thread"`
  - git diff 命令中使用传入的 pattern
  - 返回类型保持 `HashMap<PathBuf, String>`
  - 验收：方法签名为 `pub fn diff_unpushed(&self, pattern: &str) -> Result<HashMap<PathBuf, String>, GitError>`

- [ ] **Step 4: 新增 discard_unpushed()**
  - 封装 `rebase_abort()` + `reset_hard_origin()` 为一个公开方法 `discard_unpushed()`
  - 语义：丢弃所有本地未推送变更，回到远端状态
  - rebase_abort 继续保持 best-effort（ignore errors）
  - 验收：调用后本地 HEAD 与 origin/main 一致

- [ ] **Step 5: 删除 push_with_retry**
  - 移除 `push_with_retry` 方法，重试逻辑归 SyncLoop
  - 验收：GitStorage 不再有 retry 相关方法

- [ ] **Step 6: push() 区分 Conflict 错误**
  - 在 `push()` 方法中，解析 stderr 判断是否为远端领先导致的 push 失败
  - 如果 stderr 包含 `"rejected"` 或 `"non-fast-forward"`，返回 `Err(GitError::PushConflict)`
  - 其他错误继续返回 `Err(GitError::CommandFailed(...))`
  - 验收：push 到领先的远端时返回 PushConflict，其他失败返回 CommandFailed

- [ ] **Step 7: Commit**

Run: `cargo test -p gitim-sync` (部分测试可能因重命名而失败，预期行为)

---

### Task 2: 更新 gitim-sync 内部调用方

**Files:**
- Modify: `crates/gitim-sync/src/lib.rs`
- Modify: `crates/gitim-sync/src/sync_loop.rs` (仅重命名，不重写逻辑)
- Modify: `crates/gitim-sync/src/conflict.rs` (仅重命名，不拆分逻辑)

- [ ] **Step 1: 更新 lib.rs re-exports**
  - 确认 `pub mod git` 导出正确
  - 验收：外部 crate 能引用 `gitim_sync::git::GitStorage`

- [ ] **Step 2: sync_loop.rs 中 GitRepo → GitStorage**
  - 所有 `GitRepo` 引用替换为 `GitStorage`
  - `diff_unpushed_thread_additions()` 替换为 `diff_unpushed("*.thread")`
  - 验收：`cargo build -p gitim-sync` 通过

- [ ] **Step 3: conflict.rs 中 GitRepo → GitStorage**
  - `resolve_thread_conflicts` 参数类型 `&GitRepo` → `&GitStorage`
  - 验收：`cargo build -p gitim-sync` 通过

- [ ] **Step 4: 更新 git_ops_test.rs**
  - 所有 `GitRepo::new` → `GitStorage::new`
  - 如有引用 `diff_unpushed_thread_additions` 则替换为 `diff_unpushed("*.thread")`
  - 新增 `discard_unpushed` 的测试：创建 repo → commit → 调用 discard_unpushed → 验证回到 origin 状态
  - 验收：`cargo test -p gitim-sync --test git_ops_test` 全部通过

- [ ] **Step 5: Commit**

Run: `cargo test -p gitim-sync`
验收：所有 gitim-sync 测试通过

---

## Chunk 2: 冲突解决职责分离

### Task 3: conflict.rs 纯化 — 移除 git 操作

**Files:**
- Modify: `crates/gitim-sync/src/conflict.rs`

- [ ] **Step 1: 提取纯内容变换函数**
  - 将 `resolve_thread_conflicts` 拆分：
    - 新函数 `resolve_content(local_additions: &HashMap<PathBuf, String>, repo_root: &Path) -> Result<Vec<ResolvedFile>>`
    - `ResolvedFile` struct: `{ path: PathBuf, content: String, commit_msg: String }`
  - 新函数只做：读当前文件 → 找 max_line → renumber → 生成 commit_msg → 返回 ResolvedFile
  - 不调用任何 GitStorage/GitRepo 方法（不 abort rebase、不 reset、不 commit）
  - 验收：`resolve_content` 只做文件读取和内容变换，grep 不到任何 `repo.` 调用

- [ ] **Step 2: 保留旧函数作为兼容桥接（临时）**
  - `resolve_thread_conflicts` 暂时保留，内部调用 `repo.discard_unpushed()` + `resolve_content()` + 写文件 + `repo.add_and_commit()`
  - 这样 sync_loop.rs 在 Task 4 之前仍能编译
  - 验收：`cargo test -p gitim-sync` 全部通过

- [ ] **Step 3: 更新 conflict_test.rs**
  - 为 `resolve_content` 编写纯单元测试：
    - 用 tempdir 创建模拟的远端文件状态
    - 传入 local_additions HashMap
    - 验证返回的 ResolvedFile 内容正确（行号重编、P ref 修正）
  - 不需要创建真实 git repo
  - 验收：`cargo test -p gitim-sync --test conflict_test` 全部通过

- [ ] **Step 4: Commit**

---

### Task 4: sync_loop.rs 冲突流程重写

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`
- Modify: `crates/gitim-sync/src/conflict.rs` (删除旧桥接函数)

- [ ] **Step 1: 重写 sync_with_push 的冲突分支**
  - 当 push 返回 `GitError::PushConflict` 时：
    1. `storage.fetch()`
    2. `storage.diff_unpushed("*.thread")` 捕获本地变更
    3. `storage.discard_unpushed()` 回到远端状态
    4. `conflict::resolve_content(&local_additions, storage.root())` 做纯内容变换
    5. 将 ResolvedFile 的内容写入文件系统
    6. `storage.commit(paths, msg)` 提交解决后的内容
    7. `storage.push()` 再次推送
  - 当 push 返回其他错误时：log + return（不重试）
  - 移除对 `repo.rebase_onto_origin()` 的直接调用（rebase 逻辑内化到 GitStorage 或由 discard_unpushed 替代）
  - 验收：sync_with_push 中不直接调用 rebase_onto_origin / rebase_abort / reset_hard_origin

- [ ] **Step 2: 简化快路径**
  - push 成功：直接 on_pushed()
  - push PushConflict：走冲突流程
  - 保持 MAX_SYNC_RETRIES 重试逻辑
  - 验收：代码路径清晰，只有 push/conflict-resolve/retry 三条

- [ ] **Step 3: 删除 conflict.rs 中的旧 resolve_thread_conflicts**
  - 删除 Task 3 Step 2 保留的桥接函数
  - 只保留 `resolve_content`、`ResolvedFile`、`build_rebase_commit_msg`
  - 更新 conflict.rs 的 use 声明，移除对 `GitError` 和 `git::GitStorage` 的依赖
  - 验收：conflict.rs 中 grep 不到 `git::` 或 `GitStorage`

- [ ] **Step 4: 处理 rebase 成功的快路径**
  - 当前逻辑：push 失败 → fetch → rebase_onto_origin → 如果 rebase 成功则直接 push
  - 新逻辑选择：
    - 方案 A：保留 rebase 快路径（rebase 成功意味着没有 .thread 冲突，直接 push）
    - 方案 B：统一走 discard + resolve_content（更简单，但多一次文件读写）
  - 建议采用方案 A：rebase 成功时直接 push，失败时走 discard + resolve_content
  - 验收：两条路径都有对应的测试覆盖

- [ ] **Step 5: 更新 sync_e2e_test.rs**
  - 调整冲突解决的 e2e 测试以匹配新流程
  - 验证：push 冲突 → resolve → 再次 push 成功
  - 验收：`cargo test -p gitim-sync --test sync_e2e_test` 全部通过

- [ ] **Step 6: Commit**

Run: `cargo test -p gitim-sync`
验收：所有 gitim-sync 测试通过

---

## Chunk 3: Daemon 侧接入

### Task 5: AppState 持有共享 GitStorage

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs`
- Modify: `crates/gitim-daemon/src/main.rs`

- [ ] **Step 1: AppState 加入 git_storage 字段**
  - 类型为 `gitim_sync::git::GitStorage`
  - AppState::new 接收 GitStorage 实例
  - 验收：AppState 有 `pub git_storage: GitStorage` 字段

- [ ] **Step 2: main.rs 创建 GitStorage 并传入**
  - 在 `main()` 中创建 `GitStorage::new(&repo_root)`
  - 传入 `AppState::new(...)` 构造函数
  - sync_loop 也使用同一 repo_root（sync_loop 内部创建自己的 GitStorage 实例，因为它运行在独立 task 中）
  - 验收：`cargo build -p gitim-daemon` 通过

- [ ] **Step 3: Commit**

---

### Task 6: handlers.rs 使用共享 GitStorage

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

- [ ] **Step 1: handle_send 使用 state.git_storage**
  - 移除 `handlers.rs:150` 的 `GitRepo::new(&state.repo_root)`
  - 替换为 `state.git_storage.add_and_commit(...)`
  - 移除 `use gitim_sync::git::GitRepo`，改为 `use gitim_sync::git::GitStorage`（或通过 state 直接访问）
  - 验收：handle_send 中 grep 不到 `GitRepo`

- [ ] **Step 2: handle_register_user 使用 state.git_storage**
  - 移除 `handlers.rs:288` 的 `GitRepo::new(&state.repo_root)`
  - 替换为 `state.git_storage.add_and_commit(...)`
  - 验收：handle_register_user 中 grep 不到 `GitRepo`

- [ ] **Step 3: 清理 imports**
  - 移除对 `gitim_sync::git::GitRepo` 的 use 声明
  - 验收：整个 handlers.rs 中 grep 不到 `GitRepo`

- [ ] **Step 4: 更新 daemon 测试**
  - `commit_test.rs` 和 `push_test.rs` 中的 AppState 构造需要传入 GitStorage
  - 验收：`cargo test -p gitim-daemon` 全部通过

- [ ] **Step 5: Commit**

Run: `cargo test`
验收：全项目所有测试通过

---

## Chunk 4: 全局验证 + 清理

### Task 7: 最终验证

**Files:**
- 无新增修改，仅验证

- [ ] **Step 1: 全项目编译**
  - Run: `cargo build`
  - 验收：0 errors, 0 warnings (项目启用了 `#![deny(warnings)]`)

- [ ] **Step 2: 全项目测试**
  - Run: `cargo test`
  - 验收：所有 109+ 测试通过

- [ ] **Step 3: 验证职责分离**
  - grep 确认：
    - `conflict.rs` 中没有 `GitStorage` 或 `git::` 引用
    - `handlers.rs` 中没有 `GitRepo` 引用
    - `git.rs` 中没有 `thread`、`parse_thread`、`renumber` 等 IM 术语
  - 验收：职责边界清晰

- [ ] **Step 4: Commit 最终清理（如有）**

- [ ] **Step 5: 全量验证通过后提交 TODOS.md**
  - 确认 TODOS.md 已在分支上

---

## 依赖关系

```
Task 1 (GitStorage 重命名)
  └→ Task 2 (更新调用方)
      └→ Task 3 (conflict 纯化)
          └→ Task 4 (sync_loop 重写)  ← 关键路径
              └→ Task 5 (AppState 改造)
                  └→ Task 6 (handlers 适配)
                      └→ Task 7 (最终验证)
```

严格串行：每个 Task 依赖上一个 Task 的完成。不可并行。

## NOT in scope

- onboard.ts 的 git 操作收敛（记录在 TODOS.md）
- .gitignore 管理移入 init 流程（记录在 TODOS.md）
- CLI 侧 TypeScript 代码变更
- VersionedStorage trait / ContentHandler trait（评审中已决定不做 trait 抽象）
