# 04 — CLI:DM archive + burn-self + 客户端方法

> 对应 [01-plan.md](01-plan.md) Part C。5 个新命令 + 对应 client.rs 方法。3 个 task。

## C.1 — client.rs 方法

**文件**:[crates/gitim-client/src/client.rs](../../../crates/gitim-client/src/client.rs)

**新加方法**(对称现有 archiveChannel / unarchiveChannel):
- `archive_dm(peer: &str) -> Result<ApiResponse>`
- `unarchive_dm(peer: &str) -> Result<ApiResponse>`
- `list_archived_dms() -> Result<ApiResponse>`
- `list_archived_users() -> Result<ApiResponse>`
- `depart_self() -> Result<ApiResponse>`(发送 `depart_user { handler: <self> }`,handler 由 client 端从 me.json 读取后填入)

**关键边界**:
- depart_self 只能传 client 自己 me.json 的 handler,**禁止**传任意 handler 参数(防 cross-burn)
- daemon 收到 depart_user 是受信任的(daemon 内部调度),但 CLI 路径只暴露 self

**验收**:
- 每个方法发送正确 JSON 到 daemon socket
- depart_self 不接受 handler 参数,handler 从 me.json 读

**依赖**:A.1(API surface)

---

## C.2 — gitim archive-dm / unarchive-dm / list-archived-dms / list-archived-users

**文件**:
- [crates/gitim-cli/src/main.rs](../../../crates/gitim-cli/src/main.rs) — Commands enum +4 variant + dispatch
- [crates/gitim-cli/src/commands/dm.rs](../../../crates/gitim-cli/src/commands/dm.rs)(新建)— 4 cmd 函数

**新增命令**(对称参考 archive-channel / archived-channels):
- `gitim archive-dm <peer>` → `client.archive_dm(peer)`
- `gitim unarchive-dm <peer>` → `client.unarchive_dm(peer)`
- `gitim list-archived-dms` → `client.list_archived_dms()`
- `gitim list-archived-users` → `client.list_archived_users()`

**输出**(human 模式):
- archive-dm 成功:`已归档与 @<peer> 的私信`
- unarchive-dm 成功:`已恢复与 @<peer> 的私信`
- list 命令:逐行输出 `<peer>` 或 `<handler>`
- 错误:打印 daemon 原始错误信息,exit 1

**验收**:
- 4 个命令 `--help` 正常显示
- happy path:每个命令都正确触达 daemon
- daemon 拒绝(已归档/不存在等)→ exit 1 + 错误打印

**依赖**:C.1

---

## C.3 — gitim burn-self

**文件**:
- [crates/gitim-cli/src/main.rs](../../../crates/gitim-cli/src/main.rs) — Commands::BurnSelf variant + dispatch
- [crates/gitim-cli/src/commands/burn.rs](../../../crates/gitim-cli/src/commands/burn.rs)(新建)

**命令**:`gitim burn-self`
- **不接受任何参数**(包括 --handler / --confirm)— 强制只能 self,杜绝误用 / cross-burn
- dispatch:从本地 .gitim/me.json 读 handler → 调 `client.depart_self()`

**输出**:
- 成功:`已退出 workspace。本 agent 的 user 档案与所有 DM 已归档,clone 目录将由 runtime 清理。`
- 失败:打印 daemon 错误,exit 1

**关键约束**:
- 不要在 CLI 层加确认 prompt(LLM 走 CLI,prompt 没意义)
- 不暴露 archive-user / unarchive-user(P0.1 决策:agent 不能 burn 别人,人也不通过 CLI burn 别人,只 WebUI 入口)
- daemon 收到 depart_user 后会自己 kill 当前 daemon process(因为 burn-self 调用方就是 alice 的 agent daemon,被 kill 了 — 这是预期路径,exit 1 错误码 OK,因为 process 也退出了)

**与 WebUI burn 的关系**:
- WebUI 走 runtime POST /agents/burn(B.1),runtime 编排 abort loop / kill daemon / 调 depart_user / rm clone
- burn-self 直接走 daemon depart_user,不经 runtime
- runtime cleanup 由 **B.4(self-departed 自愈)**接管:agent's daemon 在下次 poll 入口 stat archive/users/<self>.meta.yaml,存在 → return `self_departed` error code → runtime agent_loop 识别后自动触发 cleanup(rm clone / 删 hermes profile / 从 ctx.agents 移除 / SSE 通知)
- 两路 cleanup 结果 idempotent 一致

**验收**:
- burn-self 调用 daemon 成功执行 depart_user
- 不接受任何参数(`gitim burn-self --foo` → exit with usage error)
- daemon 完成 depart 后 process 自然 die(process 是 agent 自己的 daemon,被 daemon-internal kill / abort)

**依赖**:C.1

---

## 测试约定

跟 [leave-channel](../2026-04-23-leave-channel/01-plan.md) 一致 — **不**新增 CLI 集成测试,靠 daemon handler 测试(A.8) + runtime burn endpoint 测试(B.3) + 手工 smoke 兜底。

---

## 开放问题

**event token 命名**:`leave-workspace` vs `depart` vs 别 — 实施 A.4 时定,只在 daemon / prompt / spec 文档里出现一处约定,改起来零成本

---

## 整体依赖

C.1 → C.2 / C.3(并行)。C.2 / C.3 没有 inter-dep。
