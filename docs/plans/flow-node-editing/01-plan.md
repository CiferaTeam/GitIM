# Flow 节点编辑 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development 或 superpowers:executing-plans，task-by-task 实现。步骤用 `- [ ]` 追踪。
> **风格约定**：本 plan 只写分工 + 接口契约 + 测试断言语义，不贴完整实现代码体（用户偏好）。契约（类型字段 / 函数签名 / 端点 / 错误码）写精确，实现时照契约写代码。

**Goal:** 让 WebUI 能完整编辑已存在 flow 的节点结构（加/删节点 + 改 needs/type/owner/participants/labels/prompt），经一个覆盖写端点落盘。

**Architecture:** 新增 `FlowReplace` IPC（覆盖写整个 flow），复用现成的 `commit_flow_document_locked`（validate → stringify → 写盘 → commit）管线。前端就地 Edit 模式组装完整 nodes 数组一次 PUT。依赖链：core → daemon → client → runtime HTTP → frontend。

**Tech Stack:** Rust（gitim-core / gitim-daemon / gitim-client / gitim-runtime）+ React 19 + Zustand + Vite（products/gitim/frontend）。

参考 spec：[`00-requirements.md`](00-requirements.md)

---

## File Structure

| 文件 | 改动 | 职责 |
|------|------|------|
| `crates/gitim-core/src/flow/types.rs` | 加 `FlowNodeInput` + `into_flow_node()` | IPC 入参类型（带 `prompt`，因 `FlowNode.prompt` 是 `#[serde(skip)]`） |
| `crates/gitim-core/src/api.rs` | 加 `FlowReplace` Request variant | IPC 协议契约 |
| `crates/gitim-daemon/src/flow_handlers.rs` | 加 `handle_flow_replace` | 读旧 doc 保元信息 → 重建 FlowMeta → `commit_flow_document_locked` |
| `crates/gitim-daemon/src/handlers/mod.rs` | dispatch arm + `is_write` guard | 路由 IPC + 标记为写操作 |
| `crates/gitim-client/src/client.rs` | 加 `flow_replace()` | thin-wrapper |
| `crates/gitim-runtime/src/http.rs` | 加 `PUT /im/flows/{slug}` + `FlowReplaceRequest` DTO + 路由 | HTTP 网关，复用现有错误映射 |
| `crates/gitim-runtime/tests/flow_http.rs` | 加 PUT 端点测试 | 该文件现无写端点覆盖 |
| `products/gitim/frontend/src/lib/types.ts` | 加 `FlowNodeInput` / replace payload 类型 | 前端 wire 类型 |
| `products/gitim/frontend/src/lib/client.ts` | 加 `replaceFlow()` | PUT 调用 |
| `products/gitim/frontend/src/components/flows/flow-detail.tsx` | 加 view/edit/saving 模式 | 节点编辑表单 + mermaid 实时预览 |
| `products/gitim/frontend/src/hooks/use-flow-store.ts` | 保存后写回 `selectedFlow` | store 更新 |

**契约锚点**（实现时核对精确行号）：
- `commit_flow_document_locked` — `flow_handlers.rs` 约 321-355（持 `commit_lock`、validate、盖 `updated_at`、commit）
- `FlowUpdateNode` handler — `flow_handlers.rs` 约 208-248（read-modify-write + departed-author 检查的可抄模板）
- `validate_flow_document` / `stringify_flow_markdown` — `validator.rs` 约 30 / `parser.rs` 约 83（**复用不动**）
- `is_write` guard 列表 — `handlers/mod.rs` 约 172-177；dispatch — 约 563-611
- `flow_write_response` / `flow_client_error_to_response` — `http.rs` 约 2454-2478 / 2222-2247（**复用不动**）
- `flows_node_set` handler — `http.rs` 约 2543-2569（最佳照抄样本）
- `human_client` — `http.rs` 约 627-643（唯一守卫）
- flow 路由注册 — `http.rs` 约 6090-6106（axum 顺序：固定前缀先于 `/{slug}`）
- `agent-detail.tsx` edit 模式 — 约 65-272（`mode` 状态机 + `draftEnv` 列表增删行）
- `updateFlowNodePrompt` — `client.ts` 约 1980-2008（PUT/PATCH 调用模板）

---

## Task 1: core — `FlowNodeInput` 类型 + `FlowReplace` IPC 契约

**Files:**
- Modify: `crates/gitim-core/src/flow/types.rs`
- Modify: `crates/gitim-core/src/api.rs`
- Test: `crates/gitim-core/src/flow/types.rs`（内联 `#[cfg(test)]`）或 `parser.rs` round-trip 旁

**契约：**

`FlowNodeInput`（`#[derive(Deserialize)]`，serde 字段对齐 frontmatter）：

