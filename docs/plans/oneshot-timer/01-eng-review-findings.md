# Eng Review Findings — Oneshot Timer

**Reviewer**: Claude (eng-manager mode, SOP Phase 3)
**Input**: [00-requirements.md](00-requirements.md)
**Date**: 2026-05-20
**Verdict**: PASS with 6 findings to enforce during implementation. No architectural rewrite needed.

## Findings & Required Plan Constraints

### F1 [P1] Lock anchor 必须独立于 atomic-rename 目标

**Problem**: design 说"fs2 flock 串行化 read-modify-write"+"atomic write 走 tmp + rename"。两者天然冲突——flock 锁的是 inode，rename 会让 old file 被 unlinked、新 file 是无锁的。两个 writer 可以同时持有"曾经的"锁但写到新 file 上 → lost update。

**Plan 必做**:
- 用单独的 `.gitim/timers.json.lock` 空文件做 lock anchor
- CLI 和 agent_loop 写 / 读时都 `open(.lock).lock_exclusive()` → 操作 `timers.json`（包括 atomic rename）→ release lock
- lockfile 持久（不要每次 create/remove，否则 lock 跨 reincarnation 不一致）；首次 use 时 create-if-missing
- `gitim-core` 提供 `with_timers_lock<T>(clone_path, f: impl FnOnce(&Path) -> Result<T>) -> Result<T>` helper，CLI 和 agent_loop 都用

**Reference pattern**: gix / libgit2 用 `.git/index.lock`；同思路。

### F2 [P2] agent_loop API 决定推迟到 plan 阶段

**Problem**: design pseudo-code 写了 `run_once_with_prefix(prefix, changes)`，但当前 `run_once` 真实 signature 未知。

**Plan 必做**:
- 读 [agent_loop.rs](crates/gitim-runtime/src/agent_loop.rs) 现有 `run_once` 和 `format_changes_as_prompt` 确定最佳集成点
- **倾向方案**: 把 fired timer 包装成 synthetic `ChannelChange` entry（或类似 enum 变体），注入到 daemon-poll-changes pipeline 上游；`format_changes_as_prompt` 看到这种 entry 时输出 `## ⏰ Timer reminder(s) fired` 段。好处：`run_once` API 零侵入；timer 跟 daemon changes 走同一处理路径，未来加 agent routing / per-message metadata 自然适配
- 若该方案不可行（synthetic entry 跟现有 ChannelChange 类型语义冲突），fallback 加 `Option<String>` prefix 到 `run_once`

### F3 [P3] N agent 同时唤醒抖动 — 记录为 known limitation

**Problem**: 多 agent 同时刻 timer → 同时 wake → daemon 同时 poll → 短峰 IO/CPU 抖动。

**Plan 不修**:
- 概率极低（cap=3 + agent 独立 schedule + 用户人工设的时间天然不同步）
- design Non-goals 段加一行说明，不引入抖动缓解（如随机 jitter sleep）

### F4 [P2] pop_fired_timers / peek_next_due panic safety

**Problem**: agent_loop 是 supervisor 关键 tokio task；timer 模块 panic 会让该 agent 失效。

**Plan 必做**:
- `pop_fired_timers(&Path) -> Result<Vec<Timer>, TimerError>` —— 所有 IO + parse 都 Result-based
- 调用方（agent_loop）`unwrap_or_default()` + `tracing::warn!`，**禁止** `.unwrap()` / `.expect()` / `panic!` / `todo!()` 在 timer 模块 prod 路径
- 复用 CLAUDE.md 已有的 `[workspace.lints]` 设置（`unwrap_used = warn` / `panic = warn`）
- 加 `#![deny(clippy::unwrap_used)]` 到 timer 模块顶部，比 workspace warn 更严

### F5 [P2] 原子写的 tmp file 清理

**Problem**: atomic write = write tmp → fsync → rename。rename 失败 → tmp 永远不清理。

**Plan 必做**:
- 用 [`tempfile::NamedTempFile::persist`](https://docs.rs/tempfile) —— `NamedTempFile` 在 Drop 时自动清理，`persist()` 成功才"过户"为目标 path；rename 失败 / panic / early return 都自动 cleanup
- 或自己 RAII guard（`Drop` 里 `fs::remove_file`），但 `tempfile` 更 boring（[Layer 1]）
- timer 模块 IO 层统一走这个 pattern

### F6 [P2] Clock skew test 用 partition_fired pure function 覆盖

**Problem**: design 描述了时间回拨 / 前进的行为，test plan 没列对应 case。

**Plan 必做**:
- `partition_fired(timers: &[Timer], now: DateTime<Utc>) -> (Vec<Timer> fired, Vec<Timer> pending)` 必须是 **pure function**，`now` 注入（不内部 call `Utc::now()`）
- Unit test 覆盖：
  - `now == 任一 timer 的 fire_at` 边界（=== 也算 fired）
  - `now < 全部 fire_at`（全 pending）
  - `now > 全部 fire_at`（全 fired，模拟跨重启补漏 / 时间前进）
  - 单 timer，pending → 时间前进到 fire_at 后再算 → fired
- 上层 `pop_fired_timers` 实现 = `partition_fired(read_file()?, Utc::now())` + write back + atomic guard，集成 test 覆盖

## Plan-Phase 输入约束总结

writing-plans 阶段生成 TDD 任务时，每个相关任务必须满足：

- **gitim-core::timer 模块**: F1 lockfile helper + F4 panic-free + F5 tempfile-based atomic write + F6 partition_fired pure function with comprehensive unit tests
- **gitim-cli::timer 子命令**: 走 F1 helper；error paths 100% test 覆盖
- **gitim-runtime::agent_loop integration**: F2 决定 API shape；走 F1 helper；调用 Result-based timer API，unwrap_or_default + log，零 panic
- **Tests**: F6 clock skew via partition_fired pure function；flock concurrency 仍按 design Section 7 列的并发 race test 跑

## Decisions to defer to plan-phase（不阻塞）

- timer 模块单 `timer.rs` 还是子模块 `timer/{types,parse,io,partition}.rs`
- `Clock` trait 是否引入（partition_fired 已 pure 不需要，但 IO 层 `now` 时刻可能想 mock）—— v1 不引入，直接 `Utc::now()`
- timer id 随机后缀字符集（hex / base32 / 自定义）—— 跟现有 flow run_id 实现保持一致
- gitim CLI 现有 atomic_write_json helper 是否复用还是 timer 模块新写

## Non-issues 已确认

- ✓ 0 innovation token spent（humantime + fs2 + tempfile 都是 boring [Layer 1]）
- ✓ 9 个文件改动（未破 8-file 阈值）
- ✓ 无新 service / 无新 binary / 无新 endpoint
- ✓ Distribution / CI 不需要改动
- ✓ Security: 无新 attack surface（pure local fs in agent's own clone）
- ✓ DX: agent 通过现有 gitim CLI 入口，无新工具链
