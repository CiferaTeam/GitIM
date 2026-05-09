# Cron Trigger Implementation Plan

> 配套 design doc：[design.md](./design.md)。每个任务的具体决策依据落在 design 里，本文只写**分工 + 验收 + 测试目标**（参考 `feedback_plan_no_code.md`：plan 不写实现代码，typed code + tests 才是契约）。
>
> 执行约定：每完成一个 task 就 commit（参考 `feedback_commit_each_task.md`）。中途不跑全量 `cargo test`，只跑 scoped（参考 CLAUDE.md "贵，别无脑跑"）。Wave 末尾 / 整体收尾再跑全量。

**Goal:** 给 gitim 加 cron trigger — 协议级 `crons/` 目录 + daemon 内 cron engine + CLI 自助创建 + WebUI 日历。

**Branch:** `claude/hopeful-yalow-fa38b8`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/gitim-sync/tests/non_thread_rebase.rs` | Create | Wave 0 验证 spec.yaml rebase 行为 |
| `crates/gitim-sync/src/sync_loop.rs` | Modify (CONDITIONAL) | 修非 thread 文件 rebase 丢失（仅 Wave 0 暴露 bug 时） |
| `crates/gitim-core/src/types/cron.rs` | Create | `CronSpec` + 校验 + `next_fire_after` |
| `crates/gitim-core/src/types/mod.rs` | Modify | 注册 `cron` 模块 |
| `crates/gitim-core/Cargo.toml` | Modify | 加 `croner` dep |
| `Cargo.toml` | Modify | workspace 加 `croner` |
| `crates/gitim-daemon/src/handlers/cron.rs` | Create | 6 个 IPC handlers（create/list/show/history/disable+enable/delete） |
| `crates/gitim-daemon/src/handlers/mod.rs` | Modify | 注册 `cron` 模块 |
| `crates/gitim-daemon/src/api.rs` | Modify | IPC request/response 变体 |
| `crates/gitim-daemon/src/cron_engine.rs` | Create | `scan_due()` + `fire()` |
| `crates/gitim-daemon/src/lib.rs` | Modify | 注册 `cron_engine` 模块 |
| `crates/gitim-daemon/src/lifecycle.rs` | Modify | spawn cron engine task |
| `crates/gitim-client/src/lib.rs` | Modify | client 端 8 个 cron API 方法 |
| `crates/gitim-cli/src/cron.rs` | Create | clap subcommand 实现 |
| `crates/gitim-cli/src/main.rs` | Modify | 注册 cron subcommand |
| `crates/gitim-runtime/src/http.rs` | Modify | 5 个 HTTP 路由（list/show/runs/runs/<ts>/timeline） |
| prompt 模板源（post prompt-refactor 在 `gitim-agent-provider/src/prompts.rs`） | Modify | 注入 cron 用法 section |
| `products/gitim/frontend/...` | Create / Modify | `/crons` route + 月历 + day detail panel（具体路径在 Task 6.1 indwell 后定） |
| `tests/integration/cron_e2e.rs`（或现有 integration tests 位置） | Create | E2E：create → wait → fire → agent process |
| `CLAUDE.md` | Modify | 加 cron 架构说明到 Crate 地图 + Current Orientation |

---

## Wave 0: 先决条件验证

### Task 0.1: 验证 sync_loop 对非 thread 文件 rebase 冲突的处理

**Files:**
- Create: `crates/gitim-sync/tests/non_thread_rebase.rs`

**Goal:** 弄清 [crates/gitim-sync/src/sync_loop.rs:332](crates/gitim-sync/src/sync_loop.rs:332) `diff_unpushed("*.thread")` 的非 thread 文件路径在 rebase 冲突时是否丢失（参考 memory `project_sync_loop_non_thread_bug.md`）。

**Sub-steps:**
1. 起两个 GitStorage clone 指向同一 bare repo
2. Clone A 改 `users/<handler>.meta.yaml`（或类似非 thread 文件），commit + push
3. Clone B 同时改同一文件不同字段，commit
4. Clone B 触发 sync 拉 + rebase
5. 断言：要么先到先赢（B 改动 commit 上升为新 head 之前会先 fast-forward A 的改动），要么冲突报错；**不能静默丢 B 的改动**

**Tests required:**
- Happy fast-forward: A push 后 B pull 看到 A 的改动
- Conflict path: 两边并发改不同字段 → 期望要么 conflict 报错，要么有 merge 策略（先到先赢 / 字段级 merge）
- 静默丢失 = 测试失败

**Acceptance:**
- 测试要么 PASS（sync_loop 对非 thread 安全），要么 FAIL 暴露具体丢失场景

**Commit message:** `test(sync): non-thread file rebase conflict behavior`

---

### Task 0.2 (CONDITIONAL): 修 sync_loop 非 thread 丢失 bug

**只在 Task 0.1 暴露 bug 时执行。**

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`（rebase resolution 路径，~330–500 行附近）
- 可能 Modify: `crates/gitim-sync/src/conflict.rs`

