# Oneshot Timer — Requirements & Design

**Status**: Approved, ready for plan-eng-review → writing-plans
**Owner**: lewis
**Worktree**: `.claude/worktrees/ecstatic-hugle-03df01`
**Date**: 2026-05-20

## Motivation

GitIM agent 是 oneshot 运行——一轮 LLM 调用结束后进程退出。当 agent 判断"这件事要过一段时间再回来看看"（等 deploy 完成、给对方时间回复、自己 cool-down 后复盘），它常说"30 分钟后我再看一下"，但**实际上 30 分钟后没有任何机制会唤醒它**。

需要一个最小的"一次性提醒"原语：agent 注册"N 分钟后唤醒我 + 这是当时的上下文锚点"，到点 runtime 重新触发它一次。

约束（用户明示）：
- **不进 git log**（timer 是 ephemeral 状态，不该污染历史）
- **存储位置宽松**——runtime 内存或本地文件都行
- **精度宽松**——10 秒级足够

## Approved Decisions（grill 收敛）

1. **触发语义**：runtime 在调 LLM 前把 `## ⏰ Timer reminder(s) fired ...` synthetic message 拼到 user prompt 最前面。不走 daemon、不进 git、不引入新 IM 概念。
2. **注册接口**：`gitim` CLI 新增 `timer` 子命令组（**不是** `gitim-runtime`）。agent 通过 shell out 调用。
3. **Storage 层级**：纯 fs，文件 `<agent_clone>/.gitim/timers.json`（gitignored，跟 `me.json` / `agent-state.json` 同目录）。**daemon 不参与、runtime 不加共享 state**。CLI 和 agent_loop 都直接 fs 读写。
4. **Watcher 位置**：**agent_loop 自己 check**（方案 A），不引入独立 watcher task。agent_loop 每 cycle 开头 pop fired，sleep 间隔改为 `min(poll_interval, time_until_next_due)`。
5. **CLI 风格**：`gitim timer set <duration> <anchor> [--note <text>]` / `timer list` / `timer cancel <id_or_prefix>`。humantime 格式。anchor positional 必填。
6. **Cap**：每 agent 最多 3 个 pending timer（用户定）。

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  agent_clone/                                                    │
│  ├── .gitim/                                                     │
│  │   ├── me.json          ← self-handler / config (existing)     │
│  │   ├── agent-state.json ← cursor/session_usage (existing)      │
│  │   └── timers.json      ← NEW: pending timers (gitignored)     │
│  └── ...                                                         │
└──────────────────────────────────────────────────────────────────┘
        ▲                                       ▲
        │ fs read/write (flock)                 │ fs read/write (flock)
        │                                       │
┌───────┴─────────────┐              ┌──────────┴────────────────┐
│ gitim CLI           │              │ gitim-runtime             │
│ (per invocation)    │              │ (long-running)            │
│                     │              │                           │
│ `gitim timer set`   │              │ agent_loop tokio task:    │
│ `gitim timer list`  │              │   loop {                  │
│ `gitim timer cancel`│              │     pop fired timers      │
│                     │              │     poll daemon           │
│ Direct fs read/wr,  │              │     if fired || changes:  │
│ no IPC, no HTTP     │              │       run_once(prefix+chg)│
└─────────────────────┘              │     sleep(min(poll, due)) │
                                     │   }                       │
                                     └───────────────────────────┘
