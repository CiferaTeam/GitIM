# GitIM v1 Implementation Plan

**状态：已完成**

**Goal:** Implement the GitIM v1 protocol — a text-file + Git based IM system with Rust daemon and TypeScript CLI.

**Architecture:** Rust cargo workspace with three crates (`gitim-core`, `gitim-daemon`, `gitim-sync`) plus a TypeScript CLI package (`gitim-cli`). The daemon is the single binary that links all Rust crates. The CLI is a thin client that talks to the daemon over Unix socket.

**Tech Stack:** Rust (tokio, serde, regex, axum), TypeScript (Node.js, commander)

**Spec:** `docs/superpowers/specs/2026-03-16-gitim-v1-design.md`

---

## Dependency Graph

```
Phase 0: Scaffolding + Core Types (serial, must complete first)
  │
  ├→ Stream A: gitim-core (parser + validator)
  ├→ Stream B: gitim-daemon (server + lifecycle)
  ├→ Stream C: gitim-sync (git engine)
  └→ Stream D: gitim-cli (TypeScript CLI)
```

Stream A is the critical path — B and C depend on A's types and traits. D depends on B's API contract (can mock). All four streams can begin in parallel after Phase 0, since A's type definitions are established there.

---

## Chunk 1: Phase 0 — Scaffolding

### Task 0.1: Rust Workspace Setup

**Files:** `Cargo.toml` (workspace root), `crates/gitim-core/`, `crates/gitim-daemon/`, `crates/gitim-sync/`

- [x] Step 1: Create workspace Cargo.toml（workspace members + shared dependencies）
- [x] Step 2: Create gitim-core crate（serde, regex, thiserror, chrono）
- [x] Step 3: Create gitim-daemon crate（依赖 gitim-core + gitim-sync, axum, tokio）
- [x] Step 4: Create gitim-sync crate（依赖 gitim-core, tokio, thiserror）
- [x] Step 5: Verify workspace builds — `cargo check`
- [x] Step 6: Commit

---

### Task 0.2: Core Type Definitions

**Files:** `crates/gitim-core/src/types/` — handler.rs, message.rs, meta.rs, config.rs, mod.rs

- [x] Step 1: Create Handler newtype（验证规则：a-z0-9连字符, 1-39字符, 禁止system, 实现 serde try_from/into）
- [x] Step 2: Create Message / ThreadLine / ThreadFile 类型
- [x] Step 3: Create UserMeta / ChannelMeta 类型（serde JSON）
- [x] Step 4: Create Config / DaemonConfig 类型（带默认值：sync_interval=30, debug_port=3000）
- [x] Step 5: Create types module re-exports
- [x] Step 6: Verify — `cargo check`
- [x] Step 7: Commit

---

### Task 0.3: TypeScript CLI Setup

**Files:** `cli/package.json`, `cli/tsconfig.json`, `cli/src/index.ts`

- [x] Step 1: Create package.json（type:module, bin:gitim, deps: commander）
- [x] Step 2: Create tsconfig.json（ES2022, Node16 module）
- [x] Step 3: Create entry point with commander skeleton
- [x] Step 4: Install and verify — `npm install && npx tsc --noEmit`
- [x] Step 5: Commit

---

## Chunk 2: Stream A — gitim-core (Parser + Validator)

### Task A.1: Thread File Parser

**Files:** `crates/gitim-core/src/parser.rs`, `crates/gitim-core/tests/parser_test.rs`

- [x] Step 1: Write parser tests（单消息、续行、多消息、混合、空文件、方括号正文、转义续行、大行号）
- [x] Step 2: Run tests to verify they fail
- [x] Step 3: Implement parser（LazyLock regex, CRLF→LF 标准化, 续行转义剥离）
- [x] Step 4: Run tests to verify they pass — all 8 tests PASS
- [x] Step 5: Commit

---

### Task A.2: Handler Validation Tests

**Files:** `crates/gitim-core/tests/handler_test.rs`

- [x] Step 1: Write handler validation tests（合法、保留字、空、超长、最大长度、非法字符、连字符边界、连续连字符）
- [x] Step 2: Run tests — all 8 tests PASS（实现已在 Task 0.2 完成）
- [x] Step 3: Commit

---

### Task A.3: Meta & Config Validation

**Files:** `crates/gitim-core/src/validator.rs`, `crates/gitim-core/tests/validator_test.rs`

- [x] Step 1: Write validator tests（user meta 合法/缺字段/display_name超长, channel meta 合法/缺字段/非法日期/非法created_by, channel name 合法/非法, config 合法/版本错/缺版本）
- [x] Step 2: Run tests to verify they fail
- [x] Step 3: Implement validator（添加 serde_yaml 依赖, 字段长度约束, handler 交叉验证, 时间戳格式校验）
- [x] Step 4: Run tests to verify they pass — all 10 tests PASS
- [x] Step 5: Commit

---

### Task A.4: Write Validation (Compliance Check)

**Files:** `crates/gitim-core/src/validator/compliance.rs`, `crates/gitim-core/tests/compliance_test.rs`

