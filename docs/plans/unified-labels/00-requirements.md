# Unified Labels Space — 需求共识

> Brainstorming(/office-hours) 输出。仅 design / requirements，不含实现步骤。
> 下一阶段 (writing-plans / plan-eng-review) 产 `01-plan.md`。

Status: DRAFT
Date: 2026-05-27
Review trail:
- 2026-05-27 brainstorming(claude opus-4-7): 8 轮迭代收敛
  - 用户初始动机:Card 已有 self-styled labels + 为 Flow 节点匹配 / Board 个人展示做铺垫
  - 用户否决"first-class registry 文件实体" → "勿增实体,AI 拿 API 打"
  - 用户否决"Flow 强制 routing" → "做信息位,让协调 agent 拉人时参考"
  - 用户接受 self-claim only、card-create assignee suggest、字段命名向 labels 收敛
- 2026-05-27 spec-review(general-purpose subagent,fresh context): 6.5/10 → 修订
  - C1 修:`handlers/card.rs` 路径错,实际是 top-level `card_handlers.rs`
  - C2 修:`AgentsWithLabels` 加 P5b 显式说明 disk read 模型(SharedState.users 只是 handler 名 cache)
  - C3 修:`archive/users/` 排除范围加入 P4 + edge cases
  - C4 修:card-suggest 在 commit_lock 内外的位置在 P5 显式定
  - I1+I6+I8 修:`gitim board set tags` 兼容策略 + CLI 命令 naming 锁在 P2 / P10
  - I2 修:重述 P4 self-claim 的 enforcement 机制(daemon-per-clone owner)
  - I3 修:LabelsList 不存在 handler 返回 404,跟 `ensure_known_user` 现有 pattern 一致
  - I4 修:`required_labels` 验证错误用共享 `LabelError`,FlowError 不必新加 variant
  - I5 修:label char set vs handler char set 关系加 footnote
  - I7 修:`onboard.rs::register_user` 加入影响面表(struct literal 需要补 `labels: vec![]`)
  - Open Questions 收敛:Q1/Q3/Q5 锁定到 requirements,只剩 Q2/Q4 留给 plan

---

## 背景

**现状盘点**:

| 对象 | 字段 | 字段名 | 上限 | 字符集 | 位置 |
|---|---|---|---|---|---|
| `CardMeta` | `labels: Vec<String>` | **labels** | 10 个 / 32 char | `a-z 0-9 - _` | `crates/gitim-core/src/types/card.rs:79` |
| `BoardMeta` | `tags: Vec<String>` | **tags** | 20 个 / 32 char | `a-z 0-9 - _` | `crates/gitim-core/src/types/board.rs:70` |
| `UserMeta` | — | — | — | — | `crates/gitim-core/src/types/meta.rs:11` |
| `ChannelMeta` | — | — | — | — | `crates/gitim-core/src/types/meta.rs:18` |
| `FlowNode` | — | — | — | — | `crates/gitim-core/src/flow/types.rs:71` |

**问题**:

1. **命名分裂** — 同一类概念(labels vs tags),validator 实现几乎一模一样,字段名却不同;两套 ad-hoc 重复代码
2. **Agent 能力无 source of truth** — `UserMeta.role` 是 free-form string,既不是 enum 也不是 list,无法表达"这个 agent 会 rust + frontend + 写文档"
3. **Flow 节点只能点名 handler** — `FlowNode.owner: Option<String>` 必须填具体 handler;无法表达"派给会 rust 的人"
4. **Card.labels 是孤岛** — 已经在用,但没跟任何"能力"概念联动,只能算"批次标签"(sprint-2 / v2 / bug 等)

**触发场景**(用户自述):
> "看到现在 Card 上已经有一些自制的标签;另一方面是想为这个 Flow 做一些预备工作 —— 节点匹配 / 个人展示 / 功能扩展。需要整体地设计一下这个场景。"

→ 核心需求:**建立一个跨 Card / Agent / Board / Flow node 的统一 labels 空间,让 agent 能用 labels-aware 方式推荐 / 匹配 / 筛选,但不引入新的 first-class 实体**。

---

## 收敛过程的关键否决

**为什么不是 first-class registry**(`tags/<name>.yaml` 每文件一 tag,带 description / category / owner):
用户原话:
> "我其实想引入独立实体,但是这种东西做不到编辑归一化或者 rebase 冲突,很可能不好处理 merge conflict。我还在找一种聪明的做法。"

