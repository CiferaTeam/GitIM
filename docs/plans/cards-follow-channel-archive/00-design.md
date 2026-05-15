# Cards Follow Channel Archive — 卡片跟随 channel 整体冰封

**Status**: Design (pending user approval)
**Author**: lewis
**Date**: 2026-05-15

---

## 1. 动机

`archive_channel`(Rust daemon + frontend daemon-web 两端实现)当前只 `git mv` channel 自己的两个文件 —— `channels/<ch>.meta.yaml` 和 `channels/<ch>.thread` —— **不动 `channels/<ch>/cards/` 子目录**。后果两层:

1. **目录布局不一致**:`channels/<archived-ch>/cards/` 变成孤儿,`archive/channels/<ch>/cards/` 永远是空的(被 `archive_card` 单独 mv 过来的除外,但那是不同语义)。
2. **可见性洞**:"跟随 channel 一起 archive 的卡片"既不在 `list_cards`(channel 已 archive,扫不到)也不在 `list_archived_cards`(只扫 `archive/channels/*/cards/`)—— 完全幽灵。`unarchive_channel` 把 channel meta mv 回来后,孤儿 cards 因为目录还在原位自动又可见,看起来"work"但实际依赖一个 leak 的对称巧合。

前置 PR `fix(frontend): refresh cards on channel archive poll change`(commit 48423b0)修了一个症状:前端 polling 没在 `channel_meta` change 时刷新 cards,导致 zustand store 里残留 archived channel 的卡片。**本设计处理深层的目录布局不一致**。

`card-refactor`(2026-04-17)plan 明确写了"channel 归档/删除语义:暂不变更",是本设计的前置。

## 2. 核心决策

| # | 决策 | 理由 |
|---|------|------|
| D1 | Cards 跟随 channel 整体冰封 | archive 语义统一:channel archive = 子资产一起进入归档,unarchive 整体复活 |
| D2 | `card.meta.yaml` 新增 `archived_via: channel \| manual \| null` 字段 | 卡片自包含状态;`unarchive_channel` 仅按字段筛选哪些需要复活;channel meta 保持纯描述 |
| D3 | 目录布局 = archive 状态的真相 | `archived_via != null ⇔ 文件在 archive/ 下`,二者同步;路径即可读判定 |
| D4 | Legacy 孤儿目录在 daemon 启动时一次性 reconcile | 幂等扫描 + 单 commit 修正;仓库立刻进入新不变量,后续代码无需 legacy 分支 |
| D5 | Rust daemon + frontend daemon-web 同时实施 | 仓库共享,行为分裂会引入 inconsistent state(一端按新格式 archive,另一端按旧格式 unarchive)|

## 3. 数据模型

### 3.1 CardMeta 字段扩展

```rust
pub struct CardMeta {
    pub title: String,
    pub channel: String,
    pub status: CardStatus,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_via: Option<ArchivedVia>,  // 新增
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ArchivedVia {
    Channel,  // 跟随 archive_channel
    Manual,   // 被 archive_card 单独归档
}
```

`#[serde(default, skip_serializing_if = "Option::is_none")]` 保证:
- 旧 yaml 没有该字段时读为 `None`(向后兼容)
- 写出时 `None` 不落盘(不污染 active card 的 yaml)

前端 TypeScript `Card` interface 对应加可选字段 `archived_via?: "channel" | "manual"`。

### 3.2 文件布局不变量

```
channels/<ch>/cards/<id>/         ⇔  archived_via 缺省或 null
archive/channels/<ch>/cards/<id>/ ⇔  archived_via == channel | manual
```

违反这个不变量的状态(路径在 archive 但 archived_via 为 null,或反之)是 bug,reconcile 路径负责修复。

## 4. 行为变更

### 4.1 archive_channel

单一 commit:
1. 读 channel meta,做 permission check(仅 creator 可 archive,保留现有逻辑)
2. 扫 `channels/<ch>/cards/<id>/card.meta.yaml`,列出当前 active cards
3. 对每张 card:
   - 改 yaml `archived_via: channel`,写回
   - `git mv channels/<ch>/cards/<id>/` → `archive/channels/<ch>/cards/<id>/`
