# Agent Routing — 需求共识

> Brainstorming 输出。仅 design / requirements，不含实现步骤。
> 下一阶段(writing-plans)产 `01-plan.md`。

Status: APPROVED
Date: 2026-05-17
Review trail:
- 2026-05-17 brainstorming(claude): 4 轮迭代收敛
  - 用户初始提出"@ 默认" → 否决(用户会忘 @)
  - "thread continuity / channel primary / coordinator agent / self-relevance" 四方案 → 用户选简化版
  - "in-memory thread participants index" → 用户否决(无限增长、无淘汰)
  - 最终收敛到 3 条静态规则,daemon 侧 union 计算,无任何持久索引

---

## 背景

**当前状态**:
- `gitim-daemon` 的 `poll.rs` 做 channel-membership filter:只有 channel 成员能 poll 到该 channel 的消息
- `gitim-runtime::agent_loop::format_changes_as_prompt` 做两层 runtime filter:
  1. 跳过 `author == self_handler`(避免自己回自己)
  2. `body.contains(&self_mention)` —— 被 @ 才回应
- 结果:**没被 @ 的消息每个 agent 都会处理一次**

**触发场景**(用户自述):
> "在超过两个 AI 同时在频道的时候,如果我发一条消息,那两个 agent 就会同时接到消息开始各自处理,然后各自回复,然后再被触发。其实感觉多个 agent 收到消息各自开始处理不太像个合理的方案。"

→ 核心需求:**channel 里多 agent 在场时,新消息只通知"相关方",不再 N agent 全部 fan-out**。

**Cascade 严重程度**:N agent 在场,1 条用户消息触发 N 次 LLM call;每次回复又作为新消息再触发 N-1 个其他 agent(因为没被自己作者跳过)。即使每个 agent 第二轮才决定"这条不关我事",已经多烧了至少 N × turn 数的 token。

---

## 收敛过程的关键否决

**为什么不是"默认 @ 触发"**:用户会忘 @ —— "偶尔会忘记艾特就白发了"。把 @ 当默认路由机制有硬伤:消息可能完全不被任何 agent 处理。

**为什么不是 daemon 内存索引(`channel → thread_root → Set<agent_handler>`)**:无限增长 + 无淘汰边界。在协议外面又挖一份要维护的状态,跟 GitIM "文件就是真相"的设计哲学相悖。

**为什么不是"thread 最近发言 agent"**:用户明确选了"thread 所有回复者"语义。

**为什么不是 sidecar 文件存 thread 参与者**:用户选定 3 规则后,parent-chain walk 即可派生,无需持久化任何状态。Sidecar 思路保留为 v2 备选(如果未来需要"全子树参与者"语义)。

---

## 共识 Premises

### P1 — 三条静态路由规则,union 命中即送

新消息的 recipients = 以下三集合的 union:

1. **群主** —— `ChannelMeta.created_by`
2. **Parent chain 上的 author** —— 从新消息 `point_to` 一路上溯,沿途所有 author
3. **显式 @mention** —— `Message.mentions`

去重 + 稳定排序(按 handler 字符串)。**没有第 4 条规则**,没有 channel-级覆写,没有 per-agent opt-out。

### P2 — 适用范围:agent only,human 不受影响

Routing filter 只在 agent 消费侧生效。Human(WebUI / CLI)忽略 recipients 字段,继续看到 channel 里全部消息。

Cascade 是 agent 特有问题(agent 会自动回复 → 触发其他 agent),human 不存在这问题,且 human 想看群里全部讨论是合理需求。

### P3 — 计算位置:daemon 侧统一计算

`compute_recipients` 在 daemon 的 poll 路径里跑一次,每条新消息附上 `recipients: Vec<Handler>`。Runtime 只做无脑过滤"self ∉ recipients 就 skip"。

理由:
- 策略集中,以后改路由规则只动 daemon 一处
- 计算只跑 1 次,不管下游有多少 agent poll
- Daemon 本来就持 `thread_cache`,走 parent chain 比 runtime cache 历史消息更快

### P4 — 复用 `created_by` 作为群主,不加新字段

