# archive-protocol:agent burn + DM archive + 系统级归档原语

## 背景

测试阶段创建一堆 throwaway agent → 发现不合适 → 想"快速回收"。现有路径只有 `agents/stop` / `agents/remove` / `agents/remove?hard_delete=true` 三件套,而 `hard_delete` **只删 agent 自己的 clone 目录**。共享仓库里的:

- `users/<handler>.meta.yaml`(用户档案)
- `channels/<X>.thread`(它历史发的所有消息,逐行混在主时间线)
- `dms/X--<handler>.thread`(它所有 DM,在每个对端 DM 列表里)
- `channels/<X>.meta.yaml` 的 `members:` 列表

**全部完整保留**。结果:你"删"了它,UI 上每条线索都在 — DM 列表、频道滚动、用户列表、mention,处处可见。这是用户痛点的根源。

[../2026-04-16-runtime-idle-exit.md](../2026-04-16-runtime-idle-exit.md) 解的是 runtime **进程级** idle 退出,不是 agent 概念级的退出。本设计填的是**概念级"退出"**这一空白。

## 现役同构 pattern

- [../2026-04-03-channel-archive.md](../2026-04-03-channel-archive.md) — 频道归档,`git mv channels/X → archive/channels/X` + 写入拦截 + 读取 fallback + poll diff 处理。已上线。
- card-archive([crates/gitim-daemon/src/card_handlers.rs](../../../crates/gitim-daemon/src/card_handlers.rs))— 卡片归档,同 pattern。已上线。

**两个 instance 已经在线**。本次工作把这套 pattern 提炼为**系统级原语**,新增 **user 和 dm** 两个 instance。

## 关键设计决策

1. **B 档 directory-as-state**:`git mv` 到 `archive/<surface>/`,不靠 metadata 字段
2. **对人 UI 完全无痕,对 agent 诚实**(决策 A2):burn 时 daemon 在 departed agent 曾参与的每个 thread 末尾 append 一条 `[L<n>][@system][<ts>] @<handler> departed` 事件。**人 UI** 默认隐藏来自 archived user 的所有内容;**agent poll** 仍能看到旧消息 + 新增的 departure event,自己决定如何处理(LLM context 不"失忆")
3. **DM 单方触发即归档**(决策 B1):任一对端发起即生效,无需 confirm。提供 unarchive 反悔通道
4. **即用即抛 v1 不入数据模型**:burn 是单一统一动作,user meta 不加 `lifecycle` 字段。等 use case 长出来再判断
5. **不重写 git 历史**:audit trail 永久保留,所有"删除"都是新 commit + 可逆
6. **spec 形式化**:`docs/specs/archive-protocol.md` 提炼成系统承诺,所有未来 archivable surface 必须遵守

## archive-protocol(v1 spec)

每个 archivable surface 必须实现以下 5 条 contract(写入 `docs/specs/archive-protocol.md`):

1. **目录命名**:active 路径 `<surface>/...` ↔ archive 路径 `archive/<surface>/...`(镜像)
2. **写入拦截**:active path 不存在时检查 archive path,如存在,所有 mutation 必须返回明确错误(包含 "is archived" 或 "is departed" 字样)
3. **读取 fallback**:active path 不存在时尝试 archive path,响应附 `archived: true` 字段
4. **poll diff 处理**:thread 出现在 archive 路径下 → poll 应将其识别为归档事件,不可误判为新建 / 删除
5. **可逆**:必须提供 unarchive 操作(物理清理路径如 hard_delete agent clone dir 除外)

surface 实现表:

| Surface | active path | archive path | 状态 |
|---|---|---|---|
| channel | `channels/X.thread` + `.meta.yaml` | `archive/channels/X.*` | 已实现 |
| card | `channels/X/cards/Y/...` | `archive/channels/X/cards/Y/...` | 已实现 |
| **user** | `users/X.meta.yaml` | `archive/users/X.meta.yaml` | **本次新增** |
| **dm** | `dms/X--Y.thread` | `archive/dms/X--Y.thread` | **本次新增** |

未来加新 archivable surface(如 reactions、附件、子线程),必须配套补这张表 + 走完 5 条 contract。

## Agent burn 工作流(happy path)

人类点 WebUI "Burn alice" → `POST /workspaces/{slug}/agents/burn { id: "alice" }` →