| 字段 | serde | 类型 | 默认 |
|------|-------|------|------|
| `id` | `id` | `String` | 必填 |
| `node_type` | `type` | `NodeType` | 必填 |
| `owner` | `owner` | `Option<String>` | `None` |
| `participants` | `participants` | `Vec<String>` | `default`（空） |
| `signal` | `signal` | `Option<String>` | `None` |
| `needs` | `needs` | `Vec<String>` | `default`（空） |
| `required_labels` | `required_labels` | `Vec<String>` | `default`（空） |
| `prompt` | `prompt` | `String` | `default`（空） |

`into_flow_node(self) -> FlowNode`：逐字段搬运，**显式设 `prompt`**（`FlowNode.prompt` 是 skip 字段，构造时手动赋值）。

`api.rs` 加 Request variant：`FlowReplace { slug: String, name: Option<String>, description: Option<String>, nodes: Vec<FlowNodeInput>, author: Option<String> }`，`#[serde(rename = "flow_replace")]`，紧挨现有 flow variants（约 420-449）。doc-comment 一句：覆盖写整个 flow；`created_by`/`created_at` 由 handler 从旧 doc 保留。

- [ ] **Step 1: 写失败测试** — 在 types.rs 测试模块加：构造一个含 2 节点（`changelog` 入口 + `e2e` needs:[changelog]，各带 prompt）的 `Vec<FlowNodeInput>` → 各 `into_flow_node()` → 组 `FlowMeta` → `stringify_flow_markdown` → `parse` 回来 → 断言节点数、`needs`、`prompt`、`node_type` 全一致（round-trip 等价）。
- [ ] **Step 2: 跑测试确认失败** — `cargo test -p gitim-core flow_node_input` → 预期编译失败（类型不存在）。
- [ ] **Step 3: 实现** — 加 `FlowNodeInput` + `into_flow_node` + `api.rs` 的 `FlowReplace` variant。
- [ ] **Step 4: 跑测试确认通过** — `cargo test -p gitim-core flow_node_input` → PASS。
- [ ] **Step 5: fmt + commit** — `cargo fmt -p gitim-core` 后 `git add` 两文件 + 测试，commit：`feat(flow): add FlowNodeInput + flow_replace IPC contract`。

---

## Task 2: daemon — `handle_flow_replace`

**Files:**
- Modify: `crates/gitim-daemon/src/flow_handlers.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`（dispatch arm + `is_write`）
- Test: `crates/gitim-daemon/tests/flow_handlers.rs`

**职责：** `handle_flow_replace(state, slug, name, description, nodes, author)`：
1. 读旧 `FlowDocument`（不存在 → `error_code: "not_found"`）；
2. `ensure_author_not_departed`（抄 FlowUpdateNode）；
3. 用旧 doc 的 `created_by` / `created_at` + 请求的 `name`/`description`（`None` 时保留旧值）+ `nodes.into_iter().map(into_flow_node)` 重建 `FlowMeta`；
4. 调 `commit_flow_document_locked`（它内部 validate → 非法返 error_code 不落盘 → stringify → 盖 `updated_at` → 持 `commit_lock` commit）。

`handlers/mod.rs`：dispatch 加 `FlowReplace { .. } => handle_flow_replace(...)`；`is_write` guard 列表加 `FlowReplace`。

**测试断言语义**（`tests/flow_handlers.rs`，抄现有 flow handler 测试的 setup）：
- **加节点**：对已有 1 节点 flow replace 成 2 节点 → reload 断言 2 节点、新节点 prompt 落 body section。
- **删节点**：3 节点 replace 成 2（删的是叶子，无人 needs 它）→ 断言 2 节点。
- **改 needs**：replace 改某节点 needs → 断言新拓扑。
- **改 type**：`human_review` → `agent_mention` 且带 owner → 断言成功。
- **非法拓扑被拒不落盘**：replace 引入环（A needs B、B needs A）→ 断言返 error_code（cycle 类）**且** reload 旧 doc 未变（证明没落盘）。
- **悬空 needs 被拒**：节点 needs 一个不存在 id → 断言 error_code。
- **created_at 保持**：replace 后 reload 断言 `created_at` == 原值，`updated_at` 已更新。
- **flow 不存在**：replace 不存在 slug → `not_found`。