后续补充:
> "我还是倾向于勿增实体。tab 后期应该也是 ai 自己拿 api 打或者管理的,人不一定需要一个列表。"

→ **Labels 永远 embedded 字符串,没有独立文件实体,没有 description,没有 owner**。Agent 通过 LLM 上下文理解 label 语义。

**为什么不是 Flow 强制 routing**(daemon 替节点选 owner、自动 assign):
用户原话:
> "flow 这里我暂时不想做很强制性的吧,还是想主要做一个信息位,让协调的 agent 在拉人的时候能参考一下就行。"

→ **`FlowNode.required_labels` 是 hint,不是 hard match**。Daemon 不替节点 assign owner;coordinator agent 自己用 read API 查候选,自己决定拉谁。

**为什么不是 namespace** (`skill:rust` / `topic:auth`):
char set 改 + yaml 解析风险 + 用户记忆负担。**YAGNI** —— 自然形成 prefix 约定 (`rust`、`frontend-react`、`mobile-ios`) 已经够用。

**为什么不是 labels-as-messages**(label 操作走 IM 消息流,daemon derive view):
跟 P3 "source of truth = `users/<h>.meta.yaml.labels`" 硬冲;derived state 难持久化;调 yaml 看不到当前 labels。**作为未来思考材料留档,v1 不走**。

**为什么不是 workspace owner 给 agent 加 label**:
跟"AI 拿 API 打"哲学冲突;增加 onboard 摩擦。**Self-claim only**,agent 只能改自己 labels。

---

## 共识 Premises

### P1 — Labels 永远 embedded,无独立 registry 文件

不存在 `tags/<name>.yaml` 或类似全局 tag 实体。Labels 是 `Vec<String>` 嵌入到现有对象 yaml(user / card / board / flow node)。

推论:
- 没有 description / category / owner 字段
- Agent LLM 自己从上下文理解 label 语义
- 不存在"列出所有 labels" UI(可选 derive query 给 agent;不给 human 暴露)

### P2 — 字段名统一为 `labels`

| 对象 | 字段 | 变更 |
|---|---|---|
| `CardMeta.labels` | 保持 | 不动 |
| `BoardMeta.tags` | → `BoardMeta.labels` | rename,加 `#[serde(alias = "tags")]` 兼容旧 yaml |
| `UserMeta` | 新增 `labels: Vec<String>` | 默认 empty,serde `#[serde(default)]` |
| `FlowNode` | 新增 `required_labels: Vec<String>` | 默认 empty,serde `#[serde(default, skip_serializing_if = "Vec::is_empty")]` |
| `ChannelMeta` | — | 不动(v1 无需 channel-level labels) |

**`gitim board set` 的 `field` arg 同步变更**(对 LLM-facing API 兼容):
- 现状:`gitim board set <field> <value>` 文档化 `field ∈ {status, summary, tags}`(见 `crates/gitim-agent-provider/src/prompts.rs:501`)
- v1:`set_board_field` 实现内同时接受 `"tags"` 和 `"labels"` 作为 field 名(两者都路由到 `labels` 字段);prompt 默认表述切到 `labels`,并加一行"`tags` 为兼容别名,等价于 `labels`"
- v2:从 prompt 撤掉 `tags` 表述,实现仍保留 alias 一两个版本

### P3 — Agent 能力 source of truth = `users/<handler>.meta.yaml.labels`

Identity 在 user meta,board 是展示层。

推论:
- `BoardMeta.labels` 保留为 mirror 字段(human-readable 展示),由 board 编辑路径独立维护;coordinator / card-suggest 路径**不读** board.md,只读 user meta
- 不做 user.meta.yaml ↔ board.md 的自动 sync(避免新的 sync 复杂度);user 自行决定 board frontmatter 要不要写 labels
- 若 board.md 上展示的 labels 跟 user.meta.yaml 不一致,以 user.meta.yaml 为准

### P4 — Self-claim only

Agent 只能改自己的 labels。

- Daemon IPC `labels_add(target_handler, [labels])` / `labels_remove(target_handler, [labels])` 验证 `target_handler == state.me.handler`,否则拒绝(`error_code: "not_self"`)
- Human 走 CLI / WebUI 改自己的 labels,本质同样是 target == state.me.handler
- 不存在"workspace owner 给别人贴 label"的 IPC

