# Protocol Typing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 GitIM 的层间协议从"请求 typed、响应开口"收敛为对称 typed，阻止新债扩散，按热点逐步收敛存量。

**Architecture:** 信封不动（`Response { ok, data, error }` / `ApiResponse`），但每个 Request method / 每个 HTTP endpoint 配 typed payload struct；`Onboard.auth` 改 enum；落盘 schema（me.json）typed 化；加守门规则。

**Tech Stack:** Rust（serde, axum, tokio）；不引入新依赖、不改 wire format、不动前端 ts。

**Design doc:** [`design.md`](./design.md)

**前置阅读:** 执行前先读 design.md 的"决策点"四条，与用户对齐答案；尤其是 endpoint 推进粒度（建议一次一个 endpoint）和 `MeJson` crate 归属。

---

## File Map

| 文件 / 目录 | 动作 | 责任 |
|---|---|---|
| `crates/gitim-core/src/me_json.rs` | 创建 | `MeJson` schema struct + merge 函数（daemon/runtime 共享） |
| `crates/gitim-core/src/lib.rs` | 修改 | 暴露 `me_json` 模块 |
| `crates/gitim-daemon/src/api.rs` | 修改 | `Onboard.auth` 改 `AuthPayload` enum；为高优先级 method 加 `XxxResponse` struct |
| `crates/gitim-daemon/src/auth_payload.rs` | 创建 | `AuthPayload` enum 定义（如不放 api.rs） |
| `crates/gitim-daemon/src/onboard.rs` | 修改 | 用 `AuthPayload` 替代 Value 链 |
| `crates/gitim-daemon/src/handlers/*.rs` | 修改 | 高优先级 handler 用 typed Response payload |
| `crates/gitim-daemon/src/card_handlers.rs` | 修改 | 同上 |
| `crates/gitim-client/src/types.rs` | 修改 | 给 `ApiResponse` 加泛型 `parse_data::<T>()` 辅助 |
| `crates/gitim-runtime/src/http.rs` | 修改 | 高优先级 endpoint 加 `XxxResponse` struct，handler 用 typed Json |
| `crates/gitim-runtime/src/http_responses.rs` | 创建（可选） | 把 endpoint Response struct 集中（避免 http.rs 进一步膨胀） |
| `crates/gitim-cli/src/*.rs` | 修改 | 切到 typed 解析，移除 Value 字段访问 |
| `CLAUDE.md` | 修改 | orientation 段落记录"协议层已 typed，新 endpoint 必须 typed Response" |
| `scripts/check-protocol-typing.sh` | 创建（可选） | grep guard 阻止新增 `Json(json!(` |

---

## Phase 0：契约规则与决策对齐

### Task 0.1：与用户确认 design.md 的四个决策点

- [ ] **Step 1: 阅读 design.md 末尾"决策点"四条**

文件：`docs/plans/protocol-typing/design.md` 的"决策点"段落。

- [ ] **Step 2: 与用户逐条确认**

逐条 paraphrase 后让用户拍板；记录答案到本 task 的 step 3。

- [ ] **Step 3: 把决策追加到 design.md**

在 design.md 末尾加一节"已确认决策（YYYY-MM-DD）"，记录四条答案，作为后续 phase 的依据。

- [ ] **Step 4: Commit**

`git add docs/plans/protocol-typing/design.md && git commit -m "docs(plan): record protocol-typing decisions"`

**验收**：design.md 末尾有"已确认决策"段；后续 task 不再有"取决于决策"的悬挂。

---

> 决定：不单独写契约文档。代码本身（typed struct + serde + snapshot 测试）就是契约。CLAUDE.md orientation 在 Phase 5.2 一处记录即可。

---

## Phase 1：me.json schema 落地（共享基础，最小破坏面）

> 选这个先做的原因：me.json 既是 daemon 写、runtime 读的双边契约，又有 CLAUDE.md 已记录的"merge 语义"约束，typed 化收益最大且不动 IPC wire format。

### Task 1.1：定义 `MeJson` struct