```

每个 agent_loop tokio task **只读自己 agent 的 timers.json**——agent_loop 已持有 `agent_clone_path`，无需中央索引、无需共享状态。

**关键原则**：
- 零新 IPC / HTTP / 协议（CLI 与 agent_loop 通过 fs 通讯，唯一约定是 JSON schema）
- 零新 tokio task（复用 agent_loop sleep-poll loop）
- per-agent 隔离天然（每个 clone 自己的文件）
- daemon 完全不参与
- 故障域天然 per-agent（agent_loop 死了只影响自己的 timer）

## File Schema

`<agent_clone>/.gitim/timers.json`：

```json
{
  "version": 1,
  "timers": [
    {
      "id": "20260520T143055Z-a3f4c2",
      "fire_at": "2026-05-20T15:00:55Z",
      "created_at": "2026-05-20T14:30:55Z",
      "anchor": "<#product:L000042>",
      "note": "check deploy status"
    }
  ]
}
```

| Field | 类型 | 说明 |
|---|---|---|
| `version` | u32 | schema 版本，v1 固定 1 |
| `timers` | Vec | pending 列表 |
| `id` | String | `YYYYMMDDTHHMMSSZ-XXXXXX`（UTC 时间戳 + 6 字符随机后缀），跟 flow run_id 同 pattern |
| `fire_at` | RFC3339 UTC | 到期绝对时刻 |
| `created_at` | RFC3339 UTC | 注册时刻（prompt 里展示"X 分钟前注册"） |
| `anchor` | String | 必填，自由字符串；建议 `<#channel:L42>` / DM 路径 / 卡片路径；不强 validate |
| `note` | Option\<String\> | 可选 humans-readable 备忘 |

- **文件不存在 = 0 个 timer**（不要求双方创建空文件）
- **原子写**：`atomic_write_json` 风格（tmp + rename）
- **并发**：CLI 和 agent_loop 都用 `fs2` advisory file lock（flock），串行化 read-modify-write
- **Migration**：新增字段走 `#[serde(default)]`；breaking 变 bump version，agent_loop 看到不认识的 version 视作 0 timer + log warning

## CLI Surface

新子命令组 `gitim timer ...`，挂在 [crates/gitim-cli/](crates/gitim-cli/) 现有 `gitim` CLI 下。

### `gitim timer set <duration> <anchor> [--note <text>]`

```
$ gitim timer set 30m '<#product:L000042>' --note "check deploy status"
20260520T143055Z-a3f4c2  fires in 30m  (at 2026-05-20T15:00:55Z)
```

- `<duration>`：humantime（`30s` / `5m` / `2h` / `1h30m`），范围 10s ~ 24h
- `<anchor>`：positional 必填，任意字符串（trim 后非空）
- `--note <text>`：可选
- 注册前 check cap：当前 pending ≥ 3 → 拒绝
- 成功 exit 0 打印 `<id>  fires in <humantime>  (at <RFC3339>)`

### `gitim timer list [--json]`

```
$ gitim timer list
ID                          DUE IN    FIRES AT              ANCHOR                   NOTE
20260520T143055Z-a3f4c2     29m12s    2026-05-20T15:00:55Z  <#product:L000042>       check deploy status
20260520T143120Z-b7e811     2h12m     2026-05-20T16:43:20Z  <#general:L000007>       follow up with alice
```

按 `fire_at` 升序；空 → `(no pending timers)`；`--json` 输出 raw JSON。

### `gitim timer cancel <id_or_prefix>`

```
$ gitim timer cancel a3f4c2
cancelled: 20260520T143055Z-a3f4c2
```

完整 id 或唯一 prefix；0 / N 匹配 → exit 2。

### Common 行为

- **CWD 推导**：从 `cwd` 向上找 `.gitim/` 目录；找不到 → exit 2
- **Self-handler 验证**：读 `.gitim/me.json`（防误操作其他 clone 但 v1 不在 entry 里冗余 handler）
- **不打开 daemon connection**——纯 fs，daemon 没跑也能用
- **Exit codes**：0 success；2 user error（参数 / cap / not found）；1 IO 错误。跟 gitim CLI 其他子命令一致。

### Non-goals（v1）

- 修改已有 timer（cancel + re-set 即可）
- recurring / cron（用户明说 oneshot）
- 跨 agent 看 / 改别人 timer（CLI 本来只能在 cwd 跑）
- `gitim timer fire <id>` 手动触发（debug 工具，留 v2）

## agent_loop Integration

改造 [crates/gitim-runtime/src/agent_loop.rs](crates/gitim-runtime/src/agent_loop.rs) 主 loop。

