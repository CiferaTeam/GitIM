# 07 — spec 文档:archive-protocol

> 对应 [01-plan.md](01-plan.md) Part F。形式化 archive 系统级原语,作为后续新 archivable surface 的强制约束。1 个 task。

## F.1 — 写 docs/specs/archive-protocol.md

**文件**:[docs/specs/archive-protocol.md](../../specs/archive-protocol.md)(新建)

**版本**:v1(用 SemVer-ish 标识,未来加 contract 时升级)

### 文档大纲

```
# Archive Protocol v1

## 概述
GitIM 系统级"归档"原语。把 channel-archive / card-archive 等已落地的 directory-as-state 模式提炼成统一约束,所有未来 archivable surface 必须遵守。

目标:对人 UI 完全无痕,git audit trail 永久保留,所有归档可逆(物理清理路径除外)。

## 核心原则

1. **directory-as-state**:active 路径 vs archive 路径的存在性是状态的 source of truth,不靠 metadata 字段标记
2. **不重写 git 历史**:所有"删除" = 新 commit。任何归档操作可通过 unarchive 反悔
3. **commit author = caller handler**(或系统级动作时由 daemon 代签 caller handler)— 不引入新 author 类型
4. **幂等**:重复触发同一归档操作不产生新 commit,return success

## 5 条 contract

### Contract 1: 目录命名
active path:`<surface>/...`
archive path:`archive/<surface>/...`
archive 路径必须是 active 路径的 **完整镜像**,只在最外层加 `archive/` 前缀。嵌套 surface 也保持镜像(例:`channels/X/cards/Y` ↔ `archive/channels/X/cards/Y`,**不是** `archive/cards/Y`)。

### Contract 2: 写入拦截
任何对 active path 的 mutation 操作,必须先 stat 对应 archive path。如果 archive path 存在(说明该 surface 已归档),mutation 必须返回明确错误,错误信息包含以下关键词之一:
- "is archived"(用于内容性 surface,如 channel / DM / card)
- "is departed"(用于 user 类 surface,语义略不同)

错误形式:daemon RPC return error 字符串,CLI / WebUI 透传给用户。

### Contract 3: 读取 fallback
任何对 active path 的 read 操作,在 active 不存在时,**必须**尝试 archive path。如 archive path 存在,return 内容 + 在响应里附 `archived: true` 字段。

例外:list 类操作默认 **不** fallback / merge — 默认只列 active,提供显式 `include_archived` 参数。

### Contract 4: poll diff 处理
git sync 拉到包含 `archive/<surface>/...` 路径变更的 commit 时,poll 必须识别为"归档/取消归档事件",**不可**误判为新建 / 删除 / 跨目录 rename。事件类型由 surface 自定义(例:dm_archived / channel_archived / user_archived)。

### Contract 5: 可逆
每个 archive 操作必须配套 unarchive 操作。原则上是 git mv 反向。例外:伴随归档的不可逆物理清理(如 hard_delete 一个 agent 的 clone dir)— 这部分不在 archive 操作的契约内,是上层动作的副作用。

## 复合操作模式:幂等多 commit + 终态判断

涉及多 surface 的复合归档(例:agent burn = 写 events + 归档 DM + 归档 user),不要追求 single-commit 原子性 — 太脆弱,push 冲突时 rebase 复杂度高。

推荐模式:
1. **明确终态不变量**:某个 archive path 的存在 = 整个复合操作完成。例:agent burn 的终态是 `archive/users/<handler>.meta.yaml` 存在
2. **幂等检查先行**:操作开始就 check 终态,已完成直接 return
3. **分阶段串行,各阶段独立 commit**:每阶段失败可单独重试。不回滚已成功阶段
4. **每阶段步骤幂等**:步骤已完成则 skip(用 stat / 文件内容判断)
5. **最后一步标志整体完成**:终态 commit 必须最后写入

参考实现:[02-daemon.md](02-daemon.md) A.4 `depart_user`。

## surface 实现表

| Surface | active path | archive path | 命令 / API |
|---|---|---|---|
| channel | `channels/X.thread` + `.meta.yaml` | `archive/channels/X.*` | `archive_channel` / `unarchive_channel` |
| card | `channels/X/cards/Y/...` | `archive/channels/X/cards/Y/...` | `archive_card` / `unarchive_card` |
| user | `users/X.meta.yaml` | `archive/users/X.meta.yaml` | `archive_user` / `unarchive_user`(daemon 内部);`depart_user` 复合(burn 编排用) |
| dm | `dm/X--Y.thread` | `archive/dm/X--Y.thread` | `archive_dm` / `unarchive_dm` |

未来加新 archivable surface,**必须**:
1. 设计阶段先补这张表
2. 实现阶段走完 5 条 contract
3. 提交 PR 时引用本 spec(`Implements archive-protocol v1 for <surface>`)

## read 一致性细节

`poll` 与 `list_users` / `list_channels` / 类似列表 API 的行为差异是有意的:

- `poll`:**不**过滤 archived user 在 active thread 里的旧消息内容(原 thread 内容不变,只 append 了 leave-workspace event)。这让 agent 看到完整历史 + 新事件,自己决定如何处理(决策 A2:对 agent 诚实)
- `list_*`:默认过滤 archived(对所有 caller 一致 — daemon 不区分人 vs agent caller)

人 UI 通过 default `list_*` 看不到 archived;agent 也通过 default `list_*` 看不到 archived。两者一致。差异只在 `poll` — 但 `poll` 返回的 thread 内容里,archived user 的历史消息是历史 commit 的内容,本来就在 git log 里,过滤会让 agent 失忆,违反 A2。

## Versioning

本 spec 用 SemVer-ish 编号(v1, v2, ...)。

加新 contract 的政策:
- **新 contract 不要求旧 surface 立即 retrofit** — 旧 surface 标 "v<N> compliant"(N 是它实现时的 spec 版本)
- retrofit 工作单独追踪(GitHub issue / TODO 列表),不阻塞新 contract 的 merge
- 但**新加的 archivable surface 必须实现当时最新的 spec**(全部 contract)

## 验收(本 task)

- spec 文件存在,内容覆盖以上大纲
- 文案与 [01-plan.md](../plans/2026-05-09-archive-protocol/01-plan.md) 设计决策一致
- 5 条 contract 形式化(可被新 surface 实现者作为 checklist 用)
- surface 表完整(channel / card 现役行为正确,user / dm 本次新加行为正确)
- 文档语气与 [docs/gitim-protocol.md](../../gitim-protocol.md) 风格一致(简洁、技术性、不引入冗余)

## 依赖

无 — 与 daemon / runtime / CLI 实施可并行。但 spec 与 A.4 的 `depart_user` 实现互相参照 — 实施时如果发现 spec 与 reference 实现不一致,以 spec 为准 + 修 reference,**不**改 spec 来迁就实现。
```