**Files:**
- Create: `crates/gitim-core/src/me_json.rs`
- Modify: `crates/gitim-core/src/lib.rs`

- [ ] **Step 1: 调研当前 me.json 真实字段**

搜 `me.json` 在 daemon onboard.rs 写盘和 runtime http.rs 读盘两端实际触达的字段（`handler` / `display_name` / `provider` / `model` / `system_prompt` / `env` / `github_email` / 任何未列出的）。在 task 注释里列全。

- [ ] **Step 2: 写 schema struct**

`MeJson` 包含必填字段（handler、display_name）和 `Option<T>` 选填字段。每个字段配 doc comment 说明 source of truth 和 merge 语义。预留 `#[serde(flatten)] extra: HashMap<String, Value>` 用于未知字段透传（避免落盘清洗掉旧字段）。

- [ ] **Step 3: 写 merge 函数**

`fn merge(existing: MeJson, incoming: MePatch) -> MeJson`：incoming 中 `Some(_)` 字段覆盖，`None` 保留 existing；`extra` 字段做 map 级别合并。CLAUDE.md 已记录"re-onboard 不传 github_email 时保留旧值"，按这个语义实现。

- [ ] **Step 4: 单元测试**

测试用例至少覆盖：
- 全字段往返序列化稳定
- merge 语义：incoming None 不抹原值
- merge 语义：incoming Some 覆盖原值
- extra 字段透传（旧字段 + 新字段都能保留）
- 未知字段反序列化不报错

- [ ] **Step 5: 在 `gitim-core::lib.rs` 暴露**

`pub mod me_json;` 加入。

- [ ] **Step 6: Commit**

`git add crates/gitim-core/src/me_json.rs crates/gitim-core/src/lib.rs && git commit -m "feat(core): add MeJson schema with merge semantics"`

**验收**：`cargo test -p gitim-core me_json` 全绿；schema 被两端引用前先单独站住。

---

### Task 1.2：daemon 写盘改用 `MeJson`

**Files:**
- Modify: `crates/gitim-daemon/src/onboard.rs`（`write_me_json` 函数）

- [ ] **Step 1: 找到 write_me_json 函数**

在 onboard.rs 中定位写盘实现（CLAUDE.md 提到"merge 语义"在这里）。

- [ ] **Step 2: 用 `MeJson` + `merge` 重写**

读盘 → deserialize 成 `MeJson`（不存在则空）→ 调 `MeJson::merge(existing, incoming_patch)` → serialize 写盘。incoming_patch 由 onboard 输入参数构造。原 Value 操作链全部移除。

- [ ] **Step 3: 测试覆盖**

为 write_me_json 加（或扩）单元测试：
- 首次 onboard 创建文件
- 二次 onboard 不传 github_email，旧值保留（CLAUDE.md 明示场景）
- 二次 onboard 传新字段，merge 后体现

- [ ] **Step 4: 跑相关测试**

`cargo test -p gitim-daemon onboard`

- [ ] **Step 5: Commit**

`git add crates/gitim-daemon/src/onboard.rs && git commit -m "refactor(daemon): write_me_json uses MeJson schema"`

**验收**：onboard.rs 这个函数不再有 `serde_json::Value` 直接操作；merge 语义由 `MeJson::merge` 单点保证。

---

### Task 1.3：runtime 读盘改用 `MeJson`

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（`/im/me` handler 附近，约 line 412）
- Modify: 任何其它读 me.json 的位置（用 grep 找全：`me.json`、`/im/me`、`read_me_json`、`MeJson`）

- [ ] **Step 1: 全仓库 grep 找 me.json 读取点**

`rg 'me\.json|read_me|MeJson' crates/`，列出所有读取位置。

- [ ] **Step 2: 把 Value 链改成 deserialize 成 `MeJson`**

每个读取点：`std::fs::read_to_string` → `serde_json::from_str::<MeJson>()` → 直接字段访问（`me.handler`、`me.display_name` 等）。移除 `.get("handler").and_then(as_str)` 链。

- [ ] **Step 3: `/im/me` handler 输出改 typed**