- [ ] **Step 1: 写失败测试** — 上述 8 个断言，每个一个 `#[tokio::test]`（或合并相近的）。先写「加节点」「非法拓扑不落盘」「created_at 保持」三个核心。
- [ ] **Step 2: 跑确认失败** — `cargo test -p gitim-daemon flow_replace` → 编译失败 / 路由未命中。
- [ ] **Step 3: 实现 handler + dispatch + is_write**。
- [ ] **Step 4: 跑确认通过** — `cargo test -p gitim-daemon flow_replace` → PASS。
- [ ] **Step 5: 补齐剩余断言**（删节点 / 改 needs / 改 type / 悬空 / 不存在）→ 跑 → PASS。
- [ ] **Step 6: fmt + commit** — `cargo fmt -p gitim-daemon`，commit：`feat(flow): daemon flow_replace handler (overwrite whole flow)`。

---

## Task 3: client — `flow_replace` wrapper

**Files:**
- Modify: `crates/gitim-client/src/client.rs`
- Test: 随 daemon 集成测试覆盖（client 是 thin-wrapper，无独立单测必要；若现有 flow client 方法有单测则照加）

**契约：** `pub async fn flow_replace(&self, slug: &str, name: Option<&str>, description: Option<&str>, nodes: Vec<FlowNodeInput>) -> Result<ApiResponse, ClientError>`，一行 `self.request("flow_replace", json!({ "slug": slug, "name": name, "description": description, "nodes": nodes })).await`（不填 `author`，daemon 自己从 me.json 推断）。挨着 `flow_update_node`（约 828）。

- [ ] **Step 1: 实现** `flow_replace`（参照 `flow_update_node` 形态）。
- [ ] **Step 2: 编译** — `cargo build -p gitim-client` → 通过。
- [ ] **Step 3: fmt + commit** — commit：`feat(flow): client flow_replace wrapper`。

---

## Task 4: runtime HTTP — `PUT /im/flows/{slug}`

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`
- Test: `crates/gitim-runtime/tests/flow_http.rs`

**契约：**
- DTO：`#[derive(Deserialize)] struct FlowReplaceRequest { name: Option<String>, description: Option<String>, nodes: Vec<FlowNodeInput> }`（typed struct，不手撸 `serde_json::Value`）。
- Handler `flows_replace(State, Path(slug), Json(body: FlowReplaceRequest))`：`human_client(&state, &slug)` → `client.flow_replace(...)` → `flow_write_response(resp)`；client err → `flow_client_error_to_response`。
- 路由：flow 路由组（约 6090）的 `/im/flows/{flow_slug}` 加 `.put(flows_replace)`（与现有 `.get(flows_show).delete(flows_remove)` 并列）。**确认固定前缀 `validate` 路由仍在 `/{slug}` 之前**。

**测试断言语义**（`tests/flow_http.rs`，抄现有 show/list/run-start 测试的 workspace setup）：
- PUT 一个已存在 flow 的新节点集 → 200 + 返回的 flow 含新节点。
- PUT 不存在 slug → 404。
- PUT 引入环的节点集 → 422（error_code 透传）。

- [ ] **Step 1: 写失败测试** — 上述 3 个（先 happy-path + 404）。
- [ ] **Step 2: 跑确认失败** — `cargo test -p gitim-runtime --test flow_http flow_replace` → 路由 404/未命中。
- [ ] **Step 3: 实现** DTO + handler + 路由。
- [ ] **Step 4: 跑确认通过** — 同上 → PASS。
- [ ] **Step 5: 补 422 环测试** → PASS。
- [ ] **Step 6: fmt + commit** — `cargo fmt -p gitim-runtime`，commit：`feat(flow): runtime PUT /im/flows/{slug} endpoint`。

---

## Task 5: frontend 数据层 — types + `replaceFlow`

**Files:**
- Modify: `products/gitim/frontend/src/lib/types.ts`（约 446-478，`FlowNodeSummary`/`FlowDocument` 附近）
- Modify: `products/gitim/frontend/src/lib/client.ts`（约 1980-2008，`updateFlowNodePrompt` 附近）

**契约：**
- TS 类型 `FlowNodeInput`：`{ id: string; type: NodeType; owner?: string; participants?: string[]; signal?: string; needs?: string[]; required_labels?: string[]; prompt?: string }`。
- `replaceFlow(slug: string, payload: { name?: string; description?: string; nodes: FlowNodeInput[] }): Promise<{ data: { flow: FlowDocument } }>`（对齐 `updateFlowNodePrompt` 的返回信封），`PUT /im/flows/{slug}`。

- [ ] **Step 1: 实现** 两个类型 + `replaceFlow`（参照 `updateFlowNodePrompt`）。
- [ ] **Step 2: 类型检查** — `cd products/gitim/frontend && npx tsc --noEmit`（或项目的 typecheck 脚本）→ 通过。
- [ ] **Step 3: commit** — `feat(flow): frontend replaceFlow client + types`。

---

## Task 6: frontend UI — flow-detail 就地 Edit 模式