**Goal:** 让 rebase 路径正确处理非 thread 文件改动，至少做到"冲突报错或先到先赢"，不能静默吃掉。

**Sub-steps:**
1. Indwell 现有 rebase resolution 逻辑：`thread_mappings` / `local_additions` 的过滤位置
2. 找到非 thread 文件被 drop 的精确点
3. 改成"非 thread 文件保留为 commit"或"冲突时报错"
4. 让 Task 0.1 的测试 PASS

**Tests required:**
- Task 0.1 的测试 PASS
- 现有 sync 测试不 regress

**Acceptance:**
- `cargo test -p gitim-sync` 全绿
- Task 0.1 测试 PASS
- memory `project_sync_loop_non_thread_bug.md` 标记为已解决（task 末尾建议用户更新该 memory）

**Commit message:** `fix(sync): preserve non-thread file changes through rebase`

---

## Wave 1: 协议层类型

### Task 1.1: 加 `croner` 依赖

**Files:**
- Modify: `Cargo.toml`（workspace `[workspace.dependencies]`）
- Modify: `crates/gitim-core/Cargo.toml`（`[dependencies]` 加 `croner = { workspace = true }`）

**Goal:** `croner` 在 gitim-core 可用。

**Acceptance:**
- `cargo check -p gitim-core` 通过

**Commit message:** `chore(core): add croner dep for cron expression parsing`

---

### Task 1.2: 定义 `CronSpec` 类型

**Files:**
- Create: `crates/gitim-core/src/types/cron.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`（`pub mod cron;`）
- Modify: `crates/gitim-core/src/lib.rs`（re-export `CronSpec`）

**Goal:** Serde-derivable struct + 解析 + 校验。

**Schema 字段（参考 design.md "spec.yaml 字段"）：**

| 字段 | Rust 类型 | 默认 | 校验 |
|---|---|---|---|
| `version` | `u32` | `1` | == 1（v1 拒绝其它） |
| `schedule` | `String` | — | croner 可解析 |
| `timezone` | `Option<String>` | `None` (= UTC) | IANA 有效（chrono-tz lookup） |
| `target` | `Handler`（已有类型） | — | 合法 handler 格式 |
| `prompt` | `String` | — | 非空，长度 ≤ 8KB |
| `enabled` | `bool` | `true` | — |
| `created_by` | `Handler` | — | 合法格式 |
| `created_at` | `String` (ISO 8601) | — | chrono 可解析 |
| `extra` | `BTreeMap<String, Value>` (`#[serde(flatten)]`) | `{}` | — (forward-compat) |

**Methods:**
- `CronSpec::from_yaml(s: &str) -> Result<Self, CronSpecError>`
- `CronSpec::to_yaml(&self) -> Result<String, CronSpecError>`
- `CronSpec::validate(&self) -> Result<(), CronSpecError>`
- `CronSpec::is_active(&self) -> bool`（= `self.enabled`，方便 future 加 archived 字段时单点扩展）

**Tests required（在同文件 `#[cfg(test)] mod tests`）：**
- `parse_minimal_yaml` — 只必填字段
- `parse_full_yaml` — 所有字段
- `roundtrip_preserves_extra` — unknown field 在 to_yaml 后保留
- `reject_invalid_schedule` — 期望 specific error variant
- `reject_invalid_timezone`
- `reject_invalid_target_handler`
- `reject_empty_prompt`
- `reject_oversized_prompt` — > 8KB
- `default_timezone_is_utc`
- `default_enabled_is_true`
- `default_version_is_1`
- `reject_version_2`

**Acceptance:**
- `cargo test -p gitim-core types::cron` 全绿

**Commit message:** `feat(core): add CronSpec type with serde + validation`

---

### Task 1.3: `next_fire_after` 计算

**Files:**
- Modify: `crates/gitim-core/src/types/cron.rs`

**Goal:** 给定 spec + 起始时刻，算出下一次 fire 的 UTC 时刻。

**Signature:**
```
pub fn next_fire_after(
    spec: &CronSpec,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, CronSpecError>
```

**Sub-steps:**
1. 解析 `spec.timezone`（默认 UTC）
2. 把 `after` 转到该 tz 下
3. 用 `croner` 算 next fire（in tz）
4. 转回 UTC 返回

