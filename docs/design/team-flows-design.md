# Team Flows — Design

> 团队 SOP 流程库:git-tracked 的 markdown 模板,DAG 渲染,coordinator 显式调用,默认 team 共享。

---

## Goal

让 agent 团队能把"做某件事的多人协作流程"沉淀为可查询、可引用的模板,下次做类似事情时由 coordinator agent 看着模板派单,而不是每次从零 reasoning。

## Background

- **现状**:GitIM 没有团队级"流程/SOP/skill" 沉淀机制。`board` 是 per-handler 的状态牌,`system_prompt` 是 per-agent 的纯字符串且不支持引用外部文件,`channels` 是对话流。**没有"团队记忆 + 结构化协作步骤"这一层**。
- **不冲突**:已批准的 `agent-coordinator-prompt-design.md` bet 在"用自然语言而非 DAG 控制流"。flow 不破这个哲学 —— 它**不是 runtime 强制执行的 pipeline**,而是 coordinator agent 在做事时被推荐参考的"团队 muscle memory"。
- **类比**:flow 之于 GitIM,等于 skill 之于 Claude Code。

## Core Concept

### 心智模型

```
flow (template)   = 沉淀的 markdown 模板,git-tracked,只读为主
flow run (v2)     = 一次具体调用产生的实例数据,挂在每个节点上
node              = 模板里的一个抽象 step
edges             = 节点之间的依赖(v1: needs[] 数组)
```

**关键性质**:
- 模板**不强制执行**,是参考材料
- 触发是**显式 invocation**(`@coordinator 用 <slug>`),不是 fuzzy auto-dispatch
- v1 只实现 template 层,v2 才补 run instance + executor

### 节点 = 抽象 step(不绑通信媒介)

节点不是"派给某 agent 的消息",而是抽象步骤,带 `type` 字段:

| type | 语义 | 必填字段 | 实例化时落地为 |
|---|---|---|---|
| `agent_mention` | 派给某个 agent 做事 | `owner` | thread 里一条 `@owner` 消息(带 prompt) |
| `channel_thread` | 多 agent 协作讨论 | `participants[]` | 临时 sub-channel + 所有人在场 |
| `human_review` | 等人确认 | (none) | thread 留消息等人 reply |
| `wait_for_signal` | 等外部事件(CI green / git tag 等) | `signal` | v1 schema 占位,不实现 |

v1 落地 `agent_mention` + `channel_thread` 两个最常用的;`human_review`、`wait_for_signal` schema 留位,后续按需启用。

## File Structure

```
<workspace-repo>/
├── channels/                      ← 已有
├── showboards/                    ← 已有
├── users/                         ← 已有
└── flows/                         ← 新增
    └── <slug>/
        └── index.md               ← 模板:frontmatter + body
        # v2 预留:
        # └── runs/<run-id>/
        #     ├── state.json       ← 节点状态、refs
        #     └── nodes/<node-id>/ ← 节点产物存档
```

- `<slug>`:小写 a-z 0-9 连字符,1-39 字符,跟 channel 命名同套规则
- v1 只创建 `index.md`,`runs/` 目录留位但不创建

## Schema(frontmatter + body)

frontmatter 是 **source of truth**;body 的 `## <node-id>` section 跟 frontmatter `nodes[].id` 1:1 对应。

```markdown
---
schema_version: 1
slug: release
name: Release Flow
description: 用于一次正式版本发布(打 tag → e2e → 发包 → 公告)
created_by: lewis
created_at: 2026-05-12T10:00:00Z

nodes:
  - id: changelog
    type: agent_mention
    owner: alice
    needs: []                      # 入口节点

  - id: e2e
    type: agent_mention
    owner: bob
    needs: [changelog]

  - id: release-discuss
    type: channel_thread
    participants: [alice, bob, carol]
    needs: [e2e]

  - id: publish
    type: agent_mention
    owner: alice
    needs: [release-discuss]
---

## changelog

请基于 `git log v0.7..HEAD` 生成 changelog,关注:
- breaking changes
- 新功能
- 性能改进

输出到一个新 thread,完成后回复"done"。

## e2e

跑 `cargo test --workspace`,把失败的测试列出来贴回 thread。

## release-discuss

在临时 channel 里和大家讨论:版本号是否升 minor、发布时间、要不要写 blog post。讨论收敛后由 alice 收尾。

## publish

跑 `release.sh v0.8`,贴出 binary 下载链接。
```

### Schema 留位字段(v1 optional,v2 才读)

为 v2 conditional / executor 留位:

- 节点 `exits: [ok, failed]`:节点可能的退出标签
- 边 `when: ok`(写在 needs 的扩展形式里,或 edges[] 单独列出):按上游节点的 exit label 决定本节点是否触发

v1 schema 都接受这两个字段(optional),但 coordinator 不强制 agent 输出 exit label,也不按 `when` 路由 —— v1 行为等价于"忽略 exits/when"。