- [x] Step 1: Refactor validator.rs → validator/mod.rs 模块化
- [x] Step 2: Write compliance tests（合法追加、行号跳跃、未知作者、无效P引用、批内P引用、空文件追加）
- [x] Step 3: Run tests to verify they fail
- [x] Step 4: Implement compliance validator（行号连续性、作者注册检查、P引用有效性、空body检查）
- [x] Step 5: Run tests to verify they pass — all 6 tests PASS
- [x] Step 6: Commit

---

### Task A.5: Message Formatter (Write Path)

**Files:** `crates/gitim-core/src/formatter.rs`, `crates/gitim-core/tests/formatter_test.rs`

- [x] Step 1: Write formatter tests（简单消息、回复、多行body、需要转义的续行、大行号）
- [x] Step 2: Run tests to verify they fail
- [x] Step 3: Implement formatter（动态行号宽度 max(digits,6), 续行 MSG_PREFIX_RE 检测后加前导空格）
- [x] Step 4: Run tests to verify they pass — all 5 tests PASS
- [x] Step 5: Commit

---

## Chunk 3: Stream B — gitim-daemon (Server)

### Task B.1: Daemon Lifecycle (PID/Lock/Socket)

**Files:** `crates/gitim-daemon/src/lifecycle.rs`, `crates/gitim-daemon/src/error.rs`

- [x] Step 1: Implement error types（AlreadyRunning, LockFailed, RepoNotFound, ConfigError）
- [x] Step 2: Implement lifecycle manager（.gitim/run/ 目录, PID 文件读写, stale PID 清理, socket path, port 文件, cleanup）。添加 libc 依赖用于 process_exists。
- [x] Step 3: Verify — `cargo check -p gitim-daemon`
- [x] Step 4: Commit

---

### Task B.2: Unix Socket Server + JSON API

**Files:** `crates/gitim-daemon/src/api.rs`, `crates/gitim-daemon/src/server.rs`

- [x] Step 1: Define API request/response types（serde tag="method", 6个方法: send/read/channels/users/thread/status）
- [x] Step 2: Implement socket server（UnixListener, 行分隔 JSON 协议, tokio spawn per connection）
- [x] Step 3: Wire up main.rs（tracing init, lifecycle check, ctrl_c cleanup, socket server start）
- [x] Step 4: Verify — `cargo check -p gitim-daemon`
- [x] Step 5: Commit

---

### Task B.3: HTTP Debug Server

**Files:** `crates/gitim-daemon/src/http.rs`

- [x] Step 1: Implement HTTP debug server（axum Router, POST /api, 复用同一 handler 逻辑）
- [x] Step 2: Wire HTTP server into main.rs（根据 config.daemon.debug_http 条件启动）
- [x] Step 3: Verify — `cargo check -p gitim-daemon`
- [x] Step 4: Commit

---

## Chunk 4: Stream C — gitim-sync (Git Engine)

### Task C.1: Git Operations

**Files:** `crates/gitim-sync/src/git.rs`

- [x] Step 1: Implement git operations wrapper（pull_rebase, add_and_commit, push, push_with_retry, has_remote）
- [x] Step 2: Verify — `cargo check -p gitim-sync`
- [x] Step 3: Commit

---

### Task C.2: File Watcher

**Files:** `crates/gitim-sync/src/watcher.rs`

- [x] Step 1: Implement file watcher（notify crate, 监控 channels/ 和 dm/, 区分 .thread 和 .meta.json 事件, tokio mpsc 转发）
- [x] Step 2: Verify — `cargo check -p gitim-sync`
- [x] Step 3: Commit

---

### Task C.3: Sync Loop

**Files:** `crates/gitim-sync/src/sync_loop.rs`

- [x] Step 1: Implement periodic sync（interval=0 禁用, 无 remote 禁用, tokio interval ticker）
- [x] Step 2: Verify — `cargo check -p gitim-sync`
- [x] Step 3: Commit

---

## Chunk 5: Stream D — gitim-cli (TypeScript CLI)

### Task D.1: Socket Client Library

**Files:** `cli/src/client.ts`

- [x] Step 1: Implement GitimClient（net.createConnection, 行分隔 JSON, 封装 status/send/read/listChannels/listUsers/getThread）
- [x] Step 2: Verify — `npx tsc --noEmit`
- [x] Step 3: Commit

---

### Task D.2: Daemon Auto-Launch

**Files:** `cli/src/daemon.ts`

- [x] Step 1: Implement daemon launcher（findRepoRoot 向上查找 .gitim/config.yaml, isDaemonRunning 检查 PID, ensureDaemon detached spawn + socket 轮询等待）
- [x] Step 2: Verify — `npx tsc --noEmit`
- [x] Step 3: Commit

---

### Task D.3: CLI Commands

**Files:** `cli/src/index.ts`, `cli/src/commands/` — init, send, read, channels, users, status

- [x] Step 1: Implement init command（创建 .gitim/ users/ channels/, 写 config.yaml, 更新 .gitignore）
- [x] Step 2: Implement remaining commands（统一模式：findRepoRoot → ensureDaemon → GitimClient → 调用 → 输出）
- [x] Step 3: Wire commands into index.ts
- [x] Step 4: Verify — `npx tsc --noEmit`
- [x] Step 5: Commit