把 `Json(json!({...}))` 改成 typed `ImMeResponse` struct（这个 struct 就放在 handler 文件里，作为 Phase 3 的样板；或参考 design.md 决定的 endpoint Response 集中位置）。

- [ ] **Step 4: 跑测试 + 手动验证**

`cargo test -p gitim-runtime` 全绿。如果有 e2e 测试覆盖 `/im/me` 也跑。

- [ ] **Step 5: Commit**

`git add crates/gitim-runtime/src/http.rs && git commit -m "refactor(runtime): /im/me reads MeJson, returns typed response"`

**验收**：runtime 不再有针对 me.json 的 `.as_str()` 链；前端调 `/im/me` 拿到的字段名/形状不变（snapshot 验证）。

---

## Phase 2：Onboard.auth 改 `AuthPayload` enum

### Task 2.1：定义 `AuthPayload` enum + serde 形态对齐

**Files:**
- Modify 或 Create: `crates/gitim-daemon/src/api.rs` 或 `crates/gitim-daemon/src/auth_payload.rs`

- [ ] **Step 1: 列出当前 auth Value 实际形态**

读 `crates/gitim-daemon/src/onboard.rs` 当前 `from_value` 解析的字段；读 runtime 端构造请求的位置（grep `git_server`、`token`、`auth`），把所有可能的 wire shape 列全。

- [ ] **Step 2: 设计 enum 形态**

`AuthPayload` 含变体：`GitLocal { handler, display_name }`、`GitHub { token }`、`GitLab { token, base_url }`、`Gitea { token, base_url }`。serde 形态优先选"和当前 wire 兼容"：根据 step 1 的现状，确定 tag 字段名（可能复用 `Onboard.git_server` 作为 discriminator）和命名（`#[serde(rename_all = "snake_case")]`）。

如果发现"完全对齐 wire"代价过高（比如当前 wire 是嵌套结构），在本 task 中决定是否破坏 wire — 若破坏，必须升级前端调用点（Phase 2 末追加 task）。

- [ ] **Step 3: 写 deserialize 单元测试**

每个变体一个 test，输入是当前真实在用的 wire JSON 字符串，输出 deserialize 成对应 variant，字段值正确。

- [ ] **Step 4: Commit**

`git add crates/gitim-daemon/src/{api,auth_payload}.rs && git commit -m "feat(daemon): AuthPayload enum"`

**验收**：enum 单独通过；wire 形态明确记录在 doc comment 里。

---

### Task 2.2：替换 `Onboard.auth` 字段类型

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`（`Request::Onboard` 变体）
- Modify: `crates/gitim-daemon/src/onboard.rs`（消费方）

- [ ] **Step 1: 把 `auth: serde_json::Value` 改成 `auth: AuthPayload`**

修改 Request::Onboard 变体定义。编译会一片红 — 这是预期的。

- [ ] **Step 2: 改 onboard.rs 处理**

把所有 `from_value()` + `.get()` 链改成 `match auth { AuthPayload::GitHub { token } => ..., ... }`。CLAUDE.md 提到的 "InferredIdentity" 构造、`/user` curl、错误码 `invalid_token` / `token_lacks_repo_access` / `insufficient_scope` 等分支都要保留。

- [ ] **Step 3: 跑相关测试**

`cargo test -p gitim-daemon` + `cargo test -p gitim-runtime`。

- [ ] **Step 4: 修复 runtime 端构造调用点**

runtime `add_agent` / `/git/init` 等位置如果用 `serde_json::json!({...})` 构造 auth，改成构造 `AuthPayload` 然后 serialize（或直接走 Request struct serialize，看上下文）。

- [ ] **Step 5: 全量回归**

`cargo test`（按 CLAUDE.md 节奏，phase 结束跑全量；中间不跑全量）。

- [ ] **Step 6: Commit**

`git add crates/gitim-daemon/src/{api,onboard}.rs crates/gitim-runtime/src/ && git commit -m "refactor(daemon): Onboard.auth becomes AuthPayload enum"`

**验收**：onboard.rs 对 auth 的 `Value` 操作消失；新增 git provider 时 grep 'AuthPayload::' 能找全扩展点；e2e onboard local 模式 + github 模式都通过。

---

## Phase 3：Daemon Response payload 增量 typed（核心高频 method）

### Task 3.1：建立 client 端 `parse_data::<T>()` 辅助

**Files:**
- Modify: `crates/gitim-client/src/types.rs`

- [ ] **Step 1: 给 `ApiResponse` 加泛型解析方法**

设计一个方法：`impl ApiResponse { pub fn parse_data<T: DeserializeOwned>(&self) -> Result<T, _>; }`，从 `data: Option<Value>` 反序列化到 typed struct。错误统一映射到 client 错误类型（看现有 error 定义）。

- [ ] **Step 2: 单元测试**

构造一个测试用 typed struct，验证 `parse_data::<TestStruct>()` 正确反序列化、字段缺失错误清晰。

- [ ] **Step 3: Commit**

`git add crates/gitim-client/src/types.rs && git commit -m "feat(client): typed parse_data helper on ApiResponse"`

**验收**：辅助方法落地；后续 task 中 CLI / runtime 消费 daemon 响应可用。

---

### Task 3.2：先做 `Status` method 作示范（最小响应）

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`（加 `StatusResponse` struct）
- Modify: `crates/gitim-daemon/src/handlers/...`（status handler）
- Modify: `crates/gitim-cli/src/...`（status 命令）