**Enforcement 机制(澄清 source of truth):**
GitIM 的部署模型是 **per-clone daemon**: 每个 daemon 进程对应一份 `<clone>/.gitim/me.json`,持有该 daemon 的绑定 handler。**所有 IPC 调用都是发给这个 daemon,该 daemon 写出的 commit author 永远是它自己绑定的 handler**。所以 self-claim 实际上是 IPC 路由 + git author 这两层都天然 enforce 的:
- IPC 层显式 reject `target != state.me.handler`,给客户端一个清晰错误
- 即使 IPC 层被绕过,git author 仍是 daemon 自己 handler;sync 时 daemon 不会用别人身份提交别人的 user.meta.yaml
- `archive/users/<departed>.meta.yaml` 是历史快照(read-only audit),不接受 LabelsAdd/Remove

**scope of read:**
- `LabelsList { target }` 只扫 `users/<target>.meta.yaml`,**不**碰 `archive/users/`
- `AgentsWithLabels { labels }` 只扫 active `users/*.meta.yaml`,**不**包含 archive/departed users
- 这跟 `card_handlers::ensure_known_user`(`crates/gitim-daemon/src/card_handlers.rs:94`)以及 `onboard::register_user` 的 "departed handler 被 archive 隔离" 语义一致

### P5 — Card create 时 daemon 给 assignee suggest

`create_card`(`crates/gitim-daemon/src/card_handlers.rs:138 handle_create_card`)时,daemon 扫所有 active agent 的 `users/<h>.meta.yaml.labels`,找 `agent.labels ⊇ card.labels` 的候选(superset match),response 加 `suggested_assignees: Vec<Handler>`。

- **同步**返回,不走 SSE event
- **客户端决定**是否采纳:WebUI 可以 prompt user 选 / 自动填充 assignee 字段 / 完全忽略;daemon 不替客户端 commit assignee
- `card.labels` 为空时,suggested_assignees 也为空(不做"全 agent 推荐"兜底)
- 排除 `archive/users/` 下的 departed handler

**Scan 位置(澄清 reviewer 的 commit-lock 担忧):**
`handle_create_card` 当前序列是: validate → write `card.meta.yaml`+`discussion.thread` → `add_and_commit_as` → `push_with_retry` → `event_tx.send` → response。
**Scan 放在 `push_with_retry` 之后、response 构建之前**:
- 不延长任何 git 写操作的时间窗口
- 不在 `commit_lock`-held 段内(`handle_create_card` 本来就不持 `commit_lock`,而是依赖 `git_storage` 内部的串行)
- Suggestion 是 advisory,**不**需要跟 card commit 原子一致

**Performance & SoT 模型(P5b):**
- `SharedState.users: RwLock<Vec<String>>` 只缓存 active handler 名,不缓存 yaml 内容
- v1 实现:`compute_suggested_assignees(card_labels)` 每次 lock-read `state.users` → 对每个 handler 同步 fs-read + serde-parse `users/<h>.meta.yaml`,提取 `labels` 字段做集合包含判断
- workspace agent N 数典型 <20、单文件 KB 级 → 总 cost <10ms,可接受
- 走 `tokio::task::spawn_blocking` 包住 fs I/O 避免阻塞 reactor
- 若以后 N 超 100 或调用频繁,加 `SharedState.user_labels: RwLock<HashMap<Handler, Vec<String>>>` cache,在 `on_synced` / 每个 LabelsAdd-Remove handler 末尾失效或更新 — **v1 不做**,作为 future option 文档化

### P6 — Flow 节点 `required_labels` 是信息位,不强制 routing

Daemon 在 flow 启动 / 节点流转时**不计算 routing**,**不去自动 assign owner**,**不阻塞 flow 推进**。

- `required_labels` 仅暴露在 flow node meta 里(via daemon API、frontmatter、WebUI)
- Coordinator agent 自行用 daemon read API (`agents_with_labels`) 查候选,自己决定拉谁
- 若节点 `required_labels` 非空但 coordinator 没找到 match,flow run 不会因此 failed;节点照常等 owner 接管
- v1 不教育 coordinator system prompt(留 v1.5),但 daemon read API 在 v1 必须备好

### P7 — Char set 不动,无 namespace

