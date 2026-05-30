# Flow 节点编辑 — 需求共识

> 仅 design / requirements，不含代码细节。下一阶段（writing-plans）产 `01-plan.md`。

Status: APPROVED
Date: 2026-05-30
Base: `claude/unruffled-ishizaka-99cb91` @ 156e82f6
来源: brainstorming（范围 / 粒度 / UI 形态三问收敛）+ core·daemon·runtime·frontend 三层并行调研

---

## 背景

[team-flows v1/v1.5](../team-flows/01-plan.md) 已落地：`flows/<slug>/index.md`（frontmatter 描述 DAG + body `## <node_id>` section 给节点 prompt）、daemon `flow_handlers`、WebUI Flows tab（mermaid 只读 DAG + react-markdown）、runs + state。

**现状写能力**（`crates/gitim-daemon/src/flow_handlers.rs`）：

| IPC | 粒度 |
|-----|------|
| `FlowCreate` | 建空壳 flow（`nodes: []`） |
| `FlowRemove` | 软删整个 flow 到 `.trash/` |
| `FlowUpdateNode` | **只改单节点 prompt**，frontmatter 全 immutable |
| `FlowValidate` / `FlowList` / `FlowShow` | 只读 |

**Gap**：加节点 / 删节点 / 改依赖（`needs`）/ 改 `type`·`owner`·`participants`·`labels` —— 这些拓扑结构编辑**完全没有写路径**。daemon 注释明说结构编辑靠"直接改 index.md 文件"。WebUI 现状是只读查看 + 改 prompt，无法在界面里编排 flow。

**目标**：把 flows 从"只读 + 改 prompt"升级为完整的可视化编排器 —— 用户不用手改 `index.md` 就能编辑已存在 flow 的节点结构。

---

## 关键事实：模板编辑与 active runs 解耦（化解一致性顾虑）

调研确认（`crates/gitim-core/src/flow/run.rs:124`、`crates/gitim-daemon/src/flow_run_handlers.rs:71`、`products/gitim/frontend/src/components/flows/run-detail.tsx:101`）：

- run 的 `state.yaml` 只存节点的 **id + 执行状态**（`FlowRunNode { id, status, actor, started_at, completed_at, result_ref }`），**不存** `type`/`owner`/`needs`/`prompt`。
- run start 时把模板节点 id **快照**进 `state.yaml`，之后 run 自包含。
- run-detail 画 DAG 时只读 run 自己的节点，**连边都不画**（`needs: []`）。

⇒ **编辑模板结构对在跑的 run 零影响**。run 用 start 那刻的快照，新 start 的 run 才用新定义。版本语义如 CI（改了 pipeline 定义，在跑的 build 用旧定义）。**因此本期不需要锁、不需要任何一致性处理。**

---

## 共识 Premises

### P1 — 范围 = 完整节点编辑
加节点 / 删节点 + 改 `needs`（依赖）+ 改 `prompt` + 改 `type`·`owner`·`participants`·`signal`·`required_labels`。本期编辑**已存在**的 flow，不含"从零新建 flow"的 UI 入口（见 Non-goals #4）。

理由：一旦做了节点编辑表单，多渲染几个字段几乎零边际成本，而"能加节点却改不了它的 owner"功能割裂。

### P2 — 粒度 = 覆盖写（`FlowReplace`）
前端在内存里组装完整 nodes 数组（加/删/改全在前端完成），一次 PUT 覆盖整个 flow。后端复用现成的 `commit_flow_document_locked` 管线（validate → stringify → 写盘 → commit）。

理由：三层调研一致指向覆盖写。`commit_flow_document_locked` + `stringify_flow_markdown` + `validate_flow_document` 这套管线**已经是为"写整个 flow"长出来的**，`FlowCreate`（写空 nodes）就是它的退化用例 —— 覆盖写只是把"空 nodes"换成"前端给的 nodes"，复用率接近 100%。细粒度 `add/remove/reorder` IPC 工作量 3-4 倍且更脆（成环只能整图校验，等于每次 mutation 都跑全量 validate）。

### P3 — UI = 就地 Edit 模式
复用 `agent-detail.tsx` 的 `view/edit/saving` 状态机，在现有 flow-detail 页加"编辑"按钮。进入 Edit 模式后：
- 节点卡片变可编辑表单，**字段按 `type` 动态显隐**；
- `needs` = 对当前节点 id 列表做 **multi-select**（排除自己）—— 这就是完整的拓扑编辑，不需要画布；
- 底部 `+ Add node`，每卡 `✕ Remove`（复用 agent-detail 的 `draftEnv` 列表增删行模式）；
- **mermaid 实时预览**：draft nodes 改动重跑 `buildMermaidSource` 自动重画；
- 保存：组装完整 nodes → PUT → 成功后写回 store（**server-confirmed，非乐观**；失败留在 edit 模式不丢草稿）。

### P4 — 一致性 = 非问题
见上"关键事实"。模板编辑不锁、不处理 active runs。

### P5 — 协议加性扩，不破现有
- `index.md` 格式不变（frontmatter + `## <id>` body section）
- `FlowNode` / `FlowMeta` schema 不变
- 现有 `FlowCreate`/`FlowRemove`/`FlowUpdateNode`/`FlowValidate`/`FlowList`/`FlowShow` 行为不动
- 纯加性：新增 `FlowReplace` IPC + `FlowNodeInput` 类型 + `PUT /im/flows/{slug}` + 前端 `replaceFlow` + edit 模式