1. abort agent loop / kill agent daemon process(同现有 hard_delete 流程)
2. **daemon 端走 archive-protocol(单 commit)**:
   - 扫描 `channels/*.thread` + `dms/*--alice.thread` + `archive/dms/*--alice.thread`,识别 alice 曾发言的 thread
   - 对每个 thread,append 一行 `[L<n>][@system][<现在 ts>] @alice departed`
   - `git mv users/alice.meta.yaml → archive/users/alice.meta.yaml`
   - `git mv` 所有 `dms/*--alice.thread` 到 `archive/dms/`(对端可能是任意人,不止操作者)
   - (开放问题 1)清理 `channels/*.meta.yaml` 的 `members:` 中的 alice 引用
   - 单 commit:`archive: depart @alice (system)`
   - push with retry(参照 channel-archive)
3. rm -rf agent clone dir(同 `hard_delete_agent_dir`)
4. 删 hermes profile(best-effort,失败仅 warn)
5. 从 in-memory `ctx.agents` 移除
6. 触发 SSE 通知 WebUI 刷新

错误处理:
- 步骤 2 失败 → 整个操作失败,前端 500,**不动** agent runtime / clone dir(可重试)
- 步骤 3 / 4 失败 → 警告日志,但 commit 已成功,**不再回滚**(channels 里 system event 已发出去)。clone dir 残留可手动 cleanup

反向操作(unburn):**v1 不做**。理由:burn 已 rm agent clone dir,即使 git mv 用户档案回 active 路径,agent runtime 也得重新 provision(handler 冲突防护要先解、re-clone 仓库等)。等真有 use case 再做。误 burn 时 v1 通过内部命令 `unarchive-user` + `unarchive-dm` 部分恢复(只恢复 UI 可见性,不恢复 agent 运行时)。

## 工作分块

### Part A — daemon API:user / dm archive + system event

文件:
- [crates/gitim-daemon/src/api.rs](../../../crates/gitim-daemon/src/api.rs) — Request enum 新增 6 个 variant
- [crates/gitim-daemon/src/handlers/](../../../crates/gitim-daemon/src/handlers) — 新增 user / dm 处理(可能新建 `user.rs` / 复用 `send.rs`)
- [crates/gitim-daemon/src/handlers/poll.rs](../../../crates/gitim-daemon/src/handlers/poll.rs) — archive/dms/ + archive/users/ 路径处理
- [crates/gitim-daemon/src/handlers/send.rs](../../../crates/gitim-daemon/src/handlers/send.rs) — DM / user 拦截

新增 daemon API:
- `archive_user` / `unarchive_user`(内部用,WebUI / CLI 不直接暴露)
- `archive_dm { peer }` / `unarchive_dm { peer }`(主动用 + 由 burn 编排)
- `list_archived_users` / `list_archived_dms`

写入拦截升级:
- `handle_send` DM 分支:写入前检查对应 sorted-pair 文件名是否落在 `archive/dms/`,是则返回 "DM with @X is archived"
- `handle_send` 用户检查:author 或 mention target 在 `archive/users/` → 返回 "user @X is departed"
- `handle_register_user` / onboard:handler 重用策略(开放问题 2)

读取 fallback:
- `handle_list_users`:默认只列 `users/`,新增 `--include-archived`
- `handle_read` DM 路径:active 不存在时 fallback `archive/dms/`,响应附 `archived: true`(对称 channel 已有逻辑)

