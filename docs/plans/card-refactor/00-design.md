# Card Refactor — 去 Board 化，卡片挂 Channel

**Status**: Design (pending user approval)
**Author**: lewis
**Date**: 2026-04-17

---

## 1. 动机

当前看板功能把 `board` 作为后端一等公民（`boards/<board>/<card-id>/...`）。在产品讨论中发现：

1. **Agent 不需要"看板"这个概念** — agent 在 `card` 里工作，只需要知道"我被分配了哪些 card、它们属于哪个 channel"。看板是**人类的鸟瞰视图**，不是数据模型需要的实体。
2. **Channel 和 Card 的关系应该显式化** — 讨论发生在 channel（时间流、discovery），执行收敛成 card（任务凭证、execution）。让 card 从属于 channel 能让这个流动关系一目了然。
3. **"看板"变成前端自由组合的视图** — 用 `labels` 做自由分类，用户可以按 channel / label / assignee / status 自由聚合出"我的 sprint-2 任务"、"backend-refactor 所有卡片"等视图。无需后端建模。

## 2. 核心决策（已与用户对齐）

| # | 决策 | Lean 理由 |
|---|------|-----------|
| D1 | 后端删除 `board` 概念，`card` 是第一公民 | 数据模型简化；agent 心智模型干净 |
| D2 | Card 必须从属于一个 channel（1:N） | 明确归属；审计链清晰；打开 channel 就能看所有卡 |
| D3 | File layout：`channels/<channel>/cards/<card-id>/{card.meta.yaml, discussion.thread}` | 路径反映从属关系；和 DM 目录风格一致 |
| D4 | `CardMeta` 加 `labels: Vec<String>`（自由 tag）和 `channel: String`（冗余但查询方便） | labels 承担"分组"；channel 字段让 meta 自包含 |
| D5 | `status` 固定三态 `todo` / `doing` / `done` | 受控状态机；灵活性交给 labels |
| D6 | 卡片 `discussion.thread` = "工作日志"（进度/结论/阻塞），**不是聊天** | 聊天回 channel；卡片是沉淀 |
| D7 | 事件 `CardCreated` / `CardStatusChanged` 字段 `board` → `channel` | 语义对齐新模型 |
| D8 | "看板视图"由前端纯聚合实现，零后端参与 | 查询即视图；用户可自定义组合 |

## 3. 数据模型

### 3.1 文件布局

```
channels/
└── <channel-name>/
    ├── <channel-name>.thread            # 既有：channel 消息流
    └── cards/
        └── <card-id>/                    # card-id = YYYYMMDD-HHMMSS-xxx
            ├── card.meta.yaml
            └── discussion.thread         # 卡片工作日志
```

- **约束**：card 必须在已存在的 channel 下创建（`create_card` 时校验 channel 存在）
- **channel 归档/删除语义**：暂不变更（本次重构不动 channel 生命周期）；如果 channel 归档，其 cards 随之"隐于视图"但仍可读

### 3.2 CardMeta 结构

```rust
pub struct CardMeta {
    pub title: String,
    pub channel: String,                // 所属 channel（路径冗余，查询方便）
    pub status: CardStatus,             // todo / doing / done
    pub labels: Vec<String>,            // 自由 tag，前端聚合依据
    pub assignee: Option<String>,       // handler，可选
    pub created_by: String,
    pub created_at: String,             // YYYYMMDDTHHMMSSZ
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CardStatus {
    Todo,
    Doing,
    Done,
}
```

**去掉的类型**：`BoardMeta`（整个结构体）、其默认 `statuses` 函数。

### 3.3 Label 约束

- 字符集：`a-z 0-9 - _`，长度 1–32
- 一张卡最多 N 个 labels（N = 10，防滥用）
- 不维护"全局 label 列表"；label 是隐式，出现即存在
- 前端可从 `list_cards` 结果里 `map -> Set` 得到当前仓库存在的所有 label

## 4. IPC 接口改造

### 4.1 移除

- `create_board`
- `list_boards`

### 4.2 改造

| 方法 | 旧签名 | 新签名 |
|------|--------|--------|
| `create_card` | `(board, title, assignee?, status?, author)` | `(channel, title, labels?, assignee?, status?, author)` |
| `list_cards` | `(board, status?)` | `(channel?, labels?, status?, assignee?)` — 全部 optional |
| `read_card` | `(board, card_id, limit?, since?)` | `(channel, card_id, limit?, since?)` |
| `send_card_message` | `(board, card_id, body, reply_to?, author)` | `(channel, card_id, body, reply_to?, author)` |
| `update_card` | `(board, card_id, status?, assignee?, author)` | `(channel, card_id, status?, labels?, assignee?, author)` |

**注意**：
- `list_cards` 所有参数 optional 后，空参数 = 列出全仓库所有 card（跨 channel）
- `update_card` 加 `labels` 后，传入即**整体替换**（不做增量 add/remove，避免复杂性）
- `status` 字段从 `String` 改 `CardStatus` enum，非法值报错

### 4.3 新增（可选，取决于前端需要）

- `list_labels` → 返回当前仓库存在的所有 label 列表（由 `list_cards` 派生，后端可选择不实现，前端自己 reduce）