---

## 架构决策（三层改动，依赖链顺序）

### §1 — core（`gitim-core`）
新增 `FlowNodeInput` 入参类型，字段对齐 `FlowNode`，但**显式带 `prompt`**（`FlowNode.prompt` 标了 `#[serde(skip)]`，不能直接复用反序列化）：

| 字段 | 必填性 | 说明 |
|------|--------|------|
| `id` | 必填 | slug 规则；本期视为 immutable（见 Non-goals #1） |
| `type` | 必填 | `agent_mention` / `channel_thread` / `human_review` / `wait_for_signal` |
| `owner` | `agent_mention` 必填 | |
| `participants` | `channel_thread` 必填 | |
| `signal` | `wait_for_signal` 必填 | |
| `needs` | 可选（空 = 入口节点） | 上游依赖 = 边 |
| `required_labels` | 可选 | 信息位，不强制 routing |
| `prompt` | 可选 | 落到 body `## <id>` section |

`stringify_flow_markdown` / `validate_flow_document` **复用不动**（成环、悬空 needs、必填字段校验全已覆盖）。

### §2 — daemon（`gitim-daemon`）
新增 `FlowReplace { slug, name?, description?, nodes: [FlowNodeInput], author }` IPC variant + `handle_flow_replace`：读旧 doc 保留 `created_by`/`created_at` → 用新 nodes 重建 `FlowMeta` → 调 `commit_flow_document_locked`。`is_write` guard 列表补一行。几乎零新逻辑。

### §3 — client（`gitim-client`）
`flow_replace` thin-wrapper（一行 `self.request("flow_replace", json!({...}))`）。

### §4 — runtime HTTP（`gitim-runtime`）
`PUT /im/flows/{flow_slug}` + typed `FlowReplaceRequest` DTO（body 含 node 列表，用 `#[derive(Deserialize)]` struct 比手撸 `serde_json::Value` 更耐改）+ 复用 `flow_write_response` / `flow_client_error_to_response`。路由注册注意 axum 顺序：固定前缀（`validate`）必须在 `/{slug}` 之前。

### §5 — frontend（`products/gitim/frontend`）
- `flow-detail.tsx` 加 `view/edit/saving` edit 模式（抄 `agent-detail.tsx:65`）；
- `client.ts` 加 `replaceFlow(slug, { name, description, nodes })`（PUT）；
- `use-flow-store` 保存成功后写回 `selectedFlow`。

---

## 错误处理

- **后端写盘前必过 validator**：成环 / 悬空 `needs` / `type` 缺必填字段 → 422 + error_code，**不落盘**（`commit_flow_document_locked` 在写盘前调 validate，非法直接返错）。
- flow 不存在 → `not_found` → 404。
- departed author → 拒（复用 `ensure_author_not_departed`）。
- **前端**：error_code 映射成友好提示，留在 edit 模式不丢草稿；另做轻量即时校验（`needs` 不能选自己、删节点时提示"还有 X 个节点依赖它"）。
- 覆盖写触发 file watcher，可能多收一个 `FlowChanged` SSE event —— 前端容忍（现有 `FlowUpdateNode` 已是此行为，净效果是一次冗余 no-op commit 尝试，无数据损坏）。

---

## Non-goals（本期不做，可推翻）

1. **node id rename**：已有节点 id 在 Edit 模式只读；改名 = 删旧 + 建新。理由：id 既是 body section 匹配键又是 `needs` 引用，rename 要同步重写所有引用，易悬空，不值当。
2. **节点视觉排序**：后端对节点数组序零语义，"顺序"对执行无意义；做了是给展示层凭空造一等概念。
3. **条件分支（`exits`）**：v2 conditional 留位，daemon v1 解析根本不读，UI 不暴露。
4. **新建 flow 的 UI 入口**：本期聚焦编辑已存在 flow。节点编辑器天然能把 `FlowCreate` 建的空壳 flow 填满，未来接上"建空壳"入口即闭环；带节点的新建向导留 v2。
5. **可视化拖拽图编辑（react-flow）**：另一个量级（替换渲染层 + 新依赖 + `needs` 无坐标的持久化），投入产出比差。表单 multi-select 已是完整拓扑编辑能力，mermaid 保留为只读预览。

---

## 测试策略

- **core**：`FlowNodeInput → FlowMeta` 重建 + `stringify` round-trip；validator 成环 / 悬空测试已有，复用。
- **daemon**：`flow_replace` 集成测试 —— 加 / 删节点、改 `needs`、改 `type` 都生效；非法拓扑（成环 / 悬空）被拒且**不落盘**；`created_at` 保持不变；departed author 拒。
- **runtime**：`tests/flow_http.rs` 补 `PUT` 端点测试（该文件现无任何写端点覆盖，顺手补上）。
- **frontend**：`flow-detail` edit 模式组件测试（增删行 / `type` 动态字段 / 保存调 PUT / error 展示）。

---

## 实现顺序

core（`FlowNodeInput`）→ daemon（`FlowReplace` handler）→ client（`flow_replace`）→ runtime HTTP（`PUT` 端点）→ frontend（edit 模式）。每层 scoped 测试通过再进下一层。