4. `git mv channels/<ch>.meta.yaml` → `archive/channels/<ch>.meta.yaml`
5. `git mv channels/<ch>.thread` → `archive/channels/<ch>.thread`
6. `rmdir channels/<ch>/`(若现已空)
7. 一次 commit:`archive: #<ch> by @<author>`(message 不变;cards 改动隐含)
8. push-with-retry(沿用现有逻辑)

已 `archived_via: manual` 的卡片(在 `archive/channels/<ch>/cards/` 下)不动 —— 它们路径和字段都对。

### 4.2 unarchive_channel

单一 commit:
1. 读 archive channel meta + permission check(沿用)
2. 扫 `archive/channels/<ch>/cards/<id>/card.meta.yaml`,**filter `archived_via == channel`**
3. 对每张 filtered card:
   - 改 yaml 清除 `archived_via`(写成 null 或删除字段,二者等价 —— 选删除字段)
   - `git mv archive/channels/<ch>/cards/<id>/` → `channels/<ch>/cards/<id>/`
4. `git mv archive/channels/<ch>.meta.yaml` → `channels/<ch>.meta.yaml`
5. `git mv archive/channels/<ch>.thread` → `channels/<ch>.thread`
6. 一次 commit:`unarchive: #<ch> by @<author>`
7. push-with-retry

`archived_via: manual` 的卡片留在 `archive/channels/<ch>/cards/` —— 此时 channel meta 回 active,但 archive/channels/<ch>/ 仍有 cards/ 子目录挂着,是合法状态。`list_archived_cards` 仍能看到它们。

### 4.3 archive_card

在现有 mv 流程基础上,改 yaml 时**同时 set `archived_via: manual`**。

### 4.4 unarchive_card

在现有 mv 流程基础上,改 yaml 时**同时 unset `archived_via`**。现有"refuse to unarchive into an inactive channel"guard 保留 ─ 实际上限制了 `unarchive_card` 永远只能针对 `archived_via: manual` 的卡片(`channel` 的卡片必须走 `unarchive_channel`)。

### 4.5 list_cards / list_archived_cards

- `list_cards`:行为不变。扫 active channels(`s.channels.keys()` 或 daemon 的 `channels/` 目录),返回 `channels/<ch>/cards/` 下的卡片。自然不包含 archived(无论 via channel 还是 via manual)。
- `list_archived_cards`:行为不变。扫 `archive/channels/*/cards/`。**现在能看到所有 archive,无论 via channel 还是 via manual**。前端需要按 `archived_via` 区分时自行渲染。

### 4.6 listChannels / listArchivedChannels

不变。channel 可见性完全由 channel meta 文件位置决定。

## 5. Migration:启动 reconcile

### 5.1 触发时机

- Rust daemon:`AppState::new` 完成后、handler loop 开始前,跑一次 `reconcile_orphan_cards`
- frontend daemon-web:Web Worker boot(`setState` 加载 workspace 后),跑一次 `reconcileOrphanCards`

两端逻辑等价(共用同一份 git 仓库)。先跑的一端做实际 mv + commit;后跑的一端 fetch/rebase 后扫不到孤儿,无 op。

### 5.2 算法

```
For each entry in channels/:
  if entry is a directory (not a *.meta.yaml file):
    channel_name = entry name
    if NOT exists(channels/<channel_name>.meta.yaml)
       AND exists(archive/channels/<channel_name>.meta.yaml)
       AND exists(channels/<channel_name>/cards/):
      # 孤儿确认
      For each card_id in channels/<channel_name>/cards/:
        card_meta_path = channels/<channel_name>/cards/<card_id>/card.meta.yaml
        if exists(card_meta_path):
          load yaml, set archived_via = channel, save yaml
          git mv channels/<channel_name>/cards/<card_id>/ archive/channels/<channel_name>/cards/<card_id>/

If any orphan migrated:
  rmdir empty channels/<channel_name>/ dirs
  commit "chore: reconcile orphan cards under archived channels"
  push-with-retry
Else:
  no commit, no push
```

特性:
- **幂等**:无孤儿时不做任何写操作,无 commit;下次启动同样 no-op
- **单 commit**:无论一次扫到多少孤儿,合并为一个 commit
- **失败可重试**:中途 panic / push 冲突 → 下次启动重新扫,自然恢复(只要 working tree 没有 dirty leftover)
- **不破坏 archive_card 历史卡片**:它们的物理路径已经是 `archive/channels/<ch>/cards/<id>/`,reconcile 扫的是 `channels/<archived>/cards/`,不会重复处理