**Tests required:**
- `next_monday_9am_from_sunday`
- `alias_daily_from_arbitrary_time`
- `tz_la_morning_from_utc`（spec.tz=America/Los_Angeles, schedule=`0 9 * * *`，from UTC 16:00 → 期望 next 是次日 LA 9am = UTC 17:00 那天还没过 → 当日 17:00；from UTC 18:00 → 次日 17:00）
- `dst_forward_no_double_fire`（"spring forward" 跳过 2:30 时，cron `30 2 * * *` 那天 skip）
- `dst_backward_no_double_fire`（"fall back" 重复 1:30 时，cron `30 1 * * *` 那天只 fire 一次）
- `invalid_schedule_returns_error`

**Acceptance:**
- `cargo test -p gitim-core types::cron::next_fire` 全绿
- DST 测试用 fixed dates（如 2026-03-08 美国 DST forward）保证可重复

**Commit message:** `feat(core): cron next_fire_after with timezone + DST`

---

## Wave 2 — Lane A: Daemon (handlers + engine)

### Task 2.1: IPC 类型 + handler 框架

**Files:**
- Create: `crates/gitim-daemon/src/handlers/cron.rs`（占位 stub）
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`（`pub mod cron;`）
- Modify: `crates/gitim-daemon/src/api.rs`（`Request` enum 加 6 个变体；`Response::data` 加对应 payload）

**Goal:** Daemon 接受 cron IPC，暂时返回 `not_implemented`。

**IPC 变体（命名沿用现有 PascalCase + snake_case 风格，看 api.rs）：**
- `CreateCron { name, schedule, timezone, target, prompt }` → `Response`
- `ListCrons {}` → `[CronSummary]`
- `ShowCron { name }` → `CronDetail`
- `HistoryCron { name, limit?: u32 }` → `[ThreadFileEntry]`
- `EnableCron { name }`, `DisableCron { name }` → `Response`
- `DeleteCron { name }` → `Response`

**Tests required:**
- IPC roundtrip（serde JSON）每个 variant
- 路由到 cron.rs handler，stub 返回 `error: not_implemented`

**Acceptance:**
- `cargo check -p gitim-daemon` 通过
- `cargo test -p gitim-daemon api::cron` 全绿

**Commit message:** `feat(daemon): scaffold cron IPC types and handler dispatch`

---

### Task 2.2: `handle_create_cron`

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/cron.rs`

**Goal:** 实现 create 全流程 — 校验、`@self` 解析、写 spec.yaml、commit。

**Sub-steps:**
1. 校验 `name`：小写 a-z 0-9 连字符，1–63 字符，不是 `archive` / `.archive`
2. 校验唯一性：检查 `crons/<name>/spec.yaml` 和 `archive/crons/<name>/spec.yaml` 不存在
3. 校验 `schedule`：构造 `CronSpec` 临时实例 + `validate()`
4. 校验 `timezone`：交给 `CronSpec::validate`
5. 校验 `target`：
   - 如果 `@self` → 替换为 author handler
   - 否则验证 `users/<target>.meta.yaml` 存在
6. 校验 `prompt`：非空，≤ 8KB
7. 拿 `commit_lock`
8. mkdir `crons/<name>/`
9. 写 `spec.yaml`（含 `version: 1`、`created_by: author`、`created_at: now`）
10. `git_storage.add_and_commit_as(&["crons/<name>/spec.yaml"], "cron: create <name> by @<author>", author_email)`
11. 释放 lock

**Tests required（用现有 daemon test harness，参考 `handlers/send` 测试）：**
- `create_happy_path` — 文件存在 + commit author / message 正确
- `create_name_invalid` — 大写 / 空 / 太长 → error
- `create_name_conflict_active` — 已存在 → `error_code: name_conflict`
- `create_name_conflict_archived` — `archive/crons/<name>/` 存在 → `name_conflict`
- `create_invalid_schedule` → `invalid_schedule`
- `create_invalid_timezone` → `invalid_timezone`
- `create_self_target_resolves` — `target=@self`，spec 存的是 `@<author>`
- `create_target_not_found` → `target_not_found`
- `create_empty_prompt` → `prompt_empty`
- `create_oversized_prompt` → `prompt_too_large`

**Acceptance:**
- `cargo test -p gitim-daemon handlers::cron::create_*` 全绿

**Commit message:** `feat(daemon): cron create handler with validation + @self resolution`

---

