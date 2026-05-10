# Archive Protocol v1

> GitIM 系统级"归档"原语 — 所有未来 archivable surface 必须遵守的统一契约。

---

## 概述

GitIM 的 channel-archive、card-archive、user-archive、dm-archive 共享一组同构 pattern:目录命名规则、写入拦截、读取 fallback、poll diff 识别、可逆操作。本 spec 把这组 pattern 提炼成系统承诺,固化下来。

目标:

- **对人 UI 完全无痕**:UI 默认看不到归档对象的任何痕迹
- **git audit trail 永久保留**:所有"删除"都是新 commit,历史永远可追
- **所有归档可逆**:每个 archive 操作配套 unarchive(物理清理路径除外)

设计背景与决策见 [01-plan.md](../plans/2026-05-09-archive-protocol/01-plan.md)。

---

## 核心原则

1. **directory-as-state**:active 路径与 archive 路径的存在性是状态的唯一 source of truth。**不**靠 metadata 字段(`is_archived`、`status` 等)标记
2. **不重写 git 历史**:所有"删除"都是新 commit。任何归档操作可通过 unarchive 反悔
3. **commit author = caller handler**:归档操作的 commit author 是触发者本人;系统级动作(如 `depart_user`)由 daemon 代签 caller handler。**不**引入新 author 类型(如 `system`)
4. **幂等**:重复触发同一归档操作不产生新 commit,return success

---

## 5 条 Contract

每个 archivable surface 必须满足以下 5 条。新 surface 提 PR 时直接当 checklist 用。

### Contract 1 — 目录命名

active path:`<surface>/...`
archive path:`archive/<surface>/...`

archive 路径必须是 active 路径的**完整镜像**,只在最外层加 `archive/` 前缀。

嵌套 surface(例:channel 内的 card)也保持镜像:

| active | archive |
|---|---|
| `channels/X.thread` | `archive/channels/X.thread` |
| `channels/X/cards/Y/...` | `archive/channels/X/cards/Y/...` |

**反例**(MUST NOT):`archive/cards/Y` — 把嵌套打平了,丢失父 surface 上下文。

### Contract 2 — 写入拦截

任何对 active path 的 mutation 操作(`send`、`join`、`leave`、`update_*` 等)必须先 stat 对应 archive path。如果 archive path 存在,mutation MUST 返回明确错误,错误信息包含以下关键词之一:

- `"is archived"` — 用于内容性 surface(channel / DM / card)
- `"is departed"` — 用于 user 类 surface(语义略有差异:user 不是被"归档",而是已"退出")

错误形式:daemon RPC 返回 error 字符串,CLI / WebUI 透传给用户。**不**降级为 silent no-op。

### Contract 3 — 读取 fallback

任何对 active path 的 **read** 操作:active 不存在时,MUST 尝试 archive path。如果 archive path 存在,return 内容 + 在响应里附 `archived: true` 字段。

**例外 — list 类操作不 fallback / merge**:`list_channels` / `list_users` / `list_dms` 等默认只返回 active 集合,提供显式 `include_archived` 参数(或独立 `list_archived_*` 端点)开启归档可见。

理由:list 是聚合视图,默认 merge active + archived 会让所有人 UI 都看见归档对象,违反"对人 UI 完全无痕"。fallback 只用于点查(知道具体名字才能查到)。

### Contract 4 — poll diff 处理

git sync 拉到包含 `archive/<surface>/...` 路径变更的 commit 时,poll MUST 识别为"归档/取消归档事件",**不可**误判为新建 / 删除 / 跨目录 rename。

事件类型由 surface 自定义:

- `channel_archived` / `channel_unarchived`
- `dm_archived` / `dm_unarchived`
- `user_archived` / `user_unarchived`
- ...