保持 `a-z 0-9 - _`。不接受 `:`。自然 prefix 约定(`rust`、`frontend-react`、`mobile-ios`),不强制。

**Label vs Handler char set 关系(footnote)**:
- Handler char set: `a-z 0-9 -` (无 underscore;见 `crates/gitim-core/src/types/handler.rs`)
- Label char set: `a-z 0-9 - _` (含 underscore;P9)
- Label 是 handler 的 superset,labels 可以含 `_` 而 handlers 不能,因此**labels 不会跟 handler namespace 视觉混淆**(`agent_helper` 是合法 label,不是合法 handler)
- 这是有意的:让 agent / human 在写 label 时即使写出"看起来像 handler"的字符串也不会引发 mention 误解析

### P8 — Onboard 时 labels 可空

新 user / agent provision 时 `labels` 默认 `[]`。Agent 启动后通过 system prompt 知道"我应该 claim 我擅长的 labels"(prompt 注入留 v1.5,v1 仅文档化 API)。

### P9 — Char set + max_len 统一,max_count 各异

- 字符集:`a-z 0-9 - _`(所有对象一致)
- 单 label 长度上限:32 chars(所有对象一致)
- 单对象 label 数量上限:
  - `CardMeta.labels`: 10(不动)
  - `BoardMeta.labels`: 20(不动)
  - `UserMeta.labels`: 30(新设;给 agent 足够 claim 空间)
  - `FlowNode.required_labels`: 10(新设;跟 card 对齐)

理由:char set + max_len 是 protocol invariant(跨对象一致);max_count 反映对象的预期密度,不强求统一。

### P10 — CLI 命名 lock,`gitim card label` 跟 `gitim labels` 长期共存

- `gitim labels add/remove/list/match <args>` — 操作 caller 自己的 `users/<self>.meta.yaml.labels` 或全局 query;new in v1
- `gitim card label add/remove <card-id> <label>` — 操作具体 card 的 `card.meta.yaml.labels`;**保留**,不 deprecate
- 命令在动词前的 noun 区分目标对象(`labels` = self user;`card label` = 具体 card),不混淆
- IPC 命名同步:`LabelsAdd / LabelsRemove / LabelsList / AgentsWithLabels` — 都是 user labels 相关;card label 操作走现有 `CardLabelAdd / CardLabelRemove`
- WebUI / agent prompts 表述也按这个动词区分:"管理你的能力" 用 `gitim labels`;"给某张卡贴标签" 用 `gitim card label`

---

## v1 Scope (Approach B)

### 包含

1. **Schema 改动** (4 处)
2. **Daemon IPC** (4 个 handler)
3. **CLI subcommand** (`gitim labels add/remove/list`)
4. **Card create 路径加 assignee suggest** (read-side only)
5. **Backward compat 给 Board.tags** (serde alias)
6. **WebUI read-only 展示** (agent detail / card 详情显示 labels chip)
7. **Documentation** (系统 prompt 提到 labels API、CLAUDE.md 记 orientation)

### 不包含 (推迟到 v1.5+)

- WebUI labels 编辑器(agent detail 写入路径) — v1 编辑走 CLI
- Card create dialog labels picker — v1 走文本输入或现有 CLI
- Coordinator agent system prompt 注入 labels API 文档 — v1 仅 CLAUDE.md 描述,coordinator 在 v1.5 教育
- Onboard 路径教育新 agent claim labels — v1.5
- Flow 节点 required_labels 在 WebUI 的展示 — v1.5
- `BoardMeta.labels` 跟 `UserMeta.labels` 的自动 sync — 永不(per P3)
- Namespace / `:` 字符 — 永不(per P7)
- 跨对象 labels search 全局 query("找所有带 labels=rust 的 card+agent+board") — v2

---

## Schema 变更详

### `UserMeta`

```rust
// crates/gitim-core/src/types/meta.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMeta {
    pub display_name: String,
    pub role: String,
    pub introduction: String,
    #[serde(default)]
    pub labels: Vec<String>,
}
```

YAML wire format:
```yaml
display_name: Alice
role: backend
introduction: 在 gitim 上写 Rust
labels:
  - rust
  - backend
  - postgres
```

旧 yaml(无 `labels` 字段)→ deserialize 成 `labels: vec![]`,正常工作。

### `BoardMeta` rename

