# 02 — daemon API:archive 操作 + leave event

> 对应 [01-plan.md](01-plan.md) Part A。daemon 是本次工作的核心层,9 个 task。

## A.1 — Request enum + 路由

**文件**:
- [crates/gitim-daemon/src/api.rs](../../../crates/gitim-daemon/src/api.rs)
- [crates/gitim-daemon/src/handlers/mod.rs](../../../crates/gitim-daemon/src/handlers/mod.rs)

**改动**:Request enum 新增 7 variant + handle_request 路由
- `ArchiveUser { handler, author }` — `"archive_user"`
- `UnarchiveUser { handler, author }` — `"unarchive_user"`
- `ArchiveDm { peer, author }` — `"archive_dm"`
- `UnarchiveDm { peer, author }` — `"unarchive_dm"`
- `ListArchivedUsers` — `"list_archived_users"`
- `ListArchivedDms { author }` — `"list_archived_dms"`(只列 caller 参与的)
- `DepartUser { handler }` — `"depart_user"`(burn 编排专用,无 author,daemon 系统级动作)

**对称参考**:`ArchiveChannel` / `UnarchiveChannel`

**验收**:
- 7 个 variant JSON 反序列化通过
- handle_request 正确 dispatch 到对应 handler
- 字段命名/风格与 ArchiveChannel 一致

**依赖**:无(先决)

---

## A.2 — archive_user / unarchive_user / list_archived_users

**文件**:[crates/gitim-daemon/src/handlers/user.rs](../../../crates/gitim-daemon/src/handlers/user.rs)(新建)

**职责**(对称参考 [crates/gitim-daemon/src/handlers/channel.rs](../../../crates/gitim-daemon/src/handlers/channel.rs) 的 archive_channel / unarchive_channel):
- archive_user:验证 active 存在 → `git mv users/<h>.meta.yaml → archive/users/<h>.meta.yaml` → 单 commit + push retry
- unarchive_user:对称反向,验证 archive 路径存在
- list_archived_users:扫 `archive/users/*.meta.yaml`

**commit message**:`archive: depart user @<handler>` / `archive: restore user @<handler>`

**验收**:
- archive 后 list_users 不见,list_archived_users 见
- unarchive 后回到 active,list_users 见
- 不存在的 user / 已 archive 的再 archive → 明确错误
- 失败时 git mv rollback(对称 channel.rs:370 模式)

**依赖**:A.1

---

## A.3 — archive_dm / unarchive_dm / list_archived_dms

**文件**:[crates/gitim-daemon/src/handlers/dm.rs](../../../crates/gitim-daemon/src/handlers/dm.rs)(新建)或扩 send.rs

**职责**:
- 解析 sorted-pair `<min(a, b)>--<max(a, b)>.thread`
- archive_dm:`git mv dms/<a>--<b>.thread → archive/dms/<a>--<b>.thread`
- unarchive_dm:对称
- list_archived_dms:扫 `archive/dms/*.thread`,filter caller handler 在文件名中

**关键边界**:
- 单方触发即生效(决策 B1),无对端 confirm
- caller author 走现有 resolve_author

**验收**:
- archive_dm peer=bob 后 dms/<sorted>.thread 在 archive/dms/
- unarchive_dm 回 active
- 不存在的 DM / 已 archive 的再 archive → 明确错误

**依赖**:A.1

---

## A.4 — depart_user 复合 API(幂等多 commit)

**文件**:[crates/gitim-daemon/src/handlers/user.rs](../../../crates/gitim-daemon/src/handlers/user.rs)

**职责**:实现 [01-plan.md](01-plan.md) "Agent burn 工作流" 步骤 3 — Phase 1-4 串行,各 phase 独立 commit。