### Task 2.3: `handle_list` / `handle_show` / `handle_history`

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/cron.rs`

**Goal:** 只读端点。

**Sub-steps（list）：**
1. 读 `crons/*/spec.yaml`（skip `archive/`）
2. 每个 → `CronSummary { name, schedule, timezone, target, enabled, next_fire }`
3. 按 name 排序返回

**Sub-steps（show）：**
1. 读 `crons/<name>/spec.yaml`，404 if missing
2. 列 `crons/<name>/*.thread` 文件，取最近 5 个
3. 计算 `next_fire`（用 `last_fire` 文件名 OR `created_at`）
4. 返回 `CronDetail { spec, recent_runs, next_fire }`

**Sub-steps（history）：**
1. 列 `crons/<name>/*.thread`，按文件名（即时间戳）倒序
2. 默认 limit=50，可由 request 覆盖

**Tests required:**
- `list_empty` → `[]`
- `list_with_active_and_archived` — 只返回 active
- `list_sort_by_name`
- `show_existing` — 含 `next_fire` 字段
- `show_missing` → 404
- `show_no_runs_yet` — `recent_runs: []`
- `history_empty`
- `history_pagination`（limit=2 验证只返 2）

**Acceptance:**
- `cargo test -p gitim-daemon handlers::cron::{list,show,history}` 全绿

**Commit message:** `feat(daemon): cron list/show/history handlers`

---

### Task 2.4: `handle_disable` / `handle_enable` / `handle_delete`

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/cron.rs`

**Goal:** 状态变更。

**Sub-steps（disable / enable）：**
1. 读 spec.yaml
2. 比较当前 `enabled` 和目标值；相同直接返回（idempotent，不 commit）
3. 否则修改 + 写回 + commit

**Sub-steps（delete = 软删除）：**
1. 拿 commit_lock
2. mkdir `archive/crons/`（如果不存在）
3. `git mv crons/<name>/ archive/crons/<name>/`（可能需要先 stage 再 commit；看 channel-archive 的 `handle_archive_channel` 实现）
4. commit message: `cron: delete <name> by @<author>`

**Tests required:**
- `disable_then_enable_roundtrip` — 其它字段不变
- `disable_already_disabled_idempotent` — 不产生新 commit
- `delete_active` — 文件移到 archive
- `delete_with_history` — 历史 thread 一起搬走
- `delete_already_archived` → 404
- `enable_archived` → 404

**Acceptance:**
- `cargo test -p gitim-daemon handlers::cron::{disable,enable,delete}` 全绿

**Commit message:** `feat(daemon): cron disable/enable/delete handlers`

---

### Task 2.5: `cron_engine::scan_due` 算法

**Files:**
- Create: `crates/gitim-daemon/src/cron_engine.rs`
- Modify: `crates/gitim-daemon/src/lib.rs`（`pub mod cron_engine;`）

**Goal:** 纯函数风格的扫描算法（无 fs IO 直接耦合，方便测试）。

**Signature:**
```
pub struct FireRequest {
    pub spec: CronSpec,
    pub theoretical_ts: DateTime<Utc>,
}

pub fn scan_due(
    crons_dir: &Path,
    self_handler: &Handler,
    now: DateTime<Utc>,
) -> Result<Vec<FireRequest>, CronEngineError>
```

**算法（参考 design.md "关键算法"）：**

```
for spec_dir in crons/*/ (skip archive/):
    spec = parse spec_dir/spec.yaml
    if !spec.enabled: continue

    if spec.target != self_handler: continue                 # ① ownership

    last_fire = max(parse ts from spec_dir/<ts>.thread)
                || spec.created_at                             # ② idempotency
    next_due = next_fire_after(spec, last_fire)

    if next_due <= now:
        push FireRequest { spec, theoretical_ts: next_due }   # ③ filename = theoretical