```rust
// crates/gitim-core/src/types/board.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BoardMeta {
    pub version: u32,
    pub handler: String,
    pub updated_at: String,
    pub status: String,
    pub summary: String,
    #[serde(default, alias = "tags")]
    pub labels: Vec<String>,
}
```

`#[serde(deny_unknown_fields)]` 跟 `alias` 配合:旧 yaml `tags: [...]` 仍可读(alias);新 yaml 输出 `labels: [...]`。旧 yaml 读完再写一遍 = 自然 migrate(无需独立 migration tool)。

### `FlowNode`

```rust
// crates/gitim-core/src/flow/types.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,

    #[serde(default)]
    pub needs: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<String>,

    /// 新加。Flow 节点声明的能力需求 — 仅信息位,daemon 不强制 routing。
    /// Coordinator 自行用 agents_with_labels API 查候选。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_labels: Vec<String>,

    #[serde(skip)]
    pub prompt: String,
}
```

旧 flow yaml(无 `required_labels`)→ 默认 empty,行为不变。

### `CardMeta`

不变。已经叫 labels,已经有 validate。

### Shared validator

把 `crates/gitim-core/src/types/card.rs::validate_label` / `validate_labels` 提取到 `crates/gitim-core/src/types/labels.rs`,Card / Board / User / FlowNode 共用。

```rust
// crates/gitim-core/src/types/labels.rs (新文件)
pub const MAX_LABEL_LEN: usize = 32;
pub const CARD_MAX_LABELS: usize = 10;
pub const BOARD_MAX_LABELS: usize = 20;
pub const USER_MAX_LABELS: usize = 30;
pub const FLOW_NODE_MAX_LABELS: usize = 10;

#[derive(Error, Debug)]
pub enum LabelError {
    #[error("label length out of range (1..={1}), got {0}")]
    LengthOutOfRange(usize, usize),
    #[error("invalid char '{0}' in label (allowed: a-z 0-9 - _)")]
    InvalidChar(char),
    #[error("too many labels (max {1}), got {0}")]
    TooMany(usize, usize),
}

pub fn validate_label(label: &str) -> Result<(), LabelError> { ... }
pub fn validate_labels(labels: &[String], max_count: usize) -> Result<(), LabelError> { ... }
```

各对象 validator 调:
```rust
// card.rs
validate_labels(&meta.labels, CARD_MAX_LABELS).map_err(CardError::from)?;
// board.rs
validate_labels(&meta.labels, BOARD_MAX_LABELS).map_err(BoardError::from)?;
// user.rs (新建 validator)
validate_labels(&meta.labels, USER_MAX_LABELS).map_err(UserError::from)?;
// flow validator
validate_labels(&node.required_labels, FLOW_NODE_MAX_LABELS)
    .map_err(|e| FlowError::InvalidNodeField { node: node.id.clone(), inner: e })?;
```

各对象的 Error enum 加 `#[from] LabelError` variant(或包一层带 context),保持 error chain。

---

## Daemon IPC

### `LabelsAdd { target: Handler, labels: Vec<String> }`

- 验证 caller handler == target(`error_code: "not_self"` 若不等)
- 验证 labels 字符集 + 单 label 长度(per P9)
- 验证 union 后总数 ≤ 30(UserMeta cap)
- 读 `users/<target>.meta.yaml` → merge labels(去重) → 写回 → commit(`system@gitim` 不做,author = target handler;复用现有 daemon commit author 逻辑)
- 拿 `commit_lock`,跟其他 writer 串行
- response: `{ok: true, current_labels: [...]}`

### `LabelsRemove { target: Handler, labels: Vec<String> }`

- 同 verify caller 逻辑
- 读 user meta → filter 出 labels → 写回 → commit
- response: `{ok: true, current_labels: [...]}`

### `LabelsList { target: Handler }`

- 不验证 caller(read API,任何 agent 都能查别人的 labels)
- 通过 `ensure_known_user(state, target)` 检查 target 是否在 `state.users` 内(active);不在则返回 `error_code: "unknown_user"`(HTTP 404)
- target 在 active 集合内但 yaml 反序列化 `labels` 为空 → 返回 `{labels: []}`
- 不扫 `archive/users/`;departed handler 视为 unknown
- response: `{labels: [...]}`

### `AgentsWithLabels { labels: Vec<String>, mode: "all-of" }`