`ChannelMeta` 已有 `created_by`。不引入新的 `owner` 字段。群主**不可转让**,创建时定死。

未来若出现"创建者离开需要换主响应"的真实需求,再加 `owner: Option<Handler>` 并提供转让命令。v1 不预判。

### P5 — Wire format 用 wrapper 而非扩 `ThreadEntry`

```rust
pub struct PollEntry {
    #[serde(flatten)]
    pub entry: ThreadEntry,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<Handler>,
}

pub struct PollChange {
    pub channel: String,
    pub kind: ChangeKind,
    pub entries: Vec<PollEntry>,  // was Vec<ThreadEntry>
}
```

`ThreadEntry` 保持"磁盘上的消息"语义;`PollEntry` 是"poll 时计算附带的元数据"。Concerns 分清,`recipients` 不混进磁盘解析层。

### P6 — DM 频道绕过 3 规则

DM 频道(路径 `dm/<a>--<b>.thread`)的每条消息 `recipients = [member_a, member_b]`,跳过 compute_recipients 主流程。

理由:DM 是 2 人对话,不存在 fanout 问题,而且 DM 漏发会让对方一脸懵。

### P7 — 删掉 runtime 现有的 body mention 扫描

新逻辑替换旧的 mention 字符串检查。Mention 语义的定义权完全交给 daemon (`compute_recipients` 内部读 `Message.mentions`)。

行为变化:之前 runtime 是"被 mention 才回",现在是"recipients 包含我就回"。**严格扩大** —— 之前会回的现在也会回,但之前不会回的现在也可能会回(owner / parent-chain 接续场景)。

---

## compute_recipients — 算法

```
fn compute_recipients(
    new_message: &Message,
    channel_meta: &ChannelMeta,
    thread_entries: &[Message],
    channel_path: &Path,
) -> Vec<Handler> {
    // DM 短路
    if is_dm_channel(channel_path) {
        return channel_meta.members.clone();
    }

    let mut recipients: BTreeSet<Handler> = BTreeSet::new();

    // Rule 1: 群主
    if !channel_meta.created_by.is_empty() {
        recipients.insert(channel_meta.created_by.clone());
    }

    // Rule 2: parent chain ancestors
    let mut cursor = new_message.point_to;
    let mut visited: HashSet<u64> = HashSet::new();
    while cursor != 0 && !visited.contains(&cursor) {
        visited.insert(cursor);
        let Some(ancestor) = thread_entries
            .iter()
            .find(|m| m.line_number == cursor) else { break };
        recipients.insert(ancestor.author.clone());
        cursor = ancestor.point_to;
    }

    // Rule 3: explicit mentions
    for handler in &new_message.mentions {
        recipients.insert(handler.clone());
    }

    recipients.into_iter().collect()  // BTreeSet 顺序天然 sorted
}
```

复杂度:O(parent_chain_depth × thread_entries) 最坏(线性扫匹配)。typical 深度 <20、thread 几十条,日常路径几百次比较,完全在 daemon hot path 预算内。若以后真碰到深 thread,可以加个 `line_number → entry` 的 HashMap 索引,但 v1 不做。

---

## Edge Cases & Error Handling

| 场景 | 处理 |
|---|---|
| `created_by` 缺失或空字符串 | 走 empty recipients fallback(见下) |
| Parent chain cycle | `visited` set 兜住,返回当前已收集的集合 |
| `point_to` 指向不存在的行 | break,recipients 算到这步为止 |
| Recipients 含已离开 channel 的 handler | 不剔除 —— 该 handler 不在 polling,留着无害,保持函数纯粹性 |
| Agent 自己在自己消息的 recipients 里 | 由 `author == self_handler` 这一条先 skip,recipients 检查走不到 |
| 消息发出时 agent 还不是成员,后来加入 | daemon 外层 membership filter 已经拦住,这条历史消息根本不出现在 agent poll 结果里 |