### Cycle body 改造（pseudo）

```rust
loop {
    let fired = pop_fired_timers(&agent_clone_path).await;     // NEW
    let daemon_changes = poller.poll().await?;

    let should_run = !fired.is_empty() || has_pending_changes(&daemon_changes);
    if should_run {
        let prefix = format_fired_timers_for_prompt(&fired);    // NEW
        agent_loop.run_once_with_prefix(prefix, daemon_changes).await?;
    }

    let next_due = peek_next_due(&agent_clone_path).await;      // NEW
    let sleep_dur = match next_due {
        Some(t) => min(poll_interval, (t - Utc::now()).max(Duration::from_secs(1))),
        None => poll_interval,
    };
    tokio::time::sleep(sleep_dur).await;
}
```

### `pop_fired_timers` 流程

1. flock 文件
2. read → JSON parse（不存在 / 损坏 → 返 [] + log warning，不删文件）
3. partition：`fire_at <= now` vs `>`
4. 把 `>` 那批 atomic write 回去
5. 释放 flock
6. 返回 `<=` 那批
7. **write 失败 → 保留 fired 在文件里（下 cycle retry）+ 本 cycle 不注入 prompt**（避免触发但没清→重复触发）

### Synthetic prompt 格式

多个同时到期合并展示：

```text
## ⏰ Timer reminder(s) fired

1. Set 30m ago
   anchor: <#product:L000042>
   note: check deploy status

2. Set 1h12m ago
   anchor: <#general:L000007>
   note: follow up with alice

Use the `gitim` CLI to fetch context at the anchor(s) above.
```

**Prepend** 到本 cycle 的 daemon-changes prompt 前。`format_changes_as_prompt` 不变，只是入口多一段 prefix。

### 跨 runtime 重启行为

runtime 起来 / agent_loop 重 spawn → 第一次 cycle pop 所有 `fire_at <= now` 的，全部一次性 fire。**这是想要的行为**：用户离线 1 小时回来，错过的 reminder 全部出现，比沉默好。

### Hot path performance

每 cycle 多 2 次 file read（pop + peek）+ 偶尔 1 次 write，文件 < 1KB，flock < 1ms。可忽略。

## Agent Discovery（system prompt）

在 [crates/gitim-agent-provider/src/prompts.rs](crates/gitim-agent-provider/src/prompts.rs) 的 `default_gitim_api()` 末尾追加：

```markdown
## 一次性定时提醒（timer）

你是 oneshot 运行的——一旦本轮响应结束，你的进程就退出了。如果你判断"这件事要过一段时间再回来看看"
（比如等一个 deploy 完成、等对方回复一段时间、给自己一个 cool-down 后复盘），普通的"30 分钟后我再
看一下"在你身上不会自动发生——没人会在 30 分钟后唤醒你。

`gitim timer` 解决这个问题。注册之后到点，runtime 会重新唤起你一次，并把"为什么唤醒、当初锚点
在哪里"塞进你看到的消息流，让你能继续之前的线索。

注册：
  gitim timer set <duration> <anchor> [--note <text>]
  例：gitim timer set 30m '<#deploys:L000128>' --note "看 prod 是否绿了"

  duration:  humantime，如 45s / 5m / 1h30m
  anchor:    指向"当时这个 timer 是为哪条消息/卡片设的"——醒来后你顺着它 gitim read 回到
             现场。建议格式 `<#channel:L行号>`、DM 路径、卡片路径。
  note:      给未来的自己一句话提醒，可选。

查看 / 撤销：
  gitim timer list
  gitim timer cancel <id 或 id 前缀>