**phases**:
1. 终态判断:`archive/users/<h>.meta.yaml` 存在 → return success
2. **Phase 1**(写 leave events):扫 active threads + archive/dms/* 找 author=handler 的 thread,逐 thread append `[L<n>][@<h>][<ts>] leave-workspace`(末尾已是 alice 的 leave-workspace 则 skip)。每个 thread 一个 commit,复用 send 路径的 retry/rebase/renumber
3. **Phase 2**(归档 DMs):扫 dms/*--handler 与 dms/handler--*,git mv 到 archive/dms/(已 archive 跳过)。每个 DM 一个 commit
4. **Phase 3**(清 channels meta):扫 channels/*.meta.yaml,members 含 handler 的移除该条目(已无则 skip)。每个 channel 一个 commit
5. **Phase 4**(归档 user entry):`git mv users/<h>.meta.yaml → archive/users/`(终态)

**关键约束**:
- 任 phase commit/push 失败 → daemon return error,**不回滚已成功的 phase**(中间态等同 stop 状态,良性)
- 重试时从头跑,各 step 用 skip 条件跳过已完成
- commit author = handler 自己(复用现有 author 推断模型,daemon 直接构造 commit author = handler 即可,因为这是系统级动作 — handler 已 abort,不可能由 handler 自己签名,daemon 代签)
- event token:`leave-workspace`(开放问题 3 决策)

**验收**:
- happy path:alice 在 #dev / #ops 各发过言 + 跟 bob 有 DM + alice 在 #dev members → burn alice → #dev / #ops thread 末尾各 1 行 `@alice leave-workspace`,dms/alice--bob.thread 在 archive/dms/,users/alice 在 archive/users/,#dev meta members 不含 alice
- 幂等:已完成时再调 return success,**无新 commit**
- 半态恢复:Phase 1 写完 5/10 thread 后 abort → 重试 → skip 前 5 + 完成剩下 + Phase 2-4 走完
- 零发言 agent:Phase 1 0 thread 匹配,直接 Phase 2-4

**依赖**:A.2 / A.3 提供共享 helper(git mv 包装、archive 路径检测),但 A.4 内部直接做 git mv 不调 A.2/A.3 RPC

---

## A.5 — write 拦截升级

**文件**:
- [crates/gitim-daemon/src/handlers/send.rs](../../../crates/gitim-daemon/src/handlers/send.rs)
- [crates/gitim-daemon/src/onboard.rs](../../../crates/gitim-daemon/src/onboard.rs)(handler 重用拒绝)

**改动**:
- handle_send DM 分支:写入前 stat archive/dms/<sorted>.thread,存在 → "DM with @<peer> is archived"
- handle_send / 任何 author write:verify caller 不在 archive/users/,是 → "user @<h> is departed"
- onboard / register_user / add_agent:handler 在 archive/users/ → "handler @<h> is reserved (previously departed)"(开放问题 2 决策:不允许重用)

**验收**:
- send 到 archived DM → "is archived"
- archived user 不能 author send(防线 2;runtime 已 kill 它,这里是 daemon 兜底)
- add_agent with archived handler → 拒绝

**依赖**:A.2 / A.3

---

## A.6 — read fallback

**文件**:相应 list / read handler

**改动**:
- handle_list_users:默认只列 `users/`,新增 `include_archived: bool` 参数(对所有 caller 一致,P2.a)
- handle_read DM 路径:active 不存在时 fallback `archive/dms/<sorted>.thread`,响应附 `archived: true`(对称 channel 已有 fallback)

**验收**:
- list_users 默认不见 archived
- include_archived=true 全列出
- read archived DM 正常返回 + `archived: true`

**依赖**:A.2 / A.3

---

## A.7 — poll diff 处理

**文件**:[crates/gitim-daemon/src/handlers/poll.rs](../../../crates/gitim-daemon/src/handlers/poll.rs)

**改动**:
- thread 出现在 `archive/dms/<sorted>.thread` → 标记 `dm_archived` 事件(对称现有 [poll.rs:207](../../../crates/gitim-daemon/src/handlers/poll.rs:207) `archive/channels/` 处理)
- thread 出现在 `archive/users/<h>.meta.yaml` → 标记 `user_archived` 事件
- **不**过滤 archived user 在 active thread 里的旧消息(决策 A2)— default 行为,verify 即可

**验收**:
- archive_dm 后 poll 返回 dm_archived event
- archive_user 后 poll 返回 user_archived event
- 旧消息(author 已 archived)在 poll 输出中正常出现(不被 filter)

**依赖**:A.2 / A.3

---

## A.8 — 测试套件

**新建测试**:
- `crates/gitim-daemon/tests/archive_user_test.rs` — A.2 + A.5 + A.6 + A.7 user 相关
- `crates/gitim-daemon/tests/archive_dm_test.rs` — A.3 + A.5 + A.6 + A.7 dm 相关
- `crates/gitim-daemon/tests/depart_user_test.rs` — A.4 全套

**covered cases**(P1.b 全部 4 case + 设计决策验证):
- happy path / 错误 case(per task 验收)
- depart_user 幂等性
- depart_user 半态恢复
- 多 agent 并发 burn(走 send retry/rebase 机制)
- 零发言 agent burn
- unarchive_user 后 thread 旧的 leave event **不抹**(audit trail 保留)
- 跨 clone fetch 同步行为正常

**测试节奏**:scoped `cargo test -p gitim-daemon`(依据 CLAUDE.md 测试节奏)

**依赖**:A.1-A.7 全部

---

## A.9 — 性能 baseline

**新建**:`crates/gitim-daemon/tests/depart_user_perf.rs`(可标 `#[ignore]` 手动跑)

**setup**:1k synthetic threads,alice 在其中 50% 各发 1-3 条消息

**衡量**:`depart_user("alice")` 端到端延迟

**判定**:
- ≤ 500ms → pass,合并
- > 500ms → 不阻塞 v1 merge,开 follow-up plan `docs/plans/<date>-archive-perf-optimization/` 跟进(候选方案:用 [gitim-index](../../../crates/gitim-index) author 反查)

**依赖**:A.4 完成

---

## 整体依赖图

```
A.1 (api routing)
  ├─ A.2 (user archive)  ─┐
  ├─ A.3 (dm archive)    ─┼─ A.4 (depart_user)  ─┐
  │                       ├─ A.5 (write 拦截)    │
  │                       ├─ A.6 (read fallback) ├─ A.8 (测试)
  │                       └─ A.7 (poll diff)     ├─ A.9 (perf baseline)
```

**并行机会**:A.2 / A.3 可同步开发(都依赖 A.1)。A.5 / A.6 / A.7 在 A.2 / A.3 后可同步。A.4 单独串行,因为依赖 archive helper。