```

**Tests required（用 tempdir 构造 fixture）：**
- `scan_empty_workspace` → `[]`
- `scan_disabled_excluded`
- `scan_ownership_filter` — spec.target=@bob，self=@alice → `[]`
- `scan_due_returned`
- `scan_already_fired_skipped` — `<ts>.thread` 已存在 → 不重复
- `scan_bootstrap_no_thread_files` — 用 `created_at` 算 last_fire
- `scan_dst_forward_no_double_fire`
- `scan_archive_dir_skipped` — `archive/crons/<name>/` 不被扫到
- `scan_malformed_spec_logged_skipped` — 损坏的 spec.yaml 不能 crash 整个 scan，只 skip 该 spec

**Acceptance:**
- `cargo test -p gitim-daemon cron_engine::scan_*` 全绿
- 所有不变式（ownership / idempotency / bootstrap）有专属测试

**Commit message:** `feat(daemon): cron_engine scan_due with three invariants`

---

### Task 2.6: `cron_engine::fire`

**Files:**
- Modify: `crates/gitim-daemon/src/cron_engine.rs`

**Goal:** 单次 fire 的副作用（写文件 + commit）。

**Signature:**
```
pub async fn fire(
    state: &AppState,
    request: FireRequest,
) -> Result<(), CronEngineError>
```

**Sub-steps:**
1. 构造文件名：`crons/<spec.name>/<theoretical_ts as ISO with `:` → `-`>.thread`
2. 构造首行：`[L1][@system][<theoretical_ts>] cron(<spec.name>): <spec.prompt>`（多行 prompt 用 thread 续行规则）
3. 拿 `state.commit_lock.lock()`
4. 如果文件已存在 → 释放锁，返回 `Ok(())`（race-safe，重复 scan 不应导致错误）
5. 写文件
6. `git add` + `git commit`（author=`system`，email = `state.github_email` 或 fallback `system@gitim`）
7. 释放锁

**Tests required:**
- `fire_happy_path` — 文件 + commit
- `fire_already_exists_no_op` — 第二次 fire 同 ts 不报错也不再 commit
- `fire_lock_held_blocks_then_proceeds`
- `fire_author_email_from_state_github_email`
- `fire_author_email_fallback_when_github_email_absent`
- `fire_multiline_prompt_uses_continuation_lines` — prompt 包含 `\n` 时正确格式化

**Acceptance:**
- `cargo test -p gitim-daemon cron_engine::fire_*` 全绿

**Commit message:** `feat(daemon): cron_engine fire under commit_lock`

---

### Task 2.7: 在 lifecycle 中 spawn engine task

**Files:**
- Modify: `crates/gitim-daemon/src/lifecycle.rs`

**Goal:** Daemon 启动时拉起 cron engine 后台任务，60s tick。

**Sub-steps:**
1. Indwell 现有 lifecycle.rs：sync_loop 怎么 spawn / 怎么收 cancellation token / log target
2. 新加 `spawn_cron_engine(state: Arc<AppState>) -> JoinHandle`
3. Loop body：
   - `let now = Utc::now()`
   - `let self_handler = read_handler_from_state(state)` — 从 me.json
   - `let requests = scan_due(&state.repo_root.join("crons"), &self_handler, now)?`
   - `for req in requests: fire(state, req).await.log_err()`
4. tokio::time::interval(Duration::from_secs(60))
5. 接 cancellation：daemon shutdown 时 task 退出

**Tests required（integration，可能需要现有 daemon test harness）：**
- `engine_starts_and_ticks` — 起 daemon，等 ~70s，无 panic
- `engine_fires_due_spec` — 准备一个 schedule 为"now+1min"的 spec，等 ~70s，断言 thread 文件存在
- `engine_skips_non_owned` — daemon 的 me.json handler=@alice，spec.target=@bob → 不 fire
- `engine_survives_malformed_spec` — 损坏的 spec.yaml 不 panic 整个 daemon
- `engine_stops_on_shutdown` — daemon shutdown 后 task 退出（不 leak）

**Acceptance:**
- `cargo test -p gitim-daemon --test cron_engine_integration` 全绿
- 跑一次 `cargo test -p gitim-daemon`（scoped），不 regress

**Commit message:** `feat(daemon): spawn cron_engine task in lifecycle`

---

## Wave 2 — Lane B: Client + CLI

### Task 3.1: Client API methods

**Files:**
- Modify: `crates/gitim-client/src/lib.rs`

**Goal:** GitimClient 上加 8 个 cron 方法，对应 IPC 变体。

**Methods（命名沿用现有风格，参考 channel/send 方法）：**
- `async fn create_cron(name, schedule, timezone, target, prompt) -> Result<()>`
- `async fn list_crons() -> Result<Vec<CronSummary>>`
- `async fn show_cron(name) -> Result<CronDetail>`
- `async fn history_cron(name, limit) -> Result<Vec<ThreadFileEntry>>`
- `async fn enable_cron(name) -> Result<()>`
- `async fn disable_cron(name) -> Result<()>`
- `async fn delete_cron(name) -> Result<()>`
- `async fn next_fire_for(name) -> Result<DateTime<Utc>>` — 内部走 `show_cron` 取 `next_fire` 字段（不需要新 IPC）

**Tests required:**
- 每个方法用 mock daemon test fixture（参考现有 client tests）：构造预期 request、stub response、断言客户端正确解析

**Acceptance:**
- `cargo test -p gitim-client cron_*` 全绿

**Commit message:** `feat(client): cron API methods`

---

### Task 3.2: CLI subcommand

**Files:**
- Create: `crates/gitim-cli/src/cron.rs`
- Modify: `crates/gitim-cli/src/main.rs`

**Goal:** `gitim cron <subcommand>`。

**Subcommands（design.md "命令集"）：**
- `create <name> --schedule <s> --target <t> (--prompt <p> | --prompt-file <path>) [--timezone <tz>]`
- `list [--json]`
- `show <name> [--json]`
- `history <name> [--limit <n>] [--json]`
- `disable <name>`
- `enable <name>`
- `delete <name>`
- `next <name>`

**Sub-steps:**
1. 用 clap derive 定义 Cron + 子命令 enum
2. `--prompt` / `--prompt-file` 用 clap `conflicts_with` + `required_unless_present`
3. `--prompt-file`：读文件、UTF-8 校验、长度校验
4. 输出格式：
   - 默认 list/show/history → 表格（沿用现有 cli 风格，看 `gitim channels` 输出）
   - `--json` → JSON
   - next → ISO 时间戳一行
5. 错误转译：daemon 返回 `error_code` → 友好中文消息（参考 onboard 错误转译）

**Tests required:**
- 每个 subcommand 解析正确（clap unit test）
- `--prompt` + `--prompt-file` 同时给 → clap 报 conflict
- 都不给 → clap 报 required
- `--prompt-file` 读不存在的文件 → 友好错误
- `list` empty workspace 的输出

**Acceptance:**
- `cargo test -p gitim-cli cron::*` 全绿
- 手动验证：build cli + 跑一次 `gitim cron create test --schedule '@daily' --target @lewis --prompt 'hi'` 成功落 spec.yaml

**Commit message:** `feat(cli): cron subcommands with --prompt-file`

---

## Wave 2 — Lane C: Runtime HTTP

### Task 4.1: List / show / runs / single-run endpoints

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

**Goal:** 5 个 read-only HTTP 路由（前 4 个）。

**Routes（design.md "Runtime HTTP API"）：**
- `GET /workspaces/<slug>/crons` → `200 {crons: [CronSummary]}`
- `GET /workspaces/<slug>/crons/<name>` → `200 CronDetail` / `404`
- `GET /workspaces/<slug>/crons/<name>/runs` → `200 [{ts, filename, line_count?}]`
- `GET /workspaces/<slug>/crons/<name>/runs/<ts>` → `200 {body: <thread file content>}` / `404`

**Sub-steps:**
1. 沿用 http.rs 现有 axum router 注册套路（参考 channel/dm 路由）
2. 通过 `WorkspaceContext` → `GitimClient` 调 daemon
3. `<ts>` 路径参数解析：URL 安全的 ISO 时间戳（`-` 而非 `:`）
4. CORS / 错误响应沿用现有 helper

**Tests required（用 axum-test 或 reqwest + spawned runtime）：**
- 每个 endpoint happy path
- 404 paths
- 路径参数包含特殊字符 → 400
- workspace 不存在 → 404

**Acceptance:**
- `cargo test -p gitim-runtime http::cron_*` 全绿

**Commit message:** `feat(runtime): cron HTTP read endpoints`

---

### Task 4.2: Timeline endpoint

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`
- 可能 Modify: `crates/gitim-client/src/lib.rs`（如果需要新 IPC 帮 timeline 高效拉数据）

