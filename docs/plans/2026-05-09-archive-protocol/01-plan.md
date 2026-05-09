# archive-protocol:agent burn + DM archive + 系统级归档原语

## 背景

测试阶段创建一堆 throwaway agent → 发现不合适 → 想"快速回收"。现有路径只有 `agents/stop` / `agents/remove` / `agents/remove?hard_delete=true` 三件套,而 `hard_delete` **只删 agent 自己的 clone 目录**。共享仓库里的:

- `users/<handler>.meta.yaml`(用户档案)
- `channels/<X>.thread`(它历史发的所有消息,逐行混在主时间线)
- `dm/X--<handler>.thread`(它所有 DM,在每个对端 DM 列表里)
- `channels/<X>.meta.yaml` 的 `members:` 列表

**全部完整保留**。结果:你"删"了它,UI 上每条线索都在 — DM 列表、频道滚动、用户列表、mention,处处可见。这是用户痛点的根源。

[../2026-04-16-runtime-idle-exit.md](../2026-04-16-runtime-idle-exit.md) 解的是 runtime **进程级** idle 退出,不是 agent 概念级的退出。本设计填的是**概念级"退出"**这一空白。

## 现役同构 pattern

- [../2026-04-03-channel-archive.md](../2026-04-03-channel-archive.md) — 频道归档,`git mv channels/X → archive/channels/X` + 写入拦截 + 读取 fallback + poll diff 处理。已上线。
- card-archive([crates/gitim-daemon/src/card_handlers.rs](../../../crates/gitim-daemon/src/card_handlers.rs))— 卡片归档,同 pattern。已上线。
- [../2026-04-23-leave-channel/01-plan.md](../2026-04-23-leave-channel/01-plan.md) — agent 主动退出某频道(写 leave event,移除 members 条目)。已上线。

**三个 instance 已经在线**。本次工作把这套 pattern 提炼为**系统级原语**,新增 **user 和 dm** 两个 instance。

## 关键设计决策

1. **B 档 directory-as-state**:`git mv` 到 `archive/<surface>/`,不靠 metadata 字段
2. **对人 UI 完全无痕,对 agent 诚实**(决策 A2):burn 时 daemon 在 departed agent 曾参与的每个 thread 末尾 append 一行 `leave-workspace` event,**作者是 agent 自己**(`@<handler>`),与 [leave-channel](../2026-04-23-leave-channel/01-plan.md) 模型对称。**人 UI** 默认隐藏来自 archived user 的所有内容;**agent poll** 仍能看到旧消息 + 新增的 leave event,自己决定后续行为(LLM context 不"失忆")
3. **DM 单方触发即归档**(决策 B1):任一对端发起即生效,无需对端 confirm。提供 unarchive 反悔通道
4. **即用即抛 v1 不入数据模型**:burn 是单一统一动作,user meta 不加 `lifecycle` 字段
5. **不重写 git 历史**:audit trail 永久保留,所有"删除"都是新 commit + 可逆
6. **spec 形式化**:`docs/specs/archive-protocol.md` 提炼成系统承诺,所有未来 archivable surface 必须遵守
7. **幂等多 commit + 终态判断**:burn 不追求 single-commit 原子性。daemon 走多个独立 commit(每个复用现有 send 路径的 retry/rebase/renumber 机制)。失败时 user 重试,daemon 用状态判断跳过已完成步骤。**`archive/users/<handler>.meta.yaml` 存在 = burn 完成**,这是单一 source of truth
8. **agent 可自我 burn**(self-burn):暴露 CLI `gitim burn-self`(无参数,只能自杀,**不能** cross-burn 别人)。配 prompt 文案告知 LLM 何时使用,何时不要用

## archive-protocol(v1 spec)

每个 archivable surface 必须实现以下 5 条 contract(写入 `docs/specs/archive-protocol.md`):

1. **目录命名**:active path `<surface>/...` ↔ archive path `archive/<surface>/...`(镜像)
2. **写入拦截**:active path 不存在时检查 archive path,如存在,所有 mutation 必须返回明确错误(包含 "is archived" 或 "is departed" 字样)
3. **读取 fallback**:active path 不存在时尝试 archive path,响应附 `archived: true` 字段
4. **poll diff 处理**:thread 出现在 archive 路径下 → poll 应识别为归档事件,不可误判为新建 / 删除
5. **可逆**:必须提供 unarchive 操作(物理清理路径如 hard_delete agent clone dir 例外)