- read-only,扫所有 `users/*.meta.yaml`
- mode = "all-of":返回 `agent.labels ⊇ query.labels` 的 handlers
- v1 仅 all-of 模式;未来加 "any-of" / "score-based"
- response: `{handlers: [...]}`

### `create_card` 改造

- 现状:`create_card` 接受 `{title, channel, status, labels, assignee, ...}`,写 card.meta.yaml + thread + commit + push + emit event
- 新加:在 **push 之后、response 构建之前** 调内部 `compute_suggested_assignees(card.labels)`(per P5 "Scan 位置"),返回 `Vec<Handler>`(empty if card.labels empty)
- response 多塞一个 `suggested_assignees: Vec<Handler>` 字段(`CreateCardResponse` struct 加字段)
- 客户端可忽略也可消费,不影响 card 是否被创建

---

## CLI Subcommand

```
gitim labels add <label1> <label2>     # 加到自己的 user.meta.yaml
gitim labels remove <label1> <label2>  # 从自己的 user.meta.yaml 移除
gitim labels list [--handler <h>]       # 默认自己,可指定看别人
gitim labels find <label1> <label2>    # all-of search
```

`gitim labels` 子命令通过 daemon IPC 走,跟现有 `gitim card label add/remove` 模式一致(可后续 deprecate `gitim card label` 收敛到 `gitim labels`,但 v1 不动)。

---

## WebUI 改动(read-only)

- Agent detail 页 (`/agents/<handler>`) 加 labels chip 列表(read-only)
- Card 详情 / hover preview 显示 card.labels chip(read-only)
- 不加编辑入口(v1.5 再做)
- 不在 sidebar / list 页加 filter UI(v2)

---

## Edge Cases & Error Handling

| 场景 | 处理 |
|---|---|
| caller != target 的 LabelsAdd / LabelsRemove | 拒绝,`error_code: "not_self"`,HTTP 403 |
| 同一 label 重复 add | 去重后写入,不报错 |
| Remove 不存在的 label | 静默 no-op,不报错 |
| 单 label 超 32 char / 含非法字符 | 拒绝,`error_code: "invalid_label"`,HTTP 422 |
| Add 后总数超 30 | 拒绝,`error_code: "labels_full"`,HTTP 422 |
| `LabelsList` 查不存在的 handler 或 departed handler | `ensure_known_user` 失败 → 返回 `error_code: "unknown_user"`,HTTP 404(跟现有 daemon 模式一致) |
| `LabelsList` active handler 但 labels 为空 | 返回 `{labels: []}` |
| `AgentsWithLabels` empty query | 返回 empty `{handlers: []}`(避免歧义:不返回"所有 agent") |
| `AgentsWithLabels` 扫到一个 user.meta.yaml 反序列化失败 | 跳过该文件,log warn,继续扫剩余;不让一个坏文件吞掉整个 query 结果 |
| `FlowNode.required_labels` 含 invalid char / 超 10 个 | flow validator 拒绝,error 带 node id context,flow load 失败 |
| `create_card` 期间 fs scan agent labels 报错 | 静默返回 `suggested_assignees: []`,log warn;不让 suggestion 失败影响 card 创建主路径 |
| `LabelsAdd` 后 yaml write 成功但 git commit 失败 | rollback yaml file(写回旧版本),返回 error;避免 commit log 跟 working tree 分裂 |
| `create_card` 没 labels | suggested_assignees = `[]` |
| `create_card` labels 有,但没 agent 全 match | suggested_assignees = `[]` |
| 旧 board.md `tags:` 字段读 | serde alias 兜住,自动 deserialize 成 labels |
| 旧 board.md 写回后变成 `labels:` 字段 | 是的,passive migration;无单独 migration tool |
| `BoardMeta.labels` 跟 `UserMeta.labels` 不一致 | 不 reconcile;以 user meta 为 canonical;board 是 mirror,以 board 编辑路径为准 |
| FlowNode required_labels 非空但无 agent match | flow 不 failed,节点照常等 owner;coordinator 看到 hint 但不强制 |
| sync_loop 拉 origin 时别人也改了 labels | git merge 走现有 conflict resolver(per-file diff);user.meta.yaml 是 line-oriented,labels list 冲突极少 |

---

## 测试策略

### `gitim-core::types::labels` 单测(纯函数)