**Files:**
- Modify: `products/gitim/frontend/src/components/flows/flow-detail.tsx`
- Modify: `products/gitim/frontend/src/hooks/use-flow-store.ts`（保存后写回 `selectedFlow`）
- Test: `products/gitim/frontend/src/components/flows/flow-detail.test.tsx`（新建，或并入现有 flow 测试）

**职责（抄 `agent-detail.tsx` 的 `mode: "view"|"edit"|"saving"` 状态机）：**
- view 模式：现状不变（只读 + 现有 prompt 编辑入口保留）。加「编辑结构」按钮进 edit。
- edit 模式：节点列表渲染为可编辑行（`draftNodes: FlowNodeInput[]`，从 `selectedFlow.nodes` seed）：
  - 字段：`id`（**已有节点只读**，新节点可填）、`type`（下拉，4 值）、按 type 动态显隐 `owner`/`participants`/`signal`、`needs`（multi-select：当前 draftNodes 的 id 集合，排除自己）、`required_labels`、`prompt`（textarea）。
  - 底部 `+ Add node`（push 一行，默认 type=agent_mention、空 id 待填）；每行 `✕ Remove`。
  - **轻量即时校验**：新节点 id 非空且唯一、`needs` 不含自己、删节点时若有别的节点 needs 它 → inline 警告（不阻塞，后端 validator 兜底）。
- mermaid 实时预览：edit 模式下用 `draftNodes` 重跑 `buildMermaidSource`（复用 `flow-dag` 的纯函数）渲染预览。
- 保存：`mode="saving"` → `client.replaceFlow(slug, { nodes: draftNodes })` → 成功 `updateSelectedFlow(res.data.flow)` 写回 store、回 view 模式；失败留在 edit 显 error（映射 error_code：cycle/unknown_need/missing field → 友好中文提示），**不丢 draftNodes**。

**测试断言语义**（组件测试，抄项目现有 React 测试风格 / vitest）：
- 进 edit → 点 `+ Add node` → 列表多一行。
- 已有节点 id 输入框 disabled（immutable）。
- 选 type=channel_thread → owner 字段隐藏、participants 字段出现。
- 点保存 → 断言 `client.replaceFlow` 被以 `draftNodes` 调用一次。
- replaceFlow reject（带 cycle error_code）→ 断言仍在 edit 模式、显错、draftNodes 未清空。

- [ ] **Step 1: 写失败测试** — 上述 5 个（先「加行」「id immutable」「保存调用」三个核心）。
- [ ] **Step 2: 跑确认失败** — 项目前端测试命令（vitest）→ FAIL。
- [ ] **Step 3: 实现** edit 模式 + 动态字段 + needs multi-select + mermaid 预览 + 保存。
- [ ] **Step 4: 跑确认通过** → PASS。
- [ ] **Step 5: 补 type 动态字段 + error 保留 draft 两个测试** → PASS。
- [ ] **Step 6: 类型检查 + commit** — typecheck 通过后 commit：`feat(flow): in-place node editing UI in flow-detail`。

---

## 收尾（实现全部 task 后）

- [ ] **scoped 回归**：按改动 crate 跑 `cargo test -p gitim-core -p gitim-daemon -p gitim-runtime`（不跑全量，遵循 CLAUDE.md 测试节奏；除非发现跨 crate 连带改动）+ 前端 typecheck + vitest。
- [ ] **code review**：requesting-code-review（或 codex review 当前 diff）。
- [ ] **DESIGN.md 一致性**：edit 模式 UI 的视觉（按钮 / 表单 / 间距）对照 `DESIGN.md` 核查。
- [ ] **停在合并前**：留实现报告，等用户回来走 finishing-a-development-branch（不自动合并）。

---

## Self-Review

**Spec coverage**：P1 完整节点编辑 → Task 6 字段全覆盖；P2 覆盖写 → Task 1+2；P3 就地 Edit → Task 6；P4 一致性非问题 → 无需 task；P5 加性扩 → 各 task 均不动现有 IPC/端点。§1-§5 架构 → Task 1-6 一一对应。错误处理（validator 拒不落盘 / 404 / 前端 error 映射）→ Task 2、4、6 测试覆盖。Non-goals（rename/排序/exits/新建 flow/拖拽）→ 不产生 task，Task 6 显式 id immutable。✅ 无缺口。

**Placeholder scan**：契约字段 / 端点 / 错误码均具体；测试以断言语义描述（非 "写测试" 占位）；行号标「约」+「实现时核对」。✅

**Type consistency**：`FlowNodeInput` 字段在 core（Task 1）/ runtime DTO（Task 4）/ 前端（Task 5）三处一致；`flow_replace` 方法名贯穿 client（Task 3）/ runtime（Task 4）；`replaceFlow` 贯穿前端 Task 5/6。✅