- [ ] **Step 1: 列 Status 当前响应字段**

读 status handler 现在 `Response::success(json!({...}))` 的内容，列全字段。

- [ ] **Step 2: 在 api.rs 加 `StatusResponse` struct**

`#[derive(Debug, Serialize, Deserialize)]`。字段对齐 step 1 列表。放在 api.rs 末尾或新建 `responses` 子模块（看决策点 4）。

- [ ] **Step 3: handler 改用 typed**

`Response::success(serde_json::to_value(StatusResponse {...}).unwrap())`。或者如果决策给 `Response::success` 加泛型重载，就直接传 struct。

- [ ] **Step 4: snapshot 测试**

加 `#[test]` 验证 `serde_json::to_value(StatusResponse{...})` 出来的 JSON shape 与改造前一致（确保 wire 不破）。

- [ ] **Step 5: CLI status 命令切到 typed**

CLI 调用 `client.status()` → `ApiResponse` → `parse_data::<StatusResponse>()` → 用字段。移除原 Value 字段访问。

- [ ] **Step 6: 跑相关测试**

`cargo test -p gitim-daemon`、`cargo test -p gitim-cli`。

- [ ] **Step 7: Commit**

`git add crates/gitim-daemon/ crates/gitim-cli/ && git commit -m "feat: typed StatusResponse end-to-end"`

**验收**：daemon Status 改字段会让 CLI 编译失败；wire 与前一致；这套模式被 Task 3.3+ 复用。

---

### Task 3.3：批量 typed 化高频 method（按 method 一个一个推进）

> 不在本 task 内一次性做完。每个 method 是独立 task，按以下顺序排队，每个一个 commit。

**优先级排队**（每个对应一个独立的 plan task；执行时按需展开为单独 task 文件或同 phase 内 sub-task）：

1. `Send`（消息发送，最热）
2. `Read`（消息读取）
3. `ListChannels`、`ListUsers`、`ListArchivedChannels`
4. `GetThread`
5. `JoinChannel` / `LeaveChannel` / `CreateChannel`
6. `ArchiveChannel` / `UnarchiveChannel`
7. `Search` / `Reindex`
8. 看板系列：`CreateCard` / `ListCards` / `ReadCard` / `SendCardMessage` / `UpdateCard` / `ArchiveCard` / `UnarchiveCard` / `ListArchivedCards`
9. `RegisterUser` / `Onboard`（Onboard 已在 Phase 2 部分处理，这里补 response 侧）
10. `Poll` / `Subscribe` / `Stop`

每个 method 走 Task 3.2 的 7 个 step：列字段 → 加 `XxxResponse` struct → handler 改 typed → snapshot 测试 → 客户端切 typed（CLI + 任何 runtime 调用点） → 测试 → commit。