surface 实现表:

| Surface | active path | archive path | 状态 |
|---|---|---|---|
| channel | `channels/X.thread` + `.meta.yaml` | `archive/channels/X.*` | 已实现 |
| card | `channels/X/cards/Y/...` | `archive/channels/X/cards/Y/...` | 已实现 |
| **user** | `users/X.meta.yaml` | `archive/users/X.meta.yaml` | **本次新增** |
| **dm** | `dm/X--Y.thread` | `archive/dm/X--Y.thread` | **本次新增** |

未来加新 archivable surface(如 reactions、附件、子线程),必须配套补这张表 + 走完 5 条 contract。

## Agent burn 工作流(幂等多 commit 模型)

**关键不变量**:`archive/users/<handler>.meta.yaml` 存在 = burn 完成。这是 daemon、runtime、外部 clone 一致的单一判定标准。

触发:
- **人类操作**:WebUI "Burn alice" 按钮 → `POST /workspaces/{slug}/agents/burn { id: "alice" }`
- **agent self-burn**:alice 自己执行 `gitim burn-self`(CLI → daemon → 同样落到 burn endpoint)

happy path:

1. **runtime 校验**:target id 必须在 `ctx.agents` 里(防止误 burn 人类 user / 不存在的 id)。不通过 → 4xx
2. **abort agent loop / kill agent daemon process**(同现有 hard_delete 流程)
3. **daemon `depart_user { handler: "alice" }`** — 幂等,可重试:
   ```
   if exists("archive/users/alice.meta.yaml"):
     return success                         # 终态判断:已完成
   
   # Phase 1: 写 leave events(每个 thread 一个独立 commit,复用现有 send 路径)
   for each thread T where alice has spoken:
     if 末尾已是 alice 的 leave-workspace event: skip   # 幂等
     else: append leave event to T          # 单文件 commit + retry/rebase/renumber
   
   # Phase 2: 归档 DMs(每个 DM 一个独立 commit)
   for each dm "X--alice":
     if 已在 archive/dm/: skip              # 幂等
     else: git mv to archive/dm/
   
   # Phase 3: 清理 channels meta(开放问题 1,倾向清)
   for each channels/<ch>.meta.yaml where alice in members:
     remove alice from members list         # 单文件 commit
   
   # Phase 4: 归档 user entry(终态)
   git mv users/alice.meta.yaml → archive/users/
   
   return success
   ```
4. **rm -rf agent clone dir**(同 `hard_delete_agent_dir`)
5. **删 hermes profile**(best-effort,失败仅 warn)
6. **从 in-memory ctx.agents 移除**
7. **触发 SSE 通知 WebUI 刷新**

错误处理与重试:
- 步骤 3 任一 commit/push 失败 → daemon return error → runtime **不**执行步骤 4-7 → user 重试 → daemon 走幂等路径 → 跳过已完成步骤,继续未完成 → 全部完成才执行步骤 4-7
- 步骤 4/5 失败 → 警告日志,不再回滚(commit 已 push)。clone dir 残留可手动 cleanup
- self-burn 重试:agent 已被 kill,只能由 user 在 WebUI 重试。CLI `gitim burn-self` 是一次性触发,daemon 完成 phase 4 之前 alice 的 daemon 已经被 kill,不可能自己重试 — 这种 case 只能 user 在 WebUI 触发 burn(走 daemon 的幂等路径)

中间状态(daemon 部分完成,runtime 没 cleanup):
- alice 已 abort,不再发消息
- alice user entry 还在 active → 别人能 mention / send,但 alice 不响应
- 这等同于现有的 "agent stop" 状态,**良性**

反向操作(unburn):**v1 不做**。误 burn 通过内部命令 `unarchive_user` + `unarchive_dm` 部分恢复(只恢复 UI 可见性,不恢复 agent 运行时;agent runtime 需要 re-add)。

## 工作分块