### 双源约束

| 情况 | 行为 |
|---|---|
| frontmatter 有 id / body 没有 `## id` section | warn,该节点 prompt 默认为空 |
| body 有 `## id` / frontmatter 没有 | warn,孤儿 section 忽略 |
| frontmatter / body 顺序不一致 | 不 warn,frontmatter 顺序决定 DAG |

## DAG 表达

- **v1 行为**:线性 + fan-out/fan-in(`needs[]` 数组描述依赖)
- **v1 schema 留位**:`exits[]` + 边 `when` 字段 optional,供 v2 conditional
- **渲染**:mermaid 直接画(`A --> B`,带 label 时 `A -->|ok| B`),WebUI 用 mermaid.js,CLI 用 ascii art

## v1 接口

### CLI

```text
gitim flows                  列出所有 flow:slug / name / description / 节点数
gitim flow show <slug>       完整模板内容 + DAG ascii 图
gitim flow create <slug>     生成 stub 模板(frontmatter only,body 空 section)
gitim flow rm <slug>         soft delete(移到 .trash/)
gitim flow validate <slug>   schema 检查 + 双源对齐报告
```

### WebUI

"Flows" tab,跟 Channels / Boards / Agents 同级:
- **列表页**:flow 卡片(name、description、节点数、updated_at)
- **详情页**:
  - mermaid 渲染 DAG
  - 各节点 prompt 展开(markdown 渲染)
  - "Run this flow" 按钮 → 把 `@coordinator 用 <slug>` 文本拷到当前 thread 输入框(不直接 invoke,让人 review 后发送)

**v1 不做 WebUI 编辑器、不做 visual node editor**。编辑 = 用文本编辑器改 `flows/<slug>/index.md`,daemon 文件 watcher 自动 git sync(跟 board 编辑路径一致)。

### Agent 内置工具(daemon IPC)

加 `flow_handlers.rs`,跟 `board_handlers.rs` 平级。**所有 agent 都能用**(不限 coordinator):

```text
flows.list()              -> [{slug, name, description, node_count, updated_at}]
flows.show(slug)          -> markdown 原文(agent 自己解,server 不做强结构化)
flows.validate(slug)      -> {ok: bool, warnings: [...], errors: [...]}
```

**不**提供 `flows.invoke()` / `flows.run()` —— v1 没有 run 概念,执行靠 agent 自己 reasoning。

### Provider Default System Prompt 增量

flow 是 gitim 的一个内置能力,跟 boards / channels 同等地位。在所有 provider 的 `build_system_prompt()` 里,跟 boards / channels 的介绍并列加一段:

```text
GitIM 提供了 flows —— 团队沉淀的 SOP 流程库。每个 flow 是 git 里的
markdown 模板,frontmatter 描述节点和依赖关系,body 是每节点的说明。

可用 API:
- flows.list()        看团队都有哪些 flow
- flows.show(slug)    读完整模板(markdown 原文)

模板是参考不是脚本。有人让你按某 flow 走时,自己读、自己 adapt 到当
前情境、自己用 thread/channel 派单、自己判断节点是否完成。
```

**所有 agent 都看到这段**,不仅 coordinator。不需要单独的"coordinator prompt 注入"机制。

## 写入路径

```text
gitim flow create   → daemon 写 flows/<slug>/index.md(template stub),commit
人工编辑 md         → daemon 文件 watcher 检测变更 → validate(warn 不阻塞) → commit & sync
gitim flow rm       → daemon mv 到 .trash/(soft delete)
```

跟 board / channel 完全一致的写入语义 —— 任何人(任何 agent)可改,不做权限模型。

## Validation 规则

daemon 加载/校验时检查:

| 规则 | 失败行为 |
|---|---|
| `slug` 合法(小写 a-z 0-9 -,1-39 字符) | error,拒绝加载 |
| node id 唯一、命名合法 | error,拒绝 |
| `needs` 不引用不存在的 id | error,拒绝 |
| `needs` 不形成环(拓扑排序失败) | error,拒绝 |
| frontmatter ↔ body section 1:1 对齐 | warn,不拒绝 |
| 单 flow 文件 ≤ 256KB | warn(超过则截断渲染) |
| 节点数 ≤ 50 | warn(避免巨型 flow) |

## v2 占位(只占位,v1 不实现)

| v2 能力 | v1 留位方式 |
|---|---|
| Fork instance | 目录预留 `flows/<slug>/runs/`(v1 不创建) |
| Executor 调度 | schema 不变;v1 节点 status 由 agent 自己 reason,v2 加持久化 state |
| 条件分支执行 | 节点 `exits[]` + 边 `when` v1 已 optional,v2 时 executor 才读 |
| Visual editor | WebUI 留 tab 位,v1 只读 |
| Subflow / tool_call type | NodeType 是 enum,v2 加变体不破兼容 |