### 5.3 单个 clone 内的多次启动

仓库共享但 clone 各自启动。Clone A 启动后扫到孤儿 → mv + commit + push;Clone B 启动后先 pull 拿到 A 的 commit → 扫不到孤儿 → no-op。

### 5.4 多 clone 并发启动

两个 clone 同时启动并 reconcile → 都 commit 一份(内容相同) → push 时一边赢,另一边走 rebase + retry(沿用现有 push-with-retry)。rebase 后第二个 commit 变成 no-op(diff 为空) — 这种 empty commit 我们**不 push**(reconcile 函数在 commit 前先 check `git diff --cached` 是否为空,空则 abort)。

## 6. 原子性 & 失败处理

`archive_channel` 的多文件 mv + yaml 改动**单一 commit**,跟现有 push-retry pattern 兼容:fetch → rebase → push 重试 N 次(channel.rs 已有逻辑)。中途 panic:working tree 可能 dirty 但 commit 未生成,下次相同操作会从一致状态重启。具体失败模式 + 恢复路径在 01-plan.md 实施时根据 daemon 现有 sync 行为再 ground 一次。

`unarchive_channel` 和 reconcile 同理。

## 7. 测试策略

### 7.1 frontend daemon-web (vitest)

`handlers.test.ts` 新增:
- `archiveChannel` 把 active cards 全部 mv 到 archive,yaml 字段 set 为 channel
- `archiveChannel` 不动已 `archived_via: manual` 的卡片
- `unarchiveChannel` 只 mv 回 `archived_via: channel` 的卡片;`manual` 的留 archive
- `archiveCard` set yaml `archived_via: manual`
- `unarchiveCard` unset yaml `archived_via`
- `reconcileOrphanCards` 扫到孤儿 → mv + commit,字段 set 为 channel
- `reconcileOrphanCards` 无孤儿 → no commit
- `listArchivedCards` 返回两种 archive 来源的卡片

### 7.2 Rust daemon (cargo test)

`crates/gitim-daemon/tests/` 对应 archive flow + reconcile 的集成测试。沿用现有 archive_channel test 模式,新增:
- `archive_channel` 带 cards 的场景
- `unarchive_channel` 选择性复活
- 启动 reconcile(在 daemon 启动测试里 seed 孤儿目录,验证启动后自动迁移)

### 7.3 跨端契约一致

frontend daemon-web 和 Rust daemon 在同一仓库下行为一致 —— 通过共同的"目录布局 = 真相"不变量保证。无需 cross-component 测试。

## 8. Scope

**In scope**:
- `gitim-core::CardMeta` 加 `archived_via` 字段
- `gitim-daemon` 的 archive_channel / unarchive_channel / archive_card / unarchive_card handler 改动
- `gitim-daemon` 启动时 reconcile_orphan_cards 调用
- `products/gitim/frontend/src/daemon-web/handlers.ts` 的 archiveChannel / unarchiveChannel / archiveCard / unarchiveCard 改动
- `products/gitim/frontend/src/daemon-web/worker.ts`(若有)的 boot-time reconcile 调用
- `products/gitim/frontend/src/lib/types.ts` 的 `Card` interface 加 `archived_via`
- 测试覆盖(frontend + Rust)
- `CLAUDE.md` Current Orientation 一句话提及新机制

**Out of scope**:
- DM archive(无 cards 概念)
- Board archive(`board.meta.yaml` 是 per-user 资产,独立 lifecycle,需单独 design)
- Card archive 的 UI 调整(WebUI 当前已通过 polling fix 看到正确的 active cards 列表;archived cards 面板的"区分 via channel vs via manual"是后续可选 enhancement,本计划不强制)
- legacy `gitim-runtime` 测试中可能存在的 seed 孤儿目录(测试 fixture 不动,reconcile 启动时自然处理)

## 9. PR 顺序

frontend daemon-web 和 Rust daemon 可分两个 PR,**frontend 先 ship**(webui v2 用户多,daemon-web 在浏览器里跑得最频繁,reconcile 落地最先生效)。前提:`CardMeta` 的 `archived_via` 字段必须在两端读为 optional,这样 PR 1 落地后 PR 2 没合并前不会因 yaml 字段缺失而 crash。

实现上,可在同一 PR 改两端 —— scope 不算大(几百行),review 一次性更高效。最终顺序留给 implementation plan 决定。