system departure event 写入(由 archive_user 触发):
- 扫描所有 active thread 与 archive/dms/* 找 author 是 departed handler 的文件
- 每个匹配 thread 末尾 append 一行 `[L<下一行号>][@system][<ts>] @<handler> departed`
- thread parser 需 verify 接受 `author=@system`(开放问题 3)

验收:
- archive_user 后 `list_users` 不见 / `list_archived_users` 见
- archive_dm 后 read 该 DM 显示 `archived: true`,send 返回 "is archived"
- system event:burn alice → 所有 alice 发过言的 thread 末尾各多一行 `@system @alice departed`
- poll diff:`archive/dms/X--Y.thread` 出现 → 识别为 dm_archived 事件

### Part B — runtime: burn endpoint

文件:[crates/gitim-runtime/src/http.rs](../../../crates/gitim-runtime/src/http.rs)

新加 endpoint:`POST /workspaces/{slug}/agents/burn { id: String }`

handler 编排顺序见上面 happy path 步骤 1-6。

向后兼容:
- 现有 `agents/stop` / `agents/remove` 不变
- `agents/remove?hard_delete=true` 仍只 rm agent clone dir,不动共享仓库 — 保留作 split-brain 修复 / debug 手段
- WebUI 主路径切换到 burn

### Part C — CLI

文件:
- [crates/gitim-cli/src/main.rs](../../../crates/gitim-cli/src/main.rs) — Commands enum 加 4 个 variant
- [crates/gitim-cli/src/commands/dm.rs](../../../crates/gitim-cli/src/commands/dm.rs)(新建)
- [crates/gitim-client/src/client.rs](../../../crates/gitim-client/src/client.rs) — 客户端方法

新增 CLI 命令(对称 archive-channel 风格):
- `gitim archive-dm <peer>`
- `gitim unarchive-dm <peer>`
- `gitim list-archived-dms`
- `gitim list-archived-users`

`archive-user` / `unarchive-user` 是 daemon 内部 API,**不暴露 CLI**(防止人/agent 误把自己 burn 掉,与 WebUI 的"Burn"按钮+二次确认形成单一入口)。

测试约定:跟 leave-channel 一致,**不**新增 CLI 集成测试,靠 daemon handler 单测 + 集成 + 手工 smoke 兜底(memory:`feedback_indwell_existing_designs_first`)。

### Part D — Agent prompt

文件:[crates/gitim-agent-provider/src/prompts.rs](../../../crates/gitim-agent-provider/src/prompts.rs)

加两块内容,沿用 AI 第一性原理 + 交接语气(memory `feedback_prompt_style_for_llms`):

1. **system departure event 解读**(放 `default_reset_protocol` 或新建小节):
   - `@system` 作者 + "departed" 关键词 = 对方已离开 workspace
   - 不要再 mention / @ 它(消息发不出,daemon 会拒)
   - 必须提及它过去的发言时,用过去式("之前 @X 提到过") + 不要假设它会回应

2. **archive-dm / unarchive-dm 命令**(放 `default_gitim_api` DM 小节,跟 leave-channel 同级):
   - context 净化的精细化手段:某条 DM 线索不再相关 → archive-dm
   - 与 leave-channel 同级:leave-channel 切断频道订阅,archive-dm 切断单条 DM。`[[RESET]]` 是 session 级重锤
   - 是公开行为(写 commit、影响双方)— 不是躲避争议工具

### Part E — WebUI

文件:`products/gitim/frontend/...`(具体 component 路径在 plan-eng-review 时切)

改动:
- agent detail 页:把"Hard Delete"按钮替换为"**Burn**"(标红),二次确认 dialog 文案明示"将归档该 agent 的 user 档案 + 所有 DM、清理 clone 目录,**不可一键恢复**(可手动 unarchive 单条 DM)"
- DM 列表:每条 DM 加"归档"按钮(右键菜单 / hover action)
- "Show archived" toggle:channels / DMs / agents 三个列表都加(默认隐藏 archived)
- agent 列表:不显示 archived users(查询路径默认 `list_users` 不 include archived)

### Part F — spec 文档

文件:[docs/specs/archive-protocol.md](../../specs/archive-protocol.md)(新建)

把上面"archive-protocol(v1 spec)"那段独立成 spec 文档,作为系统级承诺。未来加 archivable surface 时,**必须先读 spec 再实现**。spec 内容应包含:5 条 contract 的形式化定义、surface 表、命名约定、push retry 模式参考。

## 风险 / 回滚

- **风险 1 — burn 时扫所有 thread 慢**:workspace 大(>10k 消息)时几秒延迟。**缓解**:用 [gitim-index](../../../crates/gitim-index)(SQLite FTS5)走 author 反查;v1 先朴素扫描,有性能问题再优化(开放问题 4)
- **风险 2 — system departure event 大量写入触发 sync conflict**:沿用 channel-archive 的 push retry / rebase 模式,已被验证
- **风险 3 — handler 重用混乱**:burn alice 后再 add 同名 agent 行为待定(开放问题 2)
- **风险 4 — 误 burn**:`unarchive_user` + `unarchive_dm` 暴露内部 API 作 DBA-style 修复路径,但 agent runtime 不能恢复,需要重新 add_agent

回滚策略:本设计**不引入历史重写**,所有变更都是新 commit。任何意外的 archive 操作都能 unarchive 反悔(物理清理 agent clone dir 例外)。

## 合并前校验清单

参照 leave-channel 模式:

1. `cargo build -p gitim-daemon -p gitim-runtime -p gitim-cli` 通过
2. `cargo test -p gitim-daemon -p gitim-runtime -p gitim-cli -p gitim-client -p gitim-agent-provider`(scoped,依据 CLAUDE.md 测试节奏)
3. `cargo fmt --check` 通过
4. `cargo clippy --workspace -- -D warnings` 通过
5. 任务末尾跑一次 `cargo test` 全量确认 zero regression
6. 手工 smoke:WebUI 创建 throwaway agent → 让它在 #dev 发 3 条消息 → DM 一条给你 → burn → 主页刷新看 agent 列表 / DM 列表 / 频道滚动里 alice 痕迹是否完全无踪 → `list_archived_users` 能找到归档档案 → 手动 `unarchive_user` 后 user 档案回 active 但 agent 仍然停跑

## 开放问题(plan-eng-review 时收敛)

1. **channels meta.yaml 的 members 清理**:burn 时是否一并把 departed handler 从所有 `channels/*.meta.yaml` 的 `members:` 列表移除?
   - **不清**:list_channels 输出会有"幽灵成员"。但 `members:` 主要用于 poll filter,user 已 archived 后渲染层会过滤,功能上不会出错
   - **清**:多扫一遍 channels meta + 多写几个文件 entry 进同一个 commit。语义上更干净
   - **倾向:清**(单 commit 已经在写,代价小;数据模型一致性更高)

2. **handler 重用**:burn alice 后,新 agent 是否允许使用 handler `alice`?
   - **允许**:archive/users/alice.meta.yaml 不阻止 users/alice.meta.yaml 重新创建。但 channel 历史里 author=alice 的旧消息让新 alice 看上去像旧 alice 的延续 → 认知混乱
   - **不允许**:archive/users/ 下的 handler 视为永久占用,新 agent 必须用别的 handler。简单一致,但 reservation 集合会无限增长(可后续加 GC)
   - **倾向:不允许**(handler 是终身唯一标识,burn 后冻结)。`add_agent` 的 handler_conflict 检查需要扩展到检查 `archive/users/`

3. **system departure event 的格式**:`[L<n>][@system][<ts>] @<handler> departed` 中:
   - thread parser 是否已支持 `@system` 作为 author?需 verify [crates/gitim-core/src/parser/](../../../crates/gitim-core/src/parser)
   - line number 沿用文件下一行编号(常规)?
   - parent line 字段如何填(没有自然 parent — 设为 0 或省略)?
   - 现有 `system` 是 reserved handler(CLAUDE.md 已载明),复用它

4. **性能 / 索引**:burn 时扫 thread 找 author 的代价。先朴素扫描 + watch metric,有性能问题再考虑用 [gitim-index](../../../crates/gitim-index) 加速

5. **`dms/X--Y.thread` 的 sorted handler pair**:archive_dm 调用时只给 `peer`,需要根据 caller(发起者 handler)正确解析双向文件名 — 这是 daemon 的 author 推断 + 文件命名约定的事,确认现有 DM handler 已经处理好

## 文件清单(预期 diff)

```
docs/plans/2026-05-09-archive-protocol/01-plan.md      ← 本文档
docs/specs/archive-protocol.md                          ← spec 提炼(Part F)

crates/gitim-daemon/src/api.rs                          — Request enum +6 variant
crates/gitim-daemon/src/handlers/mod.rs                 — 路由
crates/gitim-daemon/src/handlers/user.rs                — archive_user / unarchive_user / list_archived_users / system event 写入(可能新建)
crates/gitim-daemon/src/handlers/send.rs                — DM 拦截 + archive_dm / unarchive_dm / list_archived_dms
crates/gitim-daemon/src/handlers/poll.rs                — archive/dms/ + archive/users/ diff 处理

crates/gitim-runtime/src/http.rs                        — POST /agents/burn endpoint + 编排

crates/gitim-cli/src/main.rs                            — Commands enum +4 variant
crates/gitim-cli/src/commands/dm.rs                     — 新建,4 个 cmd
crates/gitim-client/src/client.rs                       — 4 个客户端方法

crates/gitim-agent-provider/src/prompts.rs              — system event 解读 + archive-dm 用法

products/gitim/frontend/src/.../agent-detail.tsx        — Burn 按钮 + 二次确认
products/gitim/frontend/src/.../dm-list.tsx             — Archive 按钮 + show-archived toggle
products/gitim/frontend/src/.../sidebar.tsx             — Show archived toggle(channels/agents 列表)
```

预计 ~13 个文件,新增 ~600 行(主要是 daemon handler + 测试)。

## 不做 v1 的边界(明确写下,避免 scope creep)

- **agent 自我退出(self-burn)**:v1 不暴露 CLI,只有人类能 burn
- **"常驻 vs 即用即抛"数据模型字段**:不进 user meta
- **物理 git rebase 清除历史(决策 C 档)**:永远不做
- **DM 归档双方共识流程(决策 B2)**:v1 用 B1 单方触发,提供 unarchive 反悔
- **channels 里 departed user 发言的物理变化**:不做(只 UI 渲染时灰化标记 — 实际 UI 渲染逻辑由前端处理,本设计文档定义到 daemon API 边界)
- **Burn 操作的撤销(unburn)**:v1 不做。误操作只能通过内部 unarchive_user/dm + 手动 re-add_agent 部分恢复
- **TTL / idle 自动 burn**:v1 不做。等"即用即抛 coder"用例长出来再判断
- **`agents/remove?hard_delete=true` 的弃用**:v1 保留作 split-brain 修复手段