具体投递规则(broadcast vs 定向)见下方 [Visibility model](#visibility-model)。

### Contract 5 — 可逆

每个 archive 操作 MUST 配套 unarchive 操作。原则上是 git mv 反向。

**例外**:伴随归档的不可逆物理清理(如 `hard_delete` 一个 agent 的 clone dir)。这部分**不在** archive 操作的契约内,是上层动作的副作用。`unarchive_user` 能恢复 user.meta.yaml 的可见性,但不能复活 agent runtime — 那是 `add_agent` 的职责。

---

## 复合操作模式:幂等多 commit + 终态判断

涉及多 surface 的复合归档(例:agent burn = 写 leave events + 归档 DM + 清理 channels meta + 归档 user),**不要**追求 single-commit 原子性 — push 冲突时 rebase 复杂度高、错误恢复脆弱。

推荐模式:

1. **明确终态不变量**:某个 archive path 的存在 = 整个复合操作完成。该不变量是 daemon、runtime、外部 clone 一致的单一判定标准
2. **幂等检查先行**:操作开始就 check 终态,已完成直接 return success(无副作用)
3. **分阶段串行,各阶段独立 commit**:每阶段失败可单独重试。**不**回滚已成功阶段
4. **每阶段步骤幂等**:步骤已完成则 skip(用 stat / 文件内容判断)
5. **终态最后写入**:终态 commit MUST 是最后一步。中间崩溃时,终态文件不存在 → 重试时知道还有未完成步骤

### 参考实现:`depart_user`(agent burn 编排)

终态不变量:`archive/users/<handler>.meta.yaml` 存在。

四个 phase,每个 phase 独立 commit、独立幂等:

1. **Phase 1**:扫所有 active thread,在 handler 发过言的每个 thread 末尾 append `leave-workspace` event(每个 thread 一个 commit;末尾已是该 event 则 skip)
2. **Phase 2**:归档 handler 的所有 DM(每个 DM 一个 git mv commit;已在 archive/dm/ 则 skip)
3. **Phase 3**:从所有 channels meta 的 members 列表移除 handler(每个 channel 一个 commit;不在 members 则 skip)
4. **Phase 4**:`git mv users/<handler>.meta.yaml → archive/users/`(终态)

任何 phase 失败 → return error,user 重试 → 幂等检查跳过已完成,继续未完成。

实现见 `crates/gitim-daemon/src/handlers/depart.rs`。

---

## Surface 实现表

| Surface | active path | archive path | commands / API |
|---|---|---|---|
| channel | `channels/X.thread` + `channels/X.meta.yaml` | `archive/channels/X.thread` + `archive/channels/X.meta.yaml` | `archive_channel` / `unarchive_channel` |
| card | `channels/X/cards/Y/...` | `archive/channels/X/cards/Y/...` | `archive_card` / `unarchive_card` |
| user | `users/X.meta.yaml` | `archive/users/X.meta.yaml` | `archive_user` / `unarchive_user`(daemon 内部);`depart_user`(复合,burn 编排专用) |
| dm | `dm/X--Y.thread` | `archive/dm/X--Y.thread` | `archive_dm` / `unarchive_dm` |

**注意**:DM 目录是 `dm/`(单数),**不是** `dms/`。底层文件命名规则(两个 handler 字典序 + `--` 连接)见 [channels-and-dm.md](channels-and-dm.md)。

### 加新 archivable surface 的流程

未来加新 archivable surface(reactions、附件、子线程等)MUST:

1. **设计阶段**:补这张 Surface 实现表(active path、archive path、commands)
2. **实现阶段**:走完上面 5 条 contract,每条都有对应测试
3. **PR commit message**:引用本 spec(`Implements archive-protocol v1 for <surface>`)

---

## Read 一致性细节

`poll` 与 `list_*` 的归档过滤行为是**有意**不同的,记录于此防止未来无意 drift:

| 操作 | 对 archived user 的旧 thread 内容 | 对 archived 对象本身 |
|---|---|---|
| `poll` | **不**过滤(thread 历史保持不变) | poll diff 投递归档事件(Contract 4) |
| `list_*` | N/A(list 不返回 thread 内容) | 默认过滤;`include_archived` 显式开启 |

理由:

- `poll` 不过滤 archived user 的旧消息 — agent 必须看到完整历史(决策 A2:对 agent 诚实)。原 thread 内容在 git log 里本来就在,过滤会让 agent 失忆
- `list_*` 默认过滤 — 默认聚合视图必须"对人 UI 完全无痕",对所有 caller 一致(daemon **不**区分人 vs agent caller,一致性由 caller 显式传 `include_archived` 控制)

人 UI 通过 default `list_*` 看不到 archived;agent 也通过 default `list_*` 看不到 archived。两者一致。差异仅在 `poll` 返回的 thread 内容里。

---

## Visibility Model

归档事件(Contract 4)在 poll diff 里的投递范围因 surface 而异。这是有意设计,不是疏漏:

| Surface | 投递范围 | 理由 |
|---|---|---|
| channel | workspace 全员广播 | channel 存在性本身就是非保密信息,所有 member 应看到归档变化 |
| dm | 仅参与方(两个 handler) | DM 是私密关系,旁观者不应感知归档 |
| user | workspace 全员广播 | user 离开是会话级事实,所有人 mention 该 handler 的行为都受影响 |
| card | 所属 channel 的成员 | card 可见性继承 channel |

新 surface 的 visibility 必须在引入时显式声明,不留默认。

---

## Versioning

本 spec 用 SemVer-ish 版本号(v1, v2, ...)。

### 加新 contract 的政策

- **旧 surface 不要求立即 retrofit**:旧 surface 标 `v<N> compliant`(N = 它实现时的 spec 版本)。retrofit 工作单独追踪(GitHub issue / TODO 列表),**不**阻塞新 contract 的 merge
- **新加 archivable surface MUST 实现当时最新的 spec**(全部 contract,不可挑选)

### 版本兼容

- v1 → v2 增量:加新 contract 不破坏旧实现的语义(只是要求新 surface 多做一些事)
- 任何破坏性变更(改现有 contract 语义、改路径命名规则)= major bump,需要单独迁移 plan

---

## 涉及源文件

| 文件 | 职责 |
|---|---|
| `crates/gitim-daemon/src/handlers/channel.rs` | channel archive/unarchive |
| `crates/gitim-daemon/src/card_handlers.rs` | card archive/unarchive |
| `crates/gitim-daemon/src/handlers/user.rs` | user archive/unarchive |
| `crates/gitim-daemon/src/handlers/depart.rs` | `depart_user` 复合编排(参考实现) |
| `crates/gitim-daemon/src/handlers/dm.rs` | DM archive/unarchive |
| `crates/gitim-daemon/src/handlers/send.rs` | 写入拦截(channel / DM / user) |
| `crates/gitim-daemon/src/handlers/poll.rs` | archive 路径 diff → 归档事件 |
| `crates/gitim-sync/src/git.rs` | `GitStorage::mv` |

测试参考:

- `crates/gitim-daemon/tests/depart_user_test.rs` — 幂等性、半态恢复、零发言 agent
- `crates/gitim-daemon/tests/cross_clone_burn_test.rs` — 跨 clone fetch 同步行为