**Empty recipients fallback**:
- 正常输入下 `compute_recipients` 至少返回 `[created_by]`
- 若返回空(旧 daemon 还没升级 wire 上 recipients 缺省为 `[]`,或 daemon bug):
  - daemon 侧:`warn!` 日志 + channel + line_number
  - runtime 侧:**降级为广播**(跳过 recipients 检查,只走自作者跳过)。理由:漏发比错发更糟糕,empty 意味信息不全,宁可 fanout 不可静默丢消息

---

## 测试策略

### `gitim-core::recipients` 单测(纯函数)
- root 消息无 mention → `[created_by]`
- root 消息 @alice → `[created_by, alice]` 去重排序
- 三层 thread 回复 → `[created_by, 中间 author, root author]`
- DM 频道 → `[member_a, member_b]`
- Cycle 自指 `point_to == line_number` → 不死循环,返回正常集合
- `point_to` 指向不存在的行 → 不 panic,返回当前已收集
- @ 自己 → recipients 含自己(后续由 self-author 跳过处理)

### `gitim-daemon::handlers::poll` 集成测
- 真实文件 + ChannelMeta,验 `PollChange.entries[i].recipients` 计算正确
- 老格式 wire(`recipients` 字段缺省)反序列化成空 `Vec<Handler>`

### `gitim-runtime::agent_loop` 集成测
- 3 agent 同 channel,user @alice 发消息 → 只有 alice 触发 LLM 调用
- agent A 回复后 agent B 在 thread 里 @A → A 触发(parent chain + mention 双中),B 不触发
- Empty recipients wire 模拟 → runtime fallback 为广播(所有 agent 触发)

### 性能 / 非测试项
- Parent chain 深度 <20 典型,极端 1000 行也只是一次线性扫,daemon hot path 可忽略,不做 benchmark
- 不写部署顺序文档:daemon + runtime 通过 `update-and-restart` 一起换,无 split-version 风险

---

## 非目标(Non-goals)

明确**不**在本 spec 范围、不在 v1 实现:

1. **Agent-agent cascade 在长对话里的深度收敛** —— A↔B 在 parent chain 里互相点对方,routing 仍会双方都触发(这是 feature:他们在对话中)。要收敛深度需 turn-cap 或 per-agent `responds_to` 配置,是独立改造线。
2. **群主转让 / `owner` 字段独立化** —— `created_by` 直接当群主,immutable。
3. **路由策略的 channel 级覆写**(e.g. "这个 channel 关掉 rule 2")。
4. **Per-agent opt-out**(e.g. "我只接 @ 不接 owner")—— 通过 me.json 一类机制。
5. **"全子树参与者" 语义**(thread 任意 branch 的 author 都通知)—— 当前 parent-chain 语义覆盖"你这条 reply 在跟谁的对话里",平行 branch 的 agent 在它们自己的对话里。
6. **WebUI 展示 recipients** —— WebUI 完全忽略该字段,行为不变。未来若想"highlight 给我的"再加。
7. **Rollback 路径** —— daemon + runtime 同步升级,wire 向后兼容(empty → 广播 fallback)足够 cover 中间态。

---

## 影响面

**修改**:
- `gitim-core/src/types/poll.rs`(或同等位置)—— `PollEntry` 新类型
- `gitim-core/src/recipients.rs`(新文件)—— `compute_recipients` 纯函数
- `gitim-daemon/src/handlers/poll.rs` —— 构造 `PollChange.entries` 时调 `compute_recipients`
- `gitim-runtime/src/agent_loop.rs::format_changes_as_prompt` —— 加 recipients filter,删 body mention 扫描

**不修改**:
- `ChannelMeta` schema
- 任何 `.thread` / `.meta.yaml` 文件格式
- WebUI / CLI 任何前端代码
- sync_loop / gitim-index / archive / DM 创建流程

**不需迁移**:
- 现有 channel 都有 `created_by` —— 规则 1 即刻生效
- 现有消息都有 `point_to` —— 规则 2 即刻生效
- Mention 解析在 parse 时已经完成 —— 规则 3 即刻生效
- 历史消息的 recipients **不回填**,只对升级后新消息生效(`compute_recipients` 在 poll 路径调,历史消息 poll 不出来)

---

## Open Questions

无。

(若 plan 阶段发现新问题,补到此节,并标注哪条 premise 受影响。)