```

**风格依据**：[feedback_prompt_style_for_llms.md](memory)——用 AI 第一性原理 + 交接语气讲清"为什么 + 怎么用"，不预先 over-explain 硬约束（cap 数字、精度）和具体格式（被唤醒时长啥样）；agent 在试错和被唤醒时自然学到。

## Error Handling

| 场景 | 行为 |
|---|---|
| CLI: duration 解析失败 | `error: invalid duration "30 minutes": expected humantime like "30m"`，exit 2 |
| CLI: duration < 10s 或 > 24h | `error: duration must be 10s..24h`，exit 2 |
| CLI: anchor 空 / 仅空白 | `error: anchor cannot be empty`，exit 2 |
| CLI: cap 已满 | `error: 3 pending timers already, cancel one first (gitim timer list)`，exit 2 |
| CLI: cancel id 0 匹配 | `error: no timer matches "<arg>"`，exit 2 |
| CLI: cancel id 多匹配 | `error: prefix "<arg>" matches <N> timers: <id1>, <id2>, ...`，exit 2 |
| CLI: cwd 不在 agent clone | `error: not in a gitim agent clone (no .gitim/ directory)`，exit 2 |
| CLI: 写文件 IO 失败 | `error: failed to write timers.json: <io::Error>`，exit 1 |
| CLI: flock 拿不到 | 阻塞等到拿到（< 10ms 典型），不 timeout |
| agent_loop: 文件不存在 | 视作 0 timer，不 log（正常状态） |
| agent_loop: JSON parse fail | log warning `timers.json corrupted at <path>: <err>`，视作 0 timer，**不删文件** |
| agent_loop: version 不识别 | log warning `unknown timers.json version <N>, expected 1`，视作 0 timer |
| agent_loop: write 失败 | log error，保留 fired 在文件里（下 cycle retry），本 cycle **不**注入 prompt |
| agent_loop: flock acquire 失败 | log，本 cycle 跳过 timer check（poll 仍走），下 cycle 再试 |
| Clock skew（时间回拨） | fire_at 是绝对时刻；回拨 → timer 延后到新"未来"才 fire。可接受 |
| Runtime 重启 / agent_loop 死亡 | timer 留文件里；重 spawn → 第一次 cycle 一次性 fire 所有过期的（"补漏"语义） |
| Agent hard delete | `hard_delete_agent_dir` 删整个 clone，timers.json 跟着删 |
| Agent soft delete | clone 保留，timers.json 保留，但 agent_loop 不再 spawn → timer 永不 fire。可接受（soft delete 本就是冻结状态） |

**Logging**：跟现有 agent_loop 一致，用 `tracing`；warning/error 入 daemon log（`~/.gitim/logs/<workspace>-<handler>.log`）。

**Atomicity**：所有文件写走 `atomic_write_json`（tmp + rename）；CLI 和 agent_loop 都拿 flock，串行化 read-modify-write，无 lost update。

**Fail-open 哲学**：timer 系统损坏时优先**不阻塞 agent_loop**——读不到当 0 timer，agent 继续干活；warning 入 log 等人介入。跟 GitIM 已有的 fail-open 风格一致（sync_loop auth 熔断、index disabled 等）。

## Testing Strategy

按 GitIM 惯例（unit 内联 `#[cfg(test)]`，integration 在 `tests/`）+ TDD（writing-plans 强制每个 task 先写 test）。

### Unit tests

**`gitim-core/src/timer.rs`**（新模块，types + parse + format + partition）：
- `parse_duration` happy path（`30m` / `1h30m` / `45s`）
- `parse_duration` 拒 < 10s 和 > 24h
- `parse_duration` 拒非法格式
- `Timer::new` 生成的 id 符合 `YYYYMMDDTHHMMSSZ-XXXXXX` regex
- `Timer::new` 的 `fire_at == created_at + duration`
- `TimersFile::partition_fired(now)` 正确分 due / pending
- `TimersFile` serde round-trip
- `TimersFile` 反序列化 unknown version → empty + warning（不 panic）
- `cancel_by_id_or_prefix` 0 / 1 / N 匹配

**`gitim-cli/src/timer.rs`**（CLI 命令实现）：
- `assert_cmd` spawn 子进程跑（pattern 参考 `gitim-runtime` 的 `cli_status` test）
- 每个 CLI 子命令的 happy path + 各 exit-2 error case
- 用 tempdir + 假 `.gitim/me.json` 模拟 agent clone

