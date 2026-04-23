# leave-channel:CLI 暴露 + Agent prompt 告知

## 背景

`leave_channel` RPC 在 daemon / client 层已完整实现:
- [crates/gitim-daemon/src/handlers/channel.rs](../../../crates/gitim-daemon/src/handlers/channel.rs) `handle_leave_channel` → `write_channel_event(... "leave")` 会写 event 到 `.thread` **并** 把 handler 从 `meta.yaml.members` 移除。
- [crates/gitim-daemon/src/handlers/poll.rs](../../../crates/gitim-daemon/src/handlers/poll.rs) `handle_poll` 读 `meta.members` 过滤事件 → leave 之后 agent 物理上不再收到该 channel 的事件。
- [crates/gitim-client/src/client.rs](../../../crates/gitim-client/src/client.rs:177) `leave_channel(channel, targets)` 已暴露。
- `im_rules::validate_leave` 已做合规检查(非成员不能 leave 等),CLI 不需前置校验。

但 CLI 和 agent prompt **没有** `leave-channel` 入口,agent 无法用。

## 目标

让 agent 能通过 `gitim leave-channel <name>` 主动退出某频道,作为**精细化 context 净化手段**,与现有 `[[RESET]]` 全局重置协议形成粒度互补。

**非目标**:
- 人类 UI(人类是上帝视角,退出群聊无意义)
- 踢别人(`leave-channel` 的 `targets` 参数不暴露给 CLI,只做"自己离开")
- Runtime HTTP endpoint
- daemon / client 层改动

## 分工

### Part A — CLI 接入

**文件**:
- [crates/gitim-cli/src/main.rs](../../../crates/gitim-cli/src/main.rs) — `Commands` enum 加 `LeaveChannel { channel: String }` 分支,并在 dispatcher 里调用
- [crates/gitim-cli/src/commands/channels.rs](../../../crates/gitim-cli/src/commands/channels.rs) — 加 `cmd_leave_channel`,调 `client.leave_channel(channel, &[])` (targets 永远传空)

**对称参考**:`cmd_join_channel` / `JoinChannel`。差异只有:
- CLI 层不接受 `-t targets` 参数(锁死为空数组)
- Human 模式输出:`已退出 #{channel}` / 错误走 daemon 返回的原始错误信息
- 错误 前缀改为 `退出失败:`

**验收**:
- `gitim leave-channel foo` 调用 daemon,把自己从 `foo.meta.yaml.members` 移除并写 leave event
- `gitim leave-channel --help` 显示命令
- daemon 拒绝(例如非成员、channel 不存在、channel 已归档)时 CLI 退出码 1 并打印 daemon 错误信息

### Part B — Agent prompt

**文件**:[crates/gitim-agent-provider/src/prompts.rs](../../../crates/gitim-agent-provider/src/prompts.rs)

**改动 1 — `default_gitim_api` 的"频道"小节(prompts.rs:311-319 附近)**:
在 `join-channel` 之后加一条 `leave-channel <name>` 说明,一句话解释"退出后不再在该频道收到事件"。

**改动 2 — `default_reset_protocol`(prompts.rs:229-261)**:
插入一段新内容(不开新函数,沿用 Q3 (b) 决议),说明:
- `[[RESET]]` 是 **session 级** 重置(重锤 — 记忆文件以外全部清空)
- `gitim leave-channel <name>` 是 **订阅级** 净化(手术刀 — 只切断某条无关信号,其他 channel / 记忆 / 本次 session 全保留)
- 识别判断:无关事件集中在**某个具体 channel** 用 leave;session 整体偏题、跨多个 channel 的噪声混杂用 `[[RESET]]`
- leave 是面向网络的公开行为(会写 event、改 meta、触发 sync)— 不是躲避争议的手段。该 channel 讨论没你份、或你明确不再负责该工作线时才 leave

文案风格沿用 prompts.rs 现有的 AI 第一性原理 + 交接语气(参考 memory `feedback_prompt_style_for_llms.md`)。

### Part C — 测试

`gitim-cli` 没有独立 `tests/` 目录,也没有对 `cmd_*` 的单元测试 — 现有 thin wrapper 靠 daemon handler 测试 + 手工 smoke 兜底。

**新增**:
- daemon 侧已有 `test_leave_channel_self`(handlers_test.rs:191),无需补
- prompt 侧:`gitim-agent-provider` 现有测试如果有对 prompt 文本的 assertion,同步更新;如果没有 assertion,不新增 — prompt 是人读的,过度 assert 反而锁死文案迭代
- CLI 侧:**不**新增集成测试(和 join/archive/unarchive 保持一致,避免为单条 subcommand 开 tests 目录)

### Part D — 文档(可选,低优先)

如果 `docs/specs/cli.md` 有 join-channel 的条目就同步加一条 leave-channel;没有则跳过。(Phase 5 实施时现场判断)

## 风险与回滚

- **风险 1**:agent 误 leave 关键 channel → 靠 join event 邀请回来即可,meta.members 会再加回,物理可逆
- **风险 2**:prompt 文案把 leave 说成"逃避工具" → Part B 文案里明确反用法边界("不是躲避争议的手段")
- **风险 3**:CLI 多一个命令 vs. 和 daemon API 完全对称 — 是后续加 `--targets` 的铺垫,不是债

## 合并前校验清单

1. `cargo build -p gitim-cli` 通过
2. `cargo test -p gitim-cli -p gitim-agent-provider` 通过(scoped;不跑全量,依据 CLAUDE.md 测试节奏)
3. `cargo fmt --check` 通过
4. `cargo clippy -p gitim-cli -p gitim-agent-provider -- -D warnings` 通过
5. 任务末尾跑一次 `cargo test` 全量确认无 regression
6. 手工 smoke:`gitim leave-channel --help` 能显示;在本地仓库试跑一次 leave + 看 meta.yaml 变化(可选,时间允许的话)

## 文件清单(预期 diff 范围)

```
crates/gitim-cli/src/main.rs              — 加 LeaveChannel variant + dispatch
crates/gitim-cli/src/commands/channels.rs — 加 cmd_leave_channel
crates/gitim-agent-provider/src/prompts.rs — 改 default_gitim_api + default_reset_protocol
docs/plans/2026-04-23-leave-channel/01-plan.md — 本文档
```

约 4 个文件,估计 ~80 行新增(含 prompt 文案)。