**Goal:** 单 endpoint 返回时间窗内的"过去/未来/missed"merge 列表，给前端日历用。

**Route:** `GET /workspaces/<slug>/crons/timeline?from=<ISO>&to=<ISO>`
- 默认 from=本月初, to=本月末

**Response shape：**
```
{
  "entries": [
    { "ts": "2026-05-11T09:00:00Z", "kind": "past",    "cron_name": "weekly-report", "thread_url": "/.../runs/2026-05-11T09-00-00Z" },
    { "ts": "2026-05-12T09:30:00Z", "kind": "missed",  "cron_name": "daily-standup", "reason": "no thread file present" },
    { "ts": "2026-05-18T09:00:00Z", "kind": "future",  "cron_name": "weekly-report" }
  ]
}
```

**算法（design.md "timeline"）：**
1. 取 active specs（list_crons）
2. 对每个 spec：
   - 在 `[from, to]` 内枚举所有理论 fire 时刻（用 `next_fire_after` 迭代）
3. 列每个 spec 的 `*.thread` 文件名（即实际过去 fire），落在 `[from, to]` 内的取出
4. 三类合并：
   - 实际过去 fire（kind=past）= 实际文件
   - 理论 fire 时刻 ≤ now 且无对应实际文件 → kind=missed
   - 理论 fire 时刻 > now → kind=future
5. 排序返回

**Tests required:**
- 空 workspace → `[]`
- 一个 spec 全过去 fire → 全 past
- 一个 spec 全未来 → 全 future
- 一个 spec 跨 now → past + future 混
- daemon-was-offline 模拟（理论时刻有但文件没）→ kind=missed
- 时间窗外的过去 fire 不返回
- DST 边界：跨 DST 的 spec 计算正确

**Acceptance:**
- `cargo test -p gitim-runtime http::cron_timeline` 全绿

**Commit message:** `feat(runtime): cron timeline endpoint with missed computation`

---

## Wave 2 — Lane D: Agent prompt

### Task 5.1: 注入 cron 用法到 system prompt