- [ ] **Step 1: 每完成一个 method，更新本文件勾选**

进度记号：在每条优先级前面加 `- [x]` 表示完成。

- [ ] **Step 2: Phase 3 整体收尾**

所有高频 method 完成后跑全量 `cargo test`，确认绿。

**验收**：daemon `Response::success(json!(...))` 在 handlers 里基本消失（残留只在罕用 method 或合理 Value 用例）；client 侧针对每个 method 都有 typed 解析路径。

---

## Phase 4：Runtime HTTP 响应 typed（按 endpoint 一次一个）

> 与 Phase 3 同节奏：一个 endpoint 一个 task，一个 commit。可与 Phase 3 并行（不同人 / 不同 worktree）。

### Task 4.0：endpoint 优先级清单与基础设施

**Files:**
- Create（可选）: `crates/gitim-runtime/src/http_responses.rs`
- Modify: `crates/gitim-runtime/src/http.rs`（导出 `ErrorBody` 共享类型 + 顶部 use）

- [ ] **Step 1: 列出所有 endpoint 当前响应形态**

`rg 'Json\(serde_json::json!\(' crates/gitim-runtime/src/http.rs` 列全位置；用 axum route 表对照分组（`/workspaces/*`、`/agents/*`、`/im/*`、`/runtime/*`、`/git/*`）。结果记到本 task 一个 markdown 表格里。

- [ ] **Step 2: 定义共享 `ErrorBody` 类型**

`#[derive(Serialize)] struct ErrorBody { ok: bool, error: String, error_code: Option<String> }` + 构造 helper。所有错误响应统一用这个，停止 ad-hoc `json!({"ok": false, ...})`。

- [ ] **Step 3: 决定是否新建 `http_responses.rs`**

如果决策点 4 选了"集中"，建文件并 re-export。否则保留在 http.rs 内（注意 3283 行已经过大，建议拆）。

- [ ] **Step 4: Commit**

`git add crates/gitim-runtime/src/ && git commit -m "feat(runtime): shared ErrorBody, endpoint typing infrastructure"`

**验收**：错误响应有共享 typed 路径；endpoint 清单存档作为后续 task 的 backlog。

---

### Task 4.1+：每个 endpoint 一个 task

按以下顺序（与 design.md 优先级一致）：

1. `POST /im/init`（onboarding 关键路径）
2. `GET /im/me`（已在 Phase 1.3 触达，此处确认 Response struct 化干净）
3. `POST /workspaces`、`GET /workspaces`、`GET /workspaces/{slug}`
4. `POST /git/init`、`POST /agents/{id}/patch`、`POST /agents/{id}/remove`、`POST /agents`（add_agent）
5. `POST /runtime/update-and-restart`、`GET /runtime/status`
6. 看板 / 频道 / 消息相关 HTTP endpoint（如果 runtime 有暴露给前端的镜像端点）
7. 其余

每个 endpoint 走以下 step：

- [ ] **Step 1: 读现有 handler，列响应字段**

定位 handler，读 `json!({...})` 内容；区分成功 / 各错误码分支。

- [ ] **Step 2: 加 `XxxResponse` struct**

`#[derive(Serialize)]`，字段名与现 wire 一致（snake_case 默认；如需 rename 加 attribute）。放在 step 4.0 决定的位置。

- [ ] **Step 3: handler 改 typed**

成功路径：`Json(XxxResponse { ... })`。错误路径：`Json(ErrorBody::new(...))`。

- [ ] **Step 4: snapshot 测试**

构造典型成功 / 错误响应，serialize 后比对 JSON 字符串与改造前一致。

- [ ] **Step 5: 跑测试**

`cargo test -p gitim-runtime --test <相关 test>`。如果有 e2e 测试触达此 endpoint，也跑。

- [ ] **Step 6: Commit**

`git add crates/gitim-runtime/src/ && git commit -m "refactor(runtime): typed response for <endpoint>"`

**验收**（每个 endpoint 各自）：handler 不再有 `Json(json!(...))` 成功响应；错误响应统一走 `ErrorBody`；wire 兼容。