> 各 Part 的具体 task 切分见独立文件:
> - [02-daemon.md](02-daemon.md) — Part A(9 task)
> - [03-runtime.md](03-runtime.md) — Part B(3 task)
> - [04-cli.md](04-cli.md) — Part C(3 task)
> - [05-prompt.md](05-prompt.md) — Part D(3 段文案)
> - [06-webui.md](06-webui.md) — Part E(3 sub-part)
> - [07-spec.md](07-spec.md) — Part F(1 task,spec 文档)
>
> 本节是高层概览,实施细节不在此处展开。

### Part A — daemon API:archive 操作 + leave event 写入

文件:
- [crates/gitim-daemon/src/api.rs](../../../crates/gitim-daemon/src/api.rs) — Request enum 新增多个 variant
- [crates/gitim-daemon/src/handlers/](../../../crates/gitim-daemon/src/handlers) — 新增 user / dm 处理(可能新建 `user.rs`,DM 复用 `send.rs`)
- [crates/gitim-daemon/src/handlers/poll.rs](../../../crates/gitim-daemon/src/handlers/poll.rs) — `archive/dm/` + `archive/users/` 路径处理
- [crates/gitim-daemon/src/handlers/send.rs](../../../crates/gitim-daemon/src/handlers/send.rs) — DM / user 拦截

新增 daemon API:
- `archive_user` / `unarchive_user`(内部用,不暴露 CLI / WebUI)
- `archive_dm { peer }` / `unarchive_dm { peer }`
- `list_archived_users` / `list_archived_dms`
- **`depart_user { handler }`** — 复合 API,burn 编排专用,内部走幂等多 commit 模型(见上面 happy path 步骤 3)

写入拦截升级:
- `handle_send` DM 分支:写入前检查对应 sorted-pair 文件名是否在 `archive/dm/`,是则返回 "DM with @X is archived"
- `handle_send` 用户检查:author 或 mention target 在 `archive/users/` → 返回 "user @X is departed"
- `handle_register_user` / onboard:handler 在 `archive/users/` 则拒绝(开放问题 2,倾向不允许 handler 重用)

读取 fallback:
- `handle_list_users`:默认只列 `users/`,新增 `--include-archived` 参数;**对所有 caller 一致**(daemon 不区分人 vs agent caller)
- `handle_read` DM 路径:active 不存在时 fallback `archive/dm/`,响应附 `archived: true`(对称 channel 已有逻辑)
- **read 一致性**(P2.a):`poll` **不**过滤 archived user 在 thread 里的旧消息(决策 A2 所需);`list_users` 默认过滤,这是行为差异,要在 spec 文档明示