- `validate_labels(&labels, max_count)` — 各 max_count 边界
- 单 label 字符集 / 长度
- 去重
- Board.tags alias 兼容:旧 yaml `tags: [...]` 反序列化成 `labels: vec![...]`
- 旧 board.md 写回后 yaml 输出是 `labels:` 不是 `tags:`

### `gitim-core::types::user_meta` 单测

- 旧 yaml(无 labels)反序列化成 `labels: vec![]`
- 新 yaml roundtrip

### `gitim-daemon::handlers::labels` 集成测

- `LabelsAdd` self → ok
- `LabelsAdd` non-self → 403 `not_self`
- `LabelsAdd` 超 30 个 → 422 `labels_full`
- `LabelsAdd` invalid char → 422 `invalid_label`
- `LabelsAdd` 重复 → 去重
- `LabelsRemove` not-exist label → no-op
- `LabelsList` 自己 + 别人
- `AgentsWithLabels` empty → empty
- `AgentsWithLabels` all-of match → 正确返回
- 并发两个 add → commit_lock 串行,最终状态正确

### `gitim-daemon::handlers::card::create_card` 集成测

- card.labels = empty → suggested_assignees = []
- card.labels = ["rust"]; agent A labels=[rust,backend], B=[rust,frontend], C=[python] → suggested = [A, B]
- card.labels = ["rust", "backend"]; agent A=[rust,backend], B=[rust] → suggested = [A]
- card.labels = ["rust"]; 无 agent match → suggested = []

### `gitim-cli` smoke test

- `gitim labels add rust` → 成功,user meta 包含 rust
- `gitim labels list` → 输出包含 rust
- `gitim labels find rust` → 输出包含自己 handler

### 性能 / 非测试项

- `AgentsWithLabels` O(N agents × M labels per agent) 线性扫,workspace 内 typical N<20、M<30,microsec 级,不做 benchmark
- 不写部署顺序文档:daemon 升级跟 runtime 一起 via `update-and-restart`;旧 yaml 通过 serde alias 兼容

---

## 非目标 (Non-goals)

明确**不**在 v1 范围:

1. **First-class registry 文件** — `tags/<name>.yaml` 永不
2. **Labels namespace** — `:` 字符永不
3. **Workspace owner / human 给别人 agent 加 label** — self-claim only
4. **Daemon-side hard routing for Flow nodes** — required_labels 是 hint
5. **Card 自动 assign assignee** — daemon 只 suggest,不 commit
6. **WebUI labels 编辑器** — v1.5(read 在 v1,write 在 v1.5)
7. **Coordinator agent system prompt 注入 labels API** — v1.5
8. **Onboard 教育新 agent claim labels** — v1.5
9. **跨对象 labels search global query** — v2
10. **`BoardMeta.labels` ↔ `UserMeta.labels` 自动 sync** — 永不(P3)
11. **Labels score-based / fuzzy routing** — v2
12. **Labels deprecate / 重命名** — v2
13. **Per-channel labels namespace** — v1 cross-workspace flat space
14. **Labels analytics / 使用频度统计** — v2

---

## 影响面

**新增**:
- `crates/gitim-core/src/types/labels.rs` — 共享 validator + 常量
- `crates/gitim-daemon/src/handlers/labels.rs` — 4 个 IPC handler
- `crates/gitim-cli/src/commands/labels.rs` — `gitim labels` subcommand