### 4.4 Events

```rust
Event::CardCreated { channel: String, card_id: String }
Event::CardStatusChanged {
    channel: String,
    card_id: String,
    old_status: CardStatus,
    new_status: CardStatus,
    author: String,
}
Event::CardMessageAppended {
    channel: String,
    card_id: String,
    line_numbers: Vec<u64>,
}
```

**不复用**既有的 `thread_changed` / `messages_pushed`：卡片消息走独立事件 `CardMessageAppended`。这样既有事件的 `channel` 字段语义保持（= channel 名，用于导航/显示），前端对卡片事件有明确 dispatch 路径，不需要按前缀 sniff。

## 5. CLI 改造

当前 CLI 只暴露 `create-board` / `list-boards` 两个命令（在 `crates/gitim-cli/src/commands/board.rs`）。改造：

| 旧命令 | 新命令 |
|--------|--------|
| `gitim create-board <name>` | **删除** |
| `gitim list-boards` | **删除** |
| — | `gitim card create <channel> <title> [--label ...] [--assignee ...] [--status ...]` |
| — | `gitim card list [--channel ...] [--label ...] [--status ...] [--assignee ...]` |
| — | `gitim card show <channel> <card-id>` |
| — | `gitim card update <channel> <card-id> [--status ...] [--label ...] [--assignee ...]` |
| — | `gitim card comment <channel> <card-id> <body>` |

CLI 子命令用层级：`gitim card <verb>`，避免污染顶级命名空间。

## 6. Runtime HTTP 暴露

当前 `gitim-runtime/src/http.rs` 无任何 board/card 端点。新增：

```
POST /im/cards                     create_card
GET  /im/cards                     list_cards (query: channel, label, status, assignee)
GET  /im/cards/:channel/:card_id   read_card
POST /im/cards/:channel/:card_id/messages  send_card_message
PATCH /im/cards/:channel/:card_id  update_card
```

让 webui-v2 / 未来前端能直接 HTTP 调用。

## 7. 索引层 (gitim-index)

SQLite FTS5 索引需支持：

- 卡片 discussion.thread 的消息也被索引到既有 messages 表
- messages 表新增 `card_id TEXT NULL` 字段：频道消息该字段为 NULL，卡片消息写 card-id
- 频道消息的 `channel` 字段语义不变（就是 channel 名）
- Search API 行为：
  - 默认 `search(...)` 只返回频道消息（`card_id IS NULL`），保持既有行为
  - `search(..., include_cards=true)` 同时返回卡片消息
  - `search(..., channel=foo, card_id=YYYYMMDD-xxx)` 精确检索某张卡片的讨论

## 8. 迁移

- **数据迁移**：检查是否有实际用户已创建 `boards/*` 数据。如果是 pre-release，直接删除 `boards/` 目录，不做数据迁移
- **代码迁移**：删除所有 `board_handlers.rs`、`BoardMeta` 类型、tests/board_test.rs 保留但改名 `card_test.rs` 并全量重写
- **向后兼容**：不维护。这是重构而非演进

## 9. 非目标（显式排除）

- ❌ Card 之间的依赖关系（blocked by / blocks）
- ❌ Card 的优先级 / 截止日期
- ❌ Card 的 description 字段（描述写在 discussion.thread L0001）
- ❌ 修改 card title（创建后不可改；如需改名，讨论串里写"标题更名为 X"作为约定）
- ❌ Card 批量操作
- ❌ Label 的全局管理（增删改查）
- ❌ Channel 归档对 card 的级联行为变更

以上都留给未来版本，本轮聚焦"去 board 化 + channel 归属"。

## 10. Agent 工作流含义

这次重构明确 agent 的工作路径：

```
1. 人 在 #channel-X 里 @claude-agent 派活（附带 title、上下文）
2. Runtime 检测 mention → 自动 create_card(channel=channel-X, title=..., assignee=claude-agent, status=todo)
3. Agent poller 看到自己的 todo card → 开始执行
4. Agent 每次进展 → send_card_message 到 card discussion，并 update_card(status=doing)
5. 执行中有疑问 → Agent 在原 channel 里发问（回到讨论空间，不污染 card 日志）
6. 完成 → update_card(status=done) + 最后一条汇报
```

**第 2 步（channel mention 自动建卡）** 是后续前端/runtime 的增强点，**本轮后端只需把 card API 做对**，自动建卡可以后面做。

## 11. 验收标准

1. `cargo test` 全绿（包括改写后的 card_test.rs）
2. 旧 `board_*` 方法从 IPC、client、CLI 全部消失
3. `gitim-client` 提供新的 card 方法，类型签名匹配 §4
4. `gitim-runtime` HTTP 暴露 §6 列出的 5 个端点
5. webui-v2 可以通过 HTTP 完成 "创建 card → 列出 card → 改 status → 发 discussion 消息" 完整链路（基础接入，不要求 UI 完善）
6. `gitim-index` 能把 card discussion 消息索引到 FTS5，`search` 能检索

## 12. 范围外（另开 ticket）

- 前端看板视图 UI（本设计只保证后端能力到位，前端 UI 作为独立工作流）
- Agent mention → 自动建卡 的 runtime 逻辑
- Label 全局管理界面