v1 schema 已经为这些留位,**v2 不需要 schema 迁移**。

## v1 非目标(明确不做)

```text
❌ 自动/fuzzy trigger              —— 必须显式 "用 <slug>"
❌ 条件分支的 runtime 执行         —— agent 在节点 prompt 里自己 reason
❌ Run instance / state 追踪
❌ Visual node editor / WebUI 内编辑
❌ Subflow / nested flow
❌ Frontmatter version 字段        —— git history 就够
❌ Per-handler flow / DM flow      —— 只 team 共享
❌ Cross-workspace flow 共享 / marketplace
❌ Flow 写权限模型                  —— 跟 channel/board 一致,任何人可改
❌ flows.invoke() API              —— v1 没有 run 概念
```

## 实现归属

| 任务 | crate / file |
|---|---|
| Schema 类型(`FlowTemplate`/`FlowNode`/`NodeType`) | `gitim-core::flow`(跟 `dm`/`board` 平级) |
| Frontmatter + body section parser | `gitim-core::flow::parser` |
| Validator(拓扑、id 唯一、双源对齐) | `gitim-core::flow::validator` |
| Daemon IPC handlers | `gitim-daemon::flow_handlers`(参考 `board_handlers`) |
| 文件 watcher 集成 | 复用 daemon 现有 workspace watcher,加 `flows/` 路径 |
| CLI 命令 | `gitim-cli` 加 `flows.rs` |
| Agent 内置工具暴露 | 走 daemon IPC,**无 provider 适配成本** |
| WebUI Flows tab | `products/gitim/frontend/` + mermaid.js |
| Provider system prompt 增量 | 各 provider 的 `build_system_prompt()` 实现 |

## 测试节奏

遵循 CLAUDE.md "scoped tests during dev, full test at baseline + delivery":

- `gitim-core` 加 flow parser/validator unit tests(纯函数,快)
- `gitim-daemon` 加 handler 集成 test(tempdir 仓库,跟 `board_handlers` 测试同套)
- `gitim-cli` 加 e2e 测试(create → list → show → validate → rm)
- WebUI manual smoke

## Phase 划分

- **Phase 1**(预计 3-5 天,一次性 ship):`gitim-core` flow module + daemon handlers + CLI + WebUI Flows tab + provider system prompt 增量。
- **Phase 2**(独立 plan,等 v1 用 4 周后再决策):fork instance + executor + conditional exits + WebUI 编辑器。

---

## 实现决策(2026-05-17 brainstorm 收口)

### v1 scope = Phase 1 完整一次 ship

不切 v0a/v0b。`gitim-core::flow` types + parser + validator → `gitim-daemon::flow_handlers` + watcher 接入 → `gitim-cli` flow 子命令 → WebUI Flows tab → provider system prompt 增量,**同一个 PR 走完**。预计 3-5 天。

### WebUI 渲染栈:mermaid.js + react-markdown

前端目前零 markdown / 零 graph 依赖。两个都新增,lazy-import 到 Flows tab,不污染主 bundle:

- `mermaid.js` 渲染 DAG。理由:能直接吃 design 里 `A -->|ok| B` 的 label 语法,v2 加 conditional exit 时不用换库;~500KB 但 lazy load
- `react-markdown` 渲染节点 prompt body
- CLI 端独立走 ascii art,**不**复用 mermaid 输出

### `flows.show()` 不混 validation

`flows.show(slug)` 只返 markdown 原文。validation 走独立 `flows.validate(slug)` API。Agent 默认信任 source,需要校验时显式 call —— show 不为没人看的字段付 server 端 parse cost。

### 复用基线(board/channel 既有模式)

下表是 plan 阶段会落地的复用点,**不是新决策**,是把已有模式映射到 flow:

| 层 | 复用什么 |
|---|---|
| `gitim-core::flow::types` | `ChannelName` newtype + `Result<Self, Err>` constructor(slug 校验) |
| `gitim-daemon::flow_handlers` | `state.commit_lock` → `std::fs::write` → `git_storage.add_and_commit_only_as(path, msg, author)` 单文件 commit;成功后 `event_tx.send(...)` + `push_notify.notify_one()` |
| watcher | 复用现有 daemon workspace watcher,加 `flows/` 路径(plan 阶段 verify watcher 是否 generic 到能直接加路径,还是要加注册点) |
| CLI | clap `Commands::Flow { command: FlowCommands }` enum + `commands/flow.rs` 模块 + `cmd_*` async fn,输出 `OutputMode::Human \| Json` |
| 前端 | `components/flows/flows-view.tsx` + `hooks/use-flow-store.ts`(zustand)+ `lib/client.ts` 加 `listFlows/getFlow/...` |
| Provider prompt | `default_gitim_api()`(`crates/gitim-agent-provider/src/prompts.rs`)里 Boards 段后新增 Flows 段 |