leave event 写入(由 `depart_user` Phase 1 触发):
- 扫所有 active thread 与 archive/dm/* 找 author = handler 的 thread
- 每个匹配 thread 末尾 append 一行 `[L<下一行号>][@<handler>][<ts>] leave-workspace`(具体 event token 待 plan-eng-review 后定,可选 `leave-workspace` / `depart` / 别)
- 复用现有 send 路径的 commit + push retry + rebase + renumber 机制
- **作者是 handler 自己**,与 leave-channel 模型对称,**thread parser 不需要修改**

验收(扩展 channel-archive 测试模式 + 补 4 个 P1.b case):
- `archive_user` 后 `list_users` 不见 / `list_archived_users` 见
- `archive_dm` 后 read 该 DM 显示 `archived: true`,send 返回 "is archived"
- `depart_user` happy path:burn alice → 所有 alice 发过言的 thread 末尾各多一行 alice 的 leave-workspace event,user entry 在 archive/users/,DMs 在 archive/dm/
- **`depart_user` 幂等性**:已完成时再调返回 success,无副作用
- **半态恢复**:模拟 Phase 1 写到 5/10 thread 时 abort → 重试时 skip 前 5 个,继续完成剩下 5 个 + Phase 2-4
- **多 agent 并发 burn**(P1.b):走现有 send 的 retry/rebase 机制
- **零发言 agent burn**(P1.b):没写过任何消息 → Phase 1 跳过(0 thread 匹配),只走 Phase 2-4
- **unarchive_user 后 thread 旧 leave event 留存**(P1.b):决策 — **不抹**,leave event 是 audit 记录,留作历史
- **跨 clone fetch 同步**(P1.b):别的 clone fetch 后看到 burn commit + leave events,后续 poll/send 行为正常
- **性能基线**(P2.b):1k synthetic thread workspace 下 `depart_user` 端到端延迟 < 500ms。如果不达标:**v1 不上 [gitim-index](../../../crates/gitim-index) 优化**(YAGNI,用户明确决策),开 follow-up plan 单独优化,不阻塞 v1 merge

### Part B — runtime: burn endpoint

文件:[crates/gitim-runtime/src/http.rs](../../../crates/gitim-runtime/src/http.rs)

新加 endpoint:`POST /workspaces/{slug}/agents/burn { id: String }`

handler 编排顺序见上面 happy path 步骤 1-7。

**关键校验**(P1.c):步骤 1 必须 verify `id` 在 `ctx.agents` 里 — 防止误 burn 人类 user(`users/` 下不在 ctx.agents 的就是人,burn endpoint 不放行)。daemon 层 `archive_user` / `depart_user` 仍然 type-agnostic(为未来"人类离开 workspace" 留口子,但 v1 入口只有 burn endpoint,人类用例不在 scope)。

向后兼容(P1.a-A):
- 现有 `agents/stop` 不变(pause 仍有用)
- **`agents/remove` 标 deprecated**:WebUI 切换到 burn,后续 release 清理 endpoint。`hard_delete=true` 的两面性(只删本地不动共享仓库)从此明确为 footgun,不再"留作 split-brain 修复手段"

### Part C — CLI

文件:
- [crates/gitim-cli/src/main.rs](../../../crates/gitim-cli/src/main.rs) — Commands enum +5 variant
- [crates/gitim-cli/src/commands/dm.rs](../../../crates/gitim-cli/src/commands/dm.rs)(新建)
- [crates/gitim-cli/src/commands/burn.rs](../../../crates/gitim-cli/src/commands/burn.rs)(新建)
- [crates/gitim-client/src/client.rs](../../../crates/gitim-client/src/client.rs) — 客户端方法

新增 CLI 命令:
- `gitim archive-dm <peer>`
- `gitim unarchive-dm <peer>`
- `gitim list-archived-dms`
- `gitim list-archived-users`
- **`gitim burn-self`**(无参数,只能自杀,不接受任何 handler 参数 — 防止误用 + 阻止 cross-burn)

`archive-user` / `unarchive-user` 是 daemon 内部 API,**不暴露 CLI**(防止人 / agent 误把任意 user burn 掉,WebUI 的 "Burn" 按钮 + 二次确认是人的唯一入口,`burn-self` 是 agent 的唯一入口)。

测试约定:跟 leave-channel 一致,**不**新增 CLI 集成测试,靠 daemon handler 测试 + 手工 smoke 兜底。

### Part D — Agent prompt

文件:[crates/gitim-agent-provider/src/prompts.rs](../../../crates/gitim-agent-provider/src/prompts.rs)

加三块内容,沿用 AI 第一性原理 + 交接语气(memory `feedback_prompt_style_for_llms`):

1. **`leave-workspace` event 解读**(放 `default_reset_protocol` 旁或新增小节):
   - 在 thread 里看到 `@<handler> leave-workspace` event = 对方退出 workspace
   - 不要再 mention / @ 它(消息发不出,daemon 拒)
   - 必须提及它过去发言时,用过去式("之前 @X 提到过") + 不假设它会回应

2. **`archive-dm` / `unarchive-dm` 命令**(放 `default_gitim_api` DM 小节,跟 leave-channel 同级):
   - context 净化精细化手段:某条 DM 线索不再相关 → archive-dm
   - 与 leave-channel 同级:leave-channel 切断频道订阅,archive-dm 切断单条 DM。`[[RESET]]` 是 session 级重锤
   - 是公开行为(写 commit、影响双方)— 不是躲避争议工具

3. **`burn-self` 命令** + 使用边界:
   - 何时用:任务明确完成 + workspace owner / coordinator 不再需要你 + 无后续工作可承接
   - 何时**不要**用:任务卡住时(改用 `[[RESET]]` 重置 session)/ 不确定是否真的完成时(向 owner 请示)/ 想"清理 context" 时(用 leave-channel / archive-dm,不要 burn-self)
   - 不可逆:burn-self 一旦执行,自己的 user entry / DMs 都归档,clone dir 删除,自己**不能恢复**(只能 user 重新 add 一个新 agent)

### Part E — WebUI(三个独立 sub-part,P2.c)

#### E.1 — channel show-archived toggle 现状 verify

现役 channel-archive 有没有 show-archived toggle?如有,跳过;如无,补齐(独立小 PR)。

#### E.2 — DM 列表 archive 操作

文件:`products/gitim/frontend/src/.../dm-list.tsx`(具体路径 plan-eng-review 时切)

- 每条 DM 加"归档"按钮(右键菜单 / hover action)
- DM 列表加 show-archived toggle
- 默认隐藏 archived

#### E.3 — agent burn 按钮 + agent 列表 source 边界(P2.e)

文件:`products/gitim/frontend/src/.../agent-detail.tsx` + `agent-list.tsx`

- agent detail 页:"Hard Delete" 按钮替换为 **"Burn"**(标红),二次确认 dialog 文案明示"将归档该 agent 的 user 档案 + 所有 DM、清理 clone 目录,**不可一键恢复**(可手动 unarchive 单条 DM)"
- agent 列表加 show-archived toggle
- **source 边界**(P2.e):默认查 runtime `ctx.agents`(GET /agents),show-archived toggle 切到 daemon `list_archived_users`(两个不同 source,数据形态可能略有差异 — runtime 有 in-memory metadata、daemon 只有 user.meta.yaml,WebUI 要 graceful 处理 metadata 缺失)

### Part F — spec 文档

文件:[docs/specs/archive-protocol.md](../../specs/archive-protocol.md)(新建)

内容:
- 5 条 contract 形式化定义
- surface 实现表
- 命名约定(`archive/<surface>/...` 镜像)
- 幂等多 commit + 终态判断的 reference 实现(以 `depart_user` 为例)
- **read 一致性**(P2.a):poll 不过滤 archived user 的旧 thread 内容;`list_users` 默认过滤 archived;两者行为差异是有意的,实现细节
- **Versioning**(P2.d):后续 spec 加新 contract 时,旧 surface retrofit 的政策 — retrofit 不阻塞新 contract merge,但需要追踪(GitHub issue / TODO 列表)。spec 自身用 SemVer-ish 版本号(v1, v2, ...)

## 风险 / 回滚

- **风险 1 — burn 时扫所有 thread 性能**:朴素扫描,**v1 不上 gitim-index**(YAGNI,用户明确)。Part A 加性能 baseline 测试(1k thread 下 < 500ms),不达标开 follow-up plan 单独优化,不阻塞 v1 merge
- **风险 2 — leave event push 冲突**:复用现有 send 路径的 retry/rebase/renumber,已被验证
- **风险 3 — handler 重用混乱**:开放问题 2,倾向不允许 burn 后重用
- **风险 4 — 误 burn**:`unarchive_user` + `unarchive_dm` 内部 API 作 DBA-style 修复,但 agent runtime 不能恢复,需要重新 add_agent
- **风险 5 — 中间态可见性**:user entry 还在 active 但 thread 已写部分 leave event → 等同 "agent stop" 状态,良性 + 用户重试可推进至完成

回滚策略:本设计**不引入历史重写**,所有变更都是新 commit。任何意外的 archive 操作都能 unarchive 反悔(物理清理 agent clone dir 例外)。

## 合并前校验清单

参照 leave-channel 模式:

1. `cargo build -p gitim-daemon -p gitim-runtime -p gitim-cli` 通过
2. `cargo test -p gitim-daemon -p gitim-runtime -p gitim-cli -p gitim-client -p gitim-agent-provider`(scoped,依据 CLAUDE.md 测试节奏)
3. `cargo fmt --check` 通过
4. `cargo clippy --workspace -- -D warnings` 通过
5. **性能 baseline**:1k synthetic thread workspace `depart_user` 延迟 ≤ 500ms(不达标:开 follow-up plan,不阻塞 merge)
6. 任务末尾跑一次 `cargo test` 全量确认 zero regression
7. 手工 smoke:WebUI 创建 throwaway agent → 让它在 #dev 发 3 条消息 → DM 一条给你 → 让它执行 `gitim burn-self`(测 self-burn 路径)→ 主页刷新看 agent 列表 / DM 列表 / 频道滚动里 alice 痕迹是否完全无踪 + 频道里能看到 alice 自己的 leave-workspace event → `list_archived_users` 能找到归档档案 → 手动 `unarchive_user` 后 user 档案回 active 但 agent 仍然停跑

## 开放问题(plan-eng-review 后定)

1. **channels meta.yaml 的 members 清理**:burn 时 `depart_user` Phase 3 是否一并 remove departed handler?
   - 不清:list_channels 输出会有"幽灵成员",但 user 已 archived,渲染层会过滤
   - 清:多扫一遍 channels meta(每个 channel 一个独立 commit)
   - **倾向:清**(数据模型一致性更高,代价小;reflected 在上面 happy path Phase 3)
2. **handler 重用**:burn alice 后,新 agent 是否允许使用 handler `alice`?
   - **倾向:不允许**,handler 终身唯一,burn 后冻结。`add_agent` 的 conflict check 扩展到 archive/users/
3. **`leave-workspace` event 的具体 token**:`leave-workspace` / `depart` / 其他?plan-eng-review 时定一个

## 文件清单(预期 diff)

```
docs/plans/2026-05-09-archive-protocol/01-plan.md      ← 本文档
docs/specs/archive-protocol.md                          ← spec(Part F)

crates/gitim-daemon/src/api.rs                          — Request enum +6 variant + depart_user
crates/gitim-daemon/src/handlers/mod.rs                 — 路由
crates/gitim-daemon/src/handlers/user.rs                — archive_user / unarchive_user / depart_user / list_archived_users(可能新建)
crates/gitim-daemon/src/handlers/send.rs                — DM 拦截 + archive_dm / unarchive_dm / list_archived_dms
crates/gitim-daemon/src/handlers/poll.rs                — archive/dm/ + archive/users/ diff 处理

crates/gitim-runtime/src/http.rs                        — POST /agents/burn endpoint + 编排 + type 校验
                                                          标 agents/remove deprecated

crates/gitim-cli/src/main.rs                            — Commands enum +5 variant
crates/gitim-cli/src/commands/dm.rs                     — 新建,4 个 cmd
crates/gitim-cli/src/commands/burn.rs                   — 新建,burn-self cmd
crates/gitim-client/src/client.rs                       — 5 个客户端方法

crates/gitim-agent-provider/src/prompts.rs              — leave-workspace event 解读 + archive-dm + burn-self 用法

products/gitim/frontend/src/.../dm-list.tsx             — Archive 按钮 + show-archived toggle(E.2)
products/gitim/frontend/src/.../agent-detail.tsx        — Burn 按钮 + 二次确认(E.3)
products/gitim/frontend/src/.../agent-list.tsx          — show-archived toggle + dual-source 处理(E.3)
```

预计 ~14 个文件,新增 ~700 行(主要是 daemon handler + 测试 + 三个 CLI 命令 + WebUI 两个 sub-part)。

## 不做 v1 的边界(明确写下,避免 scope creep)

- **cross-burn(agent 杀别的 agent)**:涉及权限模型(role/capability/ACL),v2 处理。multi-agent 协作场景下,coordinator 给 worker 发"请退出"消息,worker 自己 `burn-self`
- **"常驻 vs 即用即抛"数据模型字段**:不进 user meta,等 use case 长出来再判断
- **物理 git rebase 清除历史(决策 C 档)**:永远不做
- **DM 归档双方共识流程(决策 B2)**:v1 用 B1 单方触发 + unarchive 反悔
- **channels 里 departed user 发言的物理变化**:不做(只 UI 渲染时灰化标记 — 渲染逻辑由前端处理,本设计文档定义到 daemon API 边界为止)
- **Burn 操作的 unburn**:v1 不做。误操作只能通过内部 `unarchive_user` / `unarchive_dm` 部分恢复 + 手动重新 `add_agent`
- **TTL / idle 自动 burn**:v1 不做,等"即用即抛 coder"用例长出来再判断
- **`gitim-index` 性能优化**:v1 朴素扫描,不达 baseline 开 follow-up plan(不阻塞 v1 merge)
- **`agents/remove` 长期保留**:v1 标 deprecated,后续 release 清理(不再"留作 split-brain 修复手段")
- **人类 user 退出 workspace**:`archive_user` daemon 层 type-agnostic 留口子,但 v1 唯一入口 `agents/burn` 强制 verify target 是 agent
