# Phase 0: WASM 编译链 + Backend 抽象 — 实施计划

## 目标

为 daemon-web 方案打基础：让 gitim-core 和 gitim-sync 的纯函数可编译到 WASM，
同时在 webui-v2 抽取 Backend interface 为后续 LocalBackend 做准备。

## 步骤

### Step 1: gitim-core 清理 chrono 依赖

chrono 在 gitim-core 源码中未使用（时间戳是纯字符串），移除可避免 WASM 编译时的
default-features 问题。

- 文件：`crates/gitim-core/Cargo.toml`
- 改动：删除 `chrono.workspace = true`
- 验证：`cargo test -p gitim-core` 全绿

### Step 2: gitim-sync cfg 门控 + target-specific deps

I/O 模块（git.rs, watcher.rs, sync_loop.rs）仅在非 WASM 平台编译。
WASM 不兼容的依赖（tokio, notify, rand）改为 target-specific。

- 文件：`crates/gitim-sync/Cargo.toml`
  - tokio, notify, rand 移到 `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`
- 文件：`crates/gitim-sync/src/lib.rs`
  - git, watcher, sync_loop 加 `#[cfg(not(target_arch = "wasm32"))]`
- 验证：`cargo test -p gitim-sync` 全绿（native 不受影响）

### Step 3: gitim-sync 提取 resolve_content 纯函数

当前 resolve_content 混合了文件 I/O 和内容变换。拆为：
- `resolve_content_pure(local_additions, remote_contents)` — 纯函数，接收已读取的内容
- `resolve_content(local_additions, repo_root)` — I/O wrapper，读文件后调纯函数

- 文件：`crates/gitim-sync/src/conflict.rs`
- 验证：现有测试全绿 + 新增纯函数单测

### Step 4: 新建 gitim-wasm crate

wasm-bindgen wrapper，导出 gitim-core + gitim-sync 的纯函数到 JS。

- 文件：`crates/gitim-wasm/Cargo.toml`
- 文件：`crates/gitim-wasm/src/lib.rs`
- 导出函数：
  - parse_thread, format_message, format_event
  - validate_append, validate_join, validate_leave
  - validate_channel_meta, validate_user_meta
  - extract_mentions, extract_links, dm_filename
  - renumber_batch, merge_channel_meta, build_rebase_commit_msg
  - resolve_content_pure
- 验证：`wasm-pack build --target web` 成功

### Step 5: webui-v2 Backend interface 抽取

从 client.ts 提取 Backend interface，现有逻辑搬入 HttpBackend。
这是纯重构——外部行为不变。

- 新增：`webui-v2/src/lib/backend.ts` — Backend interface + HttpBackend 实现
- 修改：`webui-v2/src/lib/client.ts` — 改为通过 Backend 分发
- 验证：`npm run build` 通过，`npm run lint` 通过

### Step 6: 全量验证

- `cargo test` 全绿（~270 tests）
- `wasm-pack build --target web` 在 gitim-wasm 中成功
- webui-v2 `npm run build` 通过
