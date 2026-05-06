# Protocol Typing Design

## 现状（来自 2026-05-06 review）

GitIM 的层间协议是**单向类型化**：请求侧 typed，响应侧全部开口。具体表现：

- `gitim-daemon::api::Request` 是 30+ 变体的 tagged enum，类型完整 ✓
- `gitim-daemon::api::Event`（SSE）是 11 个 tagged 变体，类型完整 ✓
- `gitim-daemon::api::Response.data: Option<serde_json::Value>` ✗ — 协议契约的根开口
- `gitim-client::types::ApiResponse.data: Option<Value>` ✗ — 镜像开口
- daemon handlers（`card_handlers`、`onboard`、`handlers/*`）全部用 `Response::success(json!({...}))` 现场拼响应
- `gitim-runtime::http`（3283 行）有 12+ 个 `XxxRequest` typed struct，但只有 1 个 `HealthResponse`；其余 endpoint 全部 `Json(serde_json::json!({...}))`
- `Request::Onboard.auth: serde_json::Value` 是协议黑洞，onboard.rs 35 处 Value 操作 + 6 处 `from_value()` 二阶段反序列化
- 36 处 `.get("...").and_then(as_str)` 链式访问散落于 runtime（读 me.json、读 GitHub `/user`、转手响应）

**症状**：daemon ↔ runtime ↔ CLI ↔ 前端任何一段加字段、改字段名，编译器全程沉默；前端 `Record<string, unknown>` 无法对齐后端实际 shape；新加 git provider / endpoint 时无契约校验。

## 目标

让协议契约**对称 typed**：每个请求都有结构化响应，编译器在改动时给出回响。**不追求一次性全量改造**，目标是建立类型化的入口 + 守门规则，阻止债务继续扩散，并按热点逐步收敛存量。

## 不做

- **不动 wire format**：JSON 仍是传输层，serde 仍是序列化器，前端不需要改 fetch 路径
- **不重写 IPC 协议**：`Request` enum、`Event` enum、`Response { ok, data, error }` 信封保留
- **不引入 OpenAPI / Protobuf / tRPC** — 重型方案不在 v1 scope
- **不强制前端改类型**：前端的 ts 接口可以独立演进，本计划只保证后端单边 source of truth 收敛
- **不动合理的 Value 用法**：LLM 输出、第三方 webhook 透传、用户可扩展的 me.json 字段保留 Value

## 设计要点

### 1. Daemon Response payload 增量 typed

策略：保留 `Response.data: Option<Value>` 作为信封，但**每个 Request method 都有一个对应的 Response struct**。Handler 不再 `json!()` 现场拼，而是构造 struct 后 `serde_json::to_value()` 灌进 data。客户端通过泛型辅助方法 `parse_data::<T>()` 反序列化回 struct。

收益：handler 改字段会被自身 struct 编译期约束；客户端按需走 typed 路径；wire format 不动，旧客户端零迁移；可逐 endpoint 推进。

后续可选：把 `data` 从 `Value` 收紧成 `enum ResponsePayload` 按 method tagged — **本计划不做**，留作 v2。

### 2. `Onboard.auth` 改成 enum

把 `auth: serde_json::Value` 改成 `auth: AuthPayload`，`AuthPayload` 是 tagged enum：`GitLocal { handler, display_name }` / `GitHub { token }` / `GitLab { token, base_url }` / `Gitea { token, base_url }`。

收益：新 git provider 必须扩 enum，编译器全链路提示；onboard.rs 30+ 处 Value 操作收敛到一次模式匹配；测试用例不再手拼 JSON。

风险：前端发请求的 JSON shape 必须配合调整 — 通过 serde 的 `tag = "git_server"` + 对应字段映射可保 wire 兼容。

### 3. Runtime HTTP 响应 typed struct

每个 endpoint 配一个 `#[derive(Serialize)] struct XxxResponse`（成功响应）和共用的 `ErrorBody`（错误响应）。Handler 用 `Json(XxxResponse { ... })` 替代 `Json(json!({...}))`。

存量推进顺序按"出血量"：
1. `/im/init`、`/im/me`（onboarding 关键路径，已经在用 Value 链读 me.json）
2. `/workspaces/*`（多消费者：CLI + WebUI）
3. `/agents/*`（PATCH agent 是高频改动点）
4. `/runtime/*`（更新自身、状态查询）
5. 其余 endpoint

### 4. me.json 落盘 schema typed

`me.json`（`<agent-clone>/.gitim/me.json`）目前两侧（写：daemon onboard.rs；读：runtime http.rs `/im/me`）都按 Value 字段访问，CLAUDE.md 已记录"采用 merge 语义防抹掉 github_email"。这套 merge 语义本身需要 typed schema 才能在编译期保证。

把 `MeJson` 定义在 `gitim-core`，daemon 和 runtime 都从 core 引用，落盘前 deserialize 到 struct，写盘前 serialize 自 struct。merge 通过 struct 字段层面的 `Option` + 显式合并函数实现。

### 5. 守门规则

光改存量不够 — 必须有规则阻止新债：

- **CLAUDE.md 加一条**：新 endpoint 的成功响应必须是 typed struct；`json!()` 仅允许出现在错误体或 v1 既存代码
- **CI 加 grep guard**（best-effort）：在 `crates/gitim-runtime/src/http.rs` 和 daemon handlers 中检测新增 `Json(serde_json::json!(` 的 diff，如果是新增则警告 reviewer
- **测试约束**：每个 typed Response struct 配一个 `#[test]` 验证 `serde_json::to_value()` 后的 JSON 结构与既存契约一致（防止 typed 化时偷偷改了 wire 字段名）

## 决策点（需要在 plan 执行前确认）

1. **是否保留 `Response.data: Value` 信封不动**？本设计选"是"。如果后续要收紧成 `enum ResponsePayload`，可以增量做。
2. **`AuthPayload` 改造范围是否包含 Gitea/GitLab**？v1 实际只用 GitLocal + GitHub，但 enum 可以预留另外两个变体（CLAUDE.md non-goals 已说明 v1 不做 GitLab/Gitea，但协议层可以先 typed 化避免日后再改 wire format）。
3. **`MeJson` 放 `gitim-core` 还是新建 `gitim-protocol` crate**？本设计倾向放 `gitim-core`（已经是共享类型 crate），不引入新 crate。
4. **存量 endpoint 改造速度**：一次一个 endpoint 推进，每个 endpoint 一个 PR，还是一次一类（如所有 `/agents/*` 一起）？建议按 endpoint 一次一个，PR 小，回滚易。

## 验收信号

落地一个 phase 后能看到的可观测变化：

- daemon handler 改了响应字段，CLI 或 runtime 编译器立刻提示
- 新 endpoint review 时不会再看到 `Json(json!({...}))`
- onboard.rs 的 Value 操作从 35 处降到个位数（剩下的是合理的扩展字段）
- 新加 git provider 时，搜 `match auth_payload` 就能找全所有需要扩的位置
- 前端新建 ts type 时可以从后端 struct 拷过去（结构能对得上）