**修改**:
- `crates/gitim-core/src/types/meta.rs::UserMeta` — 加 `labels: Vec<String>` 字段(`#[serde(default)]`)
- `crates/gitim-core/src/types/board.rs::BoardMeta` — `tags` → `labels` + `#[serde(alias = "tags")]`
- `crates/gitim-core/src/types/board.rs::default_board` — `tags: Vec::new()` → `labels: Vec::new()`
- `crates/gitim-core/src/types/board.rs::set_board_field` — match arm `"tags"` 改为 `"tags" | "labels"`,两者都路由到 `meta.labels`
- `crates/gitim-core/src/types/card.rs::validate_label / validate_labels / MAX_LABELS / MAX_LABEL_LEN` — 提取到 `types/labels.rs`,这里 re-export 保 backward compat(若无外部 caller 则直接删,plan 阶段 grep 决定)
- `crates/gitim-core/src/flow/types.rs::FlowNode` — 加 `required_labels: Vec<String>` 字段
- `crates/gitim-core/src/flow/validator.rs` — 调用 `validate_labels(&node.required_labels, FLOW_NODE_MAX_LABELS)` 验证(复用共享 `LabelError`,不新增 `FlowError` variant — 在 validator 调用站点 wrap 错误带上 node id context)
- `crates/gitim-core/src/flow/parser.rs` — frontmatter 多解析一个 `required_labels` 字段(serde 自动 handle)
- `crates/gitim-daemon/src/card_handlers.rs::handle_create_card` — push 之后、response 构建之前调 `compute_suggested_assignees`,response 加 `suggested_assignees`
- `crates/gitim-daemon/src/onboard.rs::register_user` — UserMeta struct literal 加 `labels: vec![]`(line 395-399)
- `crates/gitim-daemon/src/handlers/mod.rs`(及路由表) — 注册 `LabelsAdd / LabelsRemove / LabelsList / AgentsWithLabels` IPC handler
- `crates/gitim-client/src/lib.rs` — labels 方法暴露给 client
- `crates/gitim-cli/src/main.rs` + 新文件 `crates/gitim-cli/src/commands/labels.rs` — 注册 `gitim labels` subcommand
- `crates/gitim-runtime/src/http.rs` — 暴露 labels API 给 WebUI(GET `/labels/<handler>`、POST/DELETE `/labels/<self>`,以及 `GET /agents-with-labels?labels=a,b`)
- `crates/gitim-agent-provider/src/prompts.rs:501` — `gitim board set` 的 field 文档加 `labels`(主)+ `tags`(legacy alias);加新一段介绍 `gitim labels add/remove/list/match`
- `products/gitim/frontend/src/` — agent detail 页 labels chip; card 详情 labels chip(read-only)
- `CLAUDE.md` 项目根 — orientation 段加 labels 简介

**不修改**:
- `ChannelMeta` schema
- 任何 `.thread` 文件格式
- sync_loop / index / archive 流程
- 现有 `gitim card label add/remove` 命令(保留向后兼容,v2 收敛)

---

## Migration

- **旧 board.md `tags:` 字段**:serde alias 兜底,read 时自动当 labels 用;board 下次被任何 daemon write 会输出 `labels:`(passive migration)。无需单独 migration tool 或 reconcile 步骤
- **旧 user.meta.yaml 无 labels**:serde default empty Vec,行为不变
- **旧 flow yaml 无 required_labels**:serde default empty Vec,行为不变
- **旧 card.meta.yaml**:CardMeta.labels 字段不动,完全 backward compatible

---

## Open Questions

(Q1 / Q3 / Q5 已在 spec-review 后锁定到 requirements 主体,不再 open。)

1. **`labels.rs` 提取后,旧 `card.rs` 的 `validate_label / validate_labels / MAX_LABELS / MAX_LABEL_LEN` 怎么处理** — 倾向于 re-export 保 backward compat,但若没有外部 caller(包括 wasm crate)就直接删。Plan 阶段 grep 决定。
2. **`suggested_assignees` 何时 cap / 怎么排** — v1 不 cap、用 BTreeSet 自动 sorted-by-handler 返回。若 plan 阶段实测 N>20 才考虑加 cap(上限 10)+ ranking(按 `|agent.labels ∩ card.labels|` 降序);现在不预判。

## 已 lock 的 review-decided 项

- **Q1 → P10**: `gitim card label` 跟 `gitim labels` 长期共存,动词前 noun 区分目标对象(`labels` = self user,`card label` = 具体 card),不收敛
- **Q3 → 影响面**: WebUI 走专用 endpoint `GET /labels/<handler>` + `POST/DELETE /labels/<self>` + `GET /agents-with-labels`;不塞进 `GET /agents/:handler`,避免该 endpoint 过度耦合
- **Q5 → P5/P6**: labels add/remove **不**emit SSE event;v1 frontend 不监听 labels 变化,靠 next page load 刷新。WebUI read-only 视图本来就低频访问,polling 都不需要

---

## Next Steps

进入 SOP `plan-eng-review`(`/sop-dev-mode` 自动 chain 到下一阶段),产 `01-eng-review.md`(可能改为 `01-eng-review-findings.md` 对齐 oneshot-timer 风格)。然后 `writing-plans` 产 `02-implementation-plan.md` 走 subagent-driven-development。