---

## Chunk 6: Stream A Supplement — DM & Read-Path Validation

### Task A.6: DM Filename Utilities

**Files:** `crates/gitim-core/src/dm.rs`, `crates/gitim-core/tests/dm_test.rs`

- [x] Step 1: Write DM filename tests（字典序排列、含连字符 handler、前缀匹配）
- [x] Step 2: Run tests to verify they fail
- [x] Step 3: Implement dm_filename（字典序排列 + `--` 连接）和 parse_dm_filename
- [x] Step 4: Run tests to verify they pass — all 3 tests PASS
- [x] Step 5: Commit

---

### Task A.7: Read-Path Compliance Detection

**Files:** `crates/gitim-core/src/validator/read_check.rs`, `crates/gitim-core/tests/read_check_test.rs`

- [x] Step 1: Write read-path detection tests（干净文件、间隙、未知作者、无效P引用）
- [x] Step 2: Run tests to verify they fail
- [x] Step 3: Implement read-path checker（IntegrityIssue 枚举, 警告级别非硬错误）
- [x] Step 4: Run tests to verify they pass — all 4 tests PASS
- [x] Step 5: Commit

---

## Chunk 7: Stream C Supplement — Conflict Re-Numbering

### Task C.4: Conflict Resolution with Line Re-Numbering

**Files:** `crates/gitim-sync/src/renumber.rs`, `crates/gitim-sync/tests/renumber_test.rs`

- [x] Step 1: Write re-numbering tests（简单重编、保留外部P引用、含续行重编）
- [x] Step 2: Run tests to verify they fail
- [x] Step 3: Implement renumber_batch（构建 old→new 行号映射, 区分批内引用 vs 外部引用, 调用 formatter 重建输出）
- [x] Step 4: Run tests to verify they pass — all 3 tests PASS
- [x] Step 5: Commit

---

## Chunk 8: Integration & Wiring

### Task I.1: Daemon Shared State & Config Loading

**Files:** `crates/gitim-daemon/src/state.rs`, modify `main.rs`

- [x] Step 1: Create SharedState（repo_root, config, RwLock<thread_cache>, RwLock<users>）
- [x] Step 2: Wire config loading（读 .gitim/config.yaml → validate_config → 扫描 users/ 填充用户列表）
- [x] Step 3: Verify — `cargo check -p gitim-daemon`
- [x] Step 4: Commit

---

### Task I.2: Implement Request Handlers

**Files:** `crates/gitim-daemon/src/handlers.rs`, modify `server.rs`

- [x] Step 1: Implement `send` handler（compliance validate → format message → append .thread → git commit）
- [x] Step 2: Implement `read` handler（parse thread from cache/file → filter by limit/since → return JSON）
- [x] Step 3: Implement `channels`, `users`, `thread` handlers
- [x] Step 4: Add DM support（`dm:handler1,handler2` 格式路由到 dm/ 目录）
- [x] Step 5: Wire handlers into server with SharedState
- [x] Step 6: Commit

---

### Task I.3: Wire Sync & Watcher into Daemon

**Files:** modify `crates/gitim-daemon/src/main.rs`

- [x] Step 1: Start sync loop from config（读 sync_interval, tokio spawn）
- [x] Step 2: Start file watcher with cache invalidation（FileEvent → 清除 thread_cache 对应条目）
- [x] Step 3: Run read-path integrity check after sync（git pull 后扫描变更的 .thread 文件, 日志警告）
- [x] Step 4: Commit

---

### Task I.4: End-to-End Test

**Files:** `tests/e2e_test.sh`

- [x] Step 1: Write e2e test（git init → gitim init → 创建用户 → 启动 daemon → status/send/read/DM → cleanup）
- [x] Step 2: Run test — passed
- [x] Step 3: Commit

---

### Task I.5: Add DM CLI Commands

**Files:** `cli/src/commands/dm.ts`, modify `cli/src/index.ts`

- [x] Step 1: Add DM subcommands（`gitim dm send`, `gitim dm read`, `gitim dm list`）
- [x] Step 2: Verify — `npx tsc --noEmit`
- [x] Step 3: Commit

---

## Parallel Execution Map

| Stream | Tasks | Dependencies | Can start after |
|--------|-------|-------------|-----------------|
| Phase 0 | 0.1, 0.2, 0.3 | None | Immediately |
| Stream A | A.1–A.5 | Phase 0 | Phase 0 complete |
| Stream A+ | A.6, A.7 | Phase 0 | Phase 0 complete (parallel with A.1–A.5) |
| Stream B | B.1, B.2, B.3 | Phase 0 (types only) | Phase 0 complete |
| Stream C | C.1, C.2, C.3 | Phase 0 (types only) | Phase 0 complete |
| Stream C+ | C.4 | A.5 (needs formatter) | Stream A.5 complete |
| Stream D | D.1, D.2, D.3 | Phase 0 | Phase 0 complete |
| Integration | I.1–I.5 | A + B + C + D all complete | All streams complete |