**Phase 4 整体验收**：`rg 'Json\(serde_json::json!\(' crates/gitim-runtime/src/http.rs` 命中数从 25+ 降到个位数（残留在合理豁免点，逐条注释说明原因）。

---

## Phase 5：守门规则落地

### Task 5.1：grep guard 脚本

**Files:**
- Create: `scripts/check-protocol-typing.sh`（或纳入既有 lint pipeline）
- Modify（可选）: `.github/workflows/*.yml`

- [ ] **Step 1: 写脚本**

shell 脚本：在 `crates/gitim-runtime/src/http.rs` 和 daemon handler 文件中检测新增的 `Json(serde_json::json!(` 出现次数。和 `main` 分支对比，如果新增数 > 0 且不是错误响应，警告（exit 1 还是 warn 看 CI 集成方式决定）。

- [ ] **Step 2: 本地试跑**

在当前 worktree 跑一次，确认能打印 diff 数。

- [ ] **Step 3: CI 集成**

如果项目用 GitHub Actions / 其它 CI，加一个 step。如果暂无 CI lint，至少在 CLAUDE.md 文档里指引"提交前手跑"。

- [ ] **Step 4: Commit**

`git add scripts/ .github/ && git commit -m "ci: guard against new Json(json!()) in protocol layer"`

**验收**：本地跑该脚本能正确识别"新增 vs 已有"；CI 能在 PR 时给出反馈。

---

### Task 5.2：CLAUDE.md orientation 更新

**Files:**
- Modify: `CLAUDE.md`（"Current Orientation" 段落）

- [ ] **Step 1: 在 Current Orientation 加一段**

记录"协议类型化已落地：daemon Response payload 已 typed（阶段性完成 N/total）；Onboard auth 是 AuthPayload enum；runtime endpoint 响应走 typed Response struct + ErrorBody；新 endpoint 必须 typed Response（禁止 `Json(json!())` 成功响应；错误响应豁免）。契约即代码 — 改 struct 字段或 wire 形态视为破坏性变更，PR 描述需标注。"

把"Where we're going"里的相关项调整（如果有）。

- [ ] **Step 2: Commit**

`git add CLAUDE.md && git commit -m "docs: orientation reflects protocol typing landed"`

**验收**：未来读 CLAUDE.md 的 AI / 新人能立刻知道协议层规则。

---

## 不做（明确 out of scope）

- 把 `Response.data: Option<Value>` 收紧成 `enum ResponsePayload`（v2 候选，需要破坏 wire）
- 引入 OpenAPI / tRPC / Protobuf（本计划之外）
- 前端 zod / runtime validation（本计划只保后端单边收敛）
- 重写 `gitim-runtime/src/http.rs` 拆分（拆分是独立工程，不在本 plan）
- 给 daemon SSE Event 加新约束（已经 typed，不动）

## 风险与回退

- **Phase 2 改 `Onboard.auth`** 是唯一可能破坏 wire 的位置，必须在 Task 2.1 step 1 里把 wire 形态对齐验证完再继续；如果不能完全对齐，回退到先在 daemon 内部 typed（保留 Value 入口 + 内部立刻 from_value），同样获得 80% 收益但前端零迁移
- **Phase 3 / Phase 4 是增量推进**，每个 method / endpoint 独立 commit，单独可回滚
- **Phase 1 me.json 改造**最危险（落盘 schema），必须在 Task 1.1 step 4 单元测试覆盖 merge 边界后再让 daemon 写盘切过去
- 每个 phase 末跑全量 `cargo test`（CLAUDE.md 测试节奏）

## 自检（落 plan 时）

- [x] 每个 task 不含代码块（feedback memory: plan_no_code）
- [x] 每个 task 给出文件路径 + 做什么 + 验收
- [x] phase 之间有依赖关系说明
- [x] 决策点与执行解耦（Phase 0 先对齐再做）
- [x] 不在 plan 内一次性吞掉所有 30+ method（按 method/endpoint 一个 task）
- [x] 不破坏 wire format 是默认目标，破坏点显式标注