**Files:**
- Indwell first：找当前 prompt 模板的位置（post prompt-refactor 应该在 `crates/gitim-agent-provider/src/prompts.rs`，但确认一下）
- Modify: 上述模块（加新 section function）
- 可能 Modify: 该 crate 的 `Provider` trait（如果 prompt-refactor 用了 trait method 模式，cron 也要加同样的 method）

**Goal:** Agent system prompt 多一段告诉它如何用 `gitim cron`。

**文案（用交接语气，参考 `feedback_prompt_style_for_llms.md`）：**

> 你可以给自己或别的 agent 安排定期任务。例：
>
> ```
> gitim cron create weekly-summary \
>   --schedule "0 9 * * 1" \
>   --target @self \
>   --prompt "扫一下上周 #general 的关键讨论，整理成周报发到 #general"
> ```
>
> 时间表达式是标准 5-field cron。`gitim cron list` 看现有的。
>
> 被 `[@system] cron(<name>) ...` 消息唤醒就是定期任务到点。处理完后回一条 "做完了 + 简要日志" 到那条消息所在 thread，未来可回溯。

**Sub-steps:**
1. 加 `default_cron_usage(ctx: &PromptContext) -> String`
2. 在 `build_system_prompt` 把它接进去（位置：identity / communication 后，能力清单类 section）
3. 沿用 prompt-refactor 的 trait method 默认实现模式（如果存在）

**Tests required:**
- snapshot test: full system prompt 包含 "你可以给自己或别的 agent 安排定期任务"
- provider override 测试（如果 trait 方式）

**Acceptance:**
- `cargo test -p gitim-agent-provider prompts` 全绿
- `cargo test -p gitim-runtime` 不 regress

**Commit message:** `feat(prompts): inject cron usage into agent system prompt`

---

## Wave 3: Frontend

### Task 6.1: Indwell + 路由 + 月历组件

**Files:**
- Indwell first：`products/gitim/frontend/src/` 现有结构 — 路由库、状态管理（zustand）、UI lib（Radix + Tailwind）、是否已有日历组件
- Create / Modify: 路由注册 + Cron tab nav 入口
- Create: 月历主组件（命名风格沿用现有，例如 `CronCalendar.tsx`）
- Create: timeline 数据 hook（fetch `/timeline`，可能用现有 query lib 模式）

**Goal:** WebUI 多一个 `/crons` tab，月历视图渲染 `timeline` 端点的数据。

**子组件设想：**
- `CronCalendar` — 月格子布局，每格列出当天 entries（max 3 条 + "+N more"）
- `CalendarHeader` — 月份切换 + "今天" 按钮
- `CalendarEntryDot` — 单条目（颜色：past=绿 / future=蓝 / missed=红）
- `useCronTimeline(from, to)` — fetch hook

**Sub-steps:**
1. Indwell 现有 frontend 路由 + nav 注册套路
2. 决定日历用现成库（react-big-calendar / FullCalendar）还是手撸 grid（参考 design 备选；30-day grid 不复杂）
3. 加路由 + nav entry
4. 实现 fetch hook（参考现有 zustand selector 套路 + memory `project_zustand_selector_pitfalls.md`）
5. 实现月格组件
6. 颜色 / hover 用 Radix Tooltip

**Tests required:**
- Component test（@testing-library）：mock hook 返回空 → 渲染空月历
- Mock 返回混合 past/future/missed → 各颜色 dot 出现
- 切换月份触发新 fetch

**Acceptance:**
- `bun test` 在 `products/gitim/frontend/` 内全绿
- 手动验证：跑前端 dev server，打开 /crons，看到正确数据
- 遵守 DESIGN.md 字体 / 间距（参考 CLAUDE.md "Design System"）

**Commit message:** `feat(webui): cron calendar tab with month view`

---

### Task 6.2: Day detail panel + 单 fire 详情

**Files:**
- Modify / Create: 在 6.1 同目录加 `CronDayPanel.tsx` + `CronRunViewer.tsx`

**Goal:** 点某天 → 弹 panel 列当天所有 entries；点单条 → 进详情。

**交互：**
- 日历某格点击 → side panel / modal 打开当天 entries（按时间）
- 单 entry 点击：
  - past → 用现有 ThreadViewer 组件展开 thread 文件内容
  - future → 显示 spec detail（schedule / target / prompt 模板 / next fires）
  - missed → 显示 "missed at <ts>" + spec detail + "原因：runtime 当时未运行" 提示

**Sub-steps:**
1. 复用现有 thread viewer（webui-v2 应该已有）
2. spec detail 视图：fetch `/crons/<name>` endpoint
3. missed 视图：和 future 共用 spec detail，加一个 "missed" badge

**Tests required:**
- 三种 entry kind 各自渲染正确
- 点 past 跳 thread viewer
- 点 future / missed 显示 spec detail