### Integration tests

**`crates/gitim-runtime/tests/timer_integration.rs`**（新）：

| Case | 设置 | 验证 |
|---|---|---|
| 单 timer 到期触发 prompt | 写 `fire_at = now - 1s` | run_once 收到 `## ⏰ Timer reminder` prefix；entry 被移除 |
| 多 timer 同时到期 | 写 2 个过期 | prefix 含 2 个 entry，编号 1/2 |
| Future timer 不触发 | 写 `fire_at = now + 1h` | 不进 prompt；文件保留 |
| 文件损坏不阻塞 | 写非法 JSON | agent_loop 继续 poll，log 有 warning，**不删文件** |
| 文件不存在 | 无 timers.json | agent_loop 正常跑，无 warning |
| 跨 runtime 重启补漏 | set 会过期的 → stop runtime → 等过期 → 重启 | 第一次 cycle 一次性 fire 所有过期的 |
| sleep 缩短到 next due | poll_interval = 60s，set 5s 后到期 | agent_loop 在 ~5-6s 内醒来 |

**`crates/gitim-cli/tests/timer_cli.rs`**（端到端）：

| Case | 步骤 | 验证 |
|---|---|---|
| set → list → cancel | tempdir + 假 me.json | 每步 stdout/exit + 最终文件状态 |
| cap 触发 | set 3 → 第 4 次 | exit 2 + 错误信息 |
| cancel 全 id | set + cancel `<full-id>` | 文件清空 |
| cancel 唯一 prefix | set + cancel `a3f` | 匹配且 cancel |
| cancel 多匹配 | set 2 → cancel `2026` | exit 2 + 列候选 |
| 并发 race（flock） | spawn N CLI 同时 set / loop pop | 无 lost update |

### 不测

- HTTP / daemon IPC（v1 不走）
- 跨 agent / 跨 workspace 干扰（架构 per-agent 隔离）
- LLM 实际理解 prompt（手工 dogfood）

### Coverage 目标

- `gitim-core::timer` ≥ 90%
- agent_loop 改动行 100% 被 integration test 覆盖
- CLI 子命令每个 exit code 都有 test

## Affected Files / Crates

- **NEW**: `crates/gitim-core/src/timer.rs` — Timer / TimersFile types, parse, partition, atomic IO + flock helper
- **NEW**: `crates/gitim-cli/src/timer.rs` + clap wiring — `set` / `list` / `cancel` 子命令
- **EDIT**: `crates/gitim-runtime/src/agent_loop.rs` — pop_fired_timers + sleep 改造 + prompt prefix 拼接
- **EDIT**: `crates/gitim-agent-provider/src/prompts.rs` — `default_gitim_api()` 追加 timer 段
- **EDIT**: `crates/gitim-cli/Cargo.toml` / `crates/gitim-core/Cargo.toml` — 加 `humantime` + `fs2`（如未引入）
- **NEW**: `crates/gitim-runtime/tests/timer_integration.rs`
- **NEW**: `crates/gitim-cli/tests/timer_cli.rs`
- **EDIT**: `CLAUDE.md` — Current Orientation 段更新（"Where we are" 加 timer 落地说明）

## Non-goals（v1）

- Recurring / cron-like timer（明确 oneshot）
- 修改已有 timer（cancel + re-set 即可）
- 跨 agent / 跨 workspace 看或改别人 timer
- `gitim timer fire <id>` 手动触发（debug 工具，v2）
- Timer fired 后写入 git audit trail（用户明示不进 git）
- WebUI 显示 timer 状态（v1 纯 CLI，v2 看需求）
- Per-agent timer 配额可调（v1 硬编码 3）
- Anchor 格式 validation（v1 自由字符串）
- 跨 daemon / runtime restart 的 fire 时刻精确补偿（v1 按"一次性补 fire"，不还原历史时序）

## Open Questions

无。所有设计决策已 grill 收敛。