**Acceptance:**
- 同 6.1
- 三种 kind 都跑通

**Commit message:** `feat(webui): cron day detail panel and run viewer`

---

## Final Wave: Integration + Docs

### Task 7.1: E2E 集成测试

**Files:**
- 决定位置：现有 integration tests 在哪里？看 daemon `tests/` 下有没有 e2e harness。倾向放 `crates/gitim-runtime/tests/cron_e2e.rs`（runtime 是协调 daemon + agent 的层）

**Goal:** 完整流程测试 — provision agent → create cron → wait → 验证 fire → agent 处理 → response 落 thread。

**Sub-steps:**
1. 起 daemon（serial_test 单线程）
2. 用 `mock` provider provision 一个 agent（参考 `gitim-runtime/tests/poller` 套路）
3. CLI 或 client API create 一个 cron — schedule "* * * * *"（每分钟）, target=@self
4. 等 ~70s
5. 断言 `crons/<name>/<ts>.thread` 文件存在
6. 断言 mock provider 收到 prompt（含 `cron(<name>)` 字符串）
7. 模拟 agent 回复一条"做完了"消息
8. 断言该消息在 cron thread 里

**Tests required:**
- 上述完整流程跑通
- 标记 `#[ignore]` 如果运行时长过长（手动跑），但默认应跑（70s 在 CI 可接受）

**Acceptance:**
- `cargo test -p gitim-runtime --test cron_e2e -- --ignored` 全绿（如果 ignore）或 `cargo test -p gitim-runtime --test cron_e2e` 全绿

**Commit message:** `test(integration): cron e2e flow`

---

### Task 7.2: 文档更新

**Files:**
- Modify: `CLAUDE.md`

**Goal:** 给未来的 agent / 维护者留下 cron 架构 orientation。

**Sub-steps:**
1. Crate 地图加一行 `gitim-daemon/src/cron_engine.rs` 的角色
2. 新加一节"Cron trigger 架构"（在 Hermes profile 隔离机制旁边），简述：
   - 协议级 `crons/` 目录
   - daemon 内 cron engine 三个不变式（ownership / idempotency / bootstrap）
   - fire = 写 thread 复用消息机制
   - missed 由 calendar UI 实时计算
   - 已知 non-goals
3. Current Orientation 节加一条 learning（如果实施过程中遇到非显然问题）

**Tests required:**
- 无（文档）

**Acceptance:**
- CLAUDE.md 改动 commit

**Commit message:** `docs: add cron architecture to CLAUDE.md`

---

### Task 7.3: 全量回归 + memory 更新建议

**Sub-steps:**
1. `cargo test`（全量，~数分钟）— 确认 baseline 干净
2. 如果 Wave 0 修了 sync_loop bug：建议用户更新 memory `project_sync_loop_non_thread_bug.md`（标记 resolved）
3. 如果实施中发现非显然问题：建议用户加一条新 memory（参考"Tensions"风格）

**Acceptance:**
- 全量测试 PASS
- 用户决定要不要落 memory

**Commit message:** 无（这是 cleanup task）

---

## 任务依赖图

```
   Wave 0:    0.1 -> 0.2 (CONDITIONAL)
                    │
                    ▼
   Wave 1:    1.1 -> 1.2 -> 1.3
                            │
                            ▼
   Wave 2 ───────────────────────────────── (parallel)
   Lane A:    2.1 -> 2.2 -> 2.3 -> 2.4 -> 2.5 -> 2.6 -> 2.7
   Lane B:    3.1 -> 3.2
   Lane C:    4.1 -> 4.2
   Lane D:    5.1
                            │
                            ▼
   Wave 3:    6.1 -> 6.2  (depends on Lane C 4.1+4.2)
                            │
                            ▼
   Final:     7.1 -> 7.2 -> 7.3
```

**Worktree 并行建议**（参考 `superpowers:dispatching-parallel-agents`）：
- Lane A、B、C、D 在 Wave 1 完成后可拆 4 个 worktree 并行实施
- Wave 0 / 1 / Final 必须 sequential
- Wave 3 frontend 需要 Lane C 完成后才有可用 API

---

## Self-Review checklist

- [ ] 每个任务 Files 列表精确（绝对 vs crate-relative 一致）
- [ ] 每个任务 Tests required 具体到 case 名，不是 "add tests"
- [ ] 三个 cron_engine 不变式（ownership / idempotency / bootstrap）每个有专属测试（task 2.5）
- [ ] sync_loop 先决条件（Wave 0）作为 blocking 任务，不是 footnote
- [ ] frontend 任务用 indwell-first 风格（先扫现有结构再决定具体路径）
- [ ] 所有任务都有 commit message convention
- [ ] 每个任务可独立 commit，不积压
