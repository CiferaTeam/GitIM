# Agent #general Auto-Join 可选化

> **For agentic workers:** 用 `superpowers:subagent-driven-development` 或 `superpowers:executing-plans` 按任务推进。每个 task 内的 `- [ ]` 用于进度追踪。

**Goal:** 让前端在创建 agent 时通过 checkbox 控制是否自动加入 #general,默认勾选(保持现有行为)。

**Architecture:** 在 daemon 的 `Request::Onboard` 加 `join_general: bool`(`#[serde(default = "default_true")]` 保留向后兼容),`handle_onboard` 把 gate 从 `if created` 改成 `if created && join_general`。`gitim-client` 的 `onboard()` 方法新增 bool 参数,所有 caller(CLI human onboard、runtime workspace owner provision、runtime agent provision)显式传值。Runtime 的 `AgentAddRequest` 加可选字段(默认 true),`provision_agent` 接收并透传到 client。前端 `add-agent-dialog.tsx` 加 checkbox(默认勾选),`client.ts addAgent()` 把布尔值带进 POST body。

**Tech Stack:** Rust(daemon / runtime / CLI / client) + React 19 + TypeScript(frontend)。

**Backward compat 策略:**
- daemon `Request::Onboard.join_general` 用 `#[serde(default = "default_true")]` — 老客户端不传字段时仍 auto-join
- runtime `AgentAddRequest.join_general` 用 `Option<bool>` + `#[serde(default)]`,handler 用 `unwrap_or(true)`
- client / runtime / CLI 的 Rust 函数签名增加显式参数,所有 caller 必须更新(编译器保证不漏)

---

## 文件清单

**Rust (4 个 crate):**
- `crates/gitim-daemon/src/api.rs` — `Request::Onboard` 加 `join_general` 字段 + 默认值函数
- `crates/gitim-daemon/src/onboard.rs` — `handle_onboard` 加参数,改 gate;新增反向测试
- `crates/gitim-daemon/src/handlers/mod.rs` — dispatch 透传新参数
- `crates/gitim-client/src/client.rs` — `onboard()` 方法签名加参数
- `crates/gitim-cli/src/commands/onboard.rs` — 两处 caller 显式传 `true`
- `crates/gitim-runtime/src/agent.rs` — `provision_agent` 加参数;两处 caller 改写
- `crates/gitim-runtime/src/http.rs` — `AgentAddRequest` 加字段,`agents_add` handler 透传

**Frontend (1 个产品):**
- `products/gitim/frontend/src/lib/client.ts` — `addAgent()` 加参数 + 写入 body
- `products/gitim/frontend/src/components/management/add-agent-dialog.tsx` — checkbox + state + submit

---

## Task 1: Daemon — `Request::Onboard` 加 `join_general` 字段

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs`(`Request::Onboard` variant 附近)
- Modify: `crates/gitim-daemon/src/onboard.rs`(`handle_onboard` + 现有 `if created` gate + 测试模块)
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`(dispatch)

**Steps:**

- [ ] **Step 1.1:** 在 `onboard.rs` 测试模块新增反向测试 `test_onboard_skip_general_when_join_general_false`:第二个 bot onboard 时 `join_general=false`,断言 `channels/general.meta.yaml` 的 `members` 不含其 handler、`channels/general.thread` 不含其 join 事件。其他用 `handle_onboard` 的现有测试在调用处显式传 `true`。
- [ ] **Step 1.2:** `cargo test -p gitim-daemon onboard --no-run` 应编译失败(参数缺失/字段缺失)— 锁住红测试存在。
- [ ] **Step 1.3:** `Request::Onboard` 加 `#[serde(default = "default_true")] join_general: bool` 字段;在该文件内补 `fn default_true() -> bool { true }`(若尚无)。
- [ ] **Step 1.4:** `handle_onboard` 函数签名追加 `join_general: bool` 参数,把 `if created` 改成 `if created && join_general`(`onboard.rs:114-120`)。
- [ ] **Step 1.5:** `handlers/mod.rs:223-228` 的 dispatch arm 加上 `join_general` 解构并透传给 `handle_onboard`。
- [ ] **Step 1.6:** `cargo test -p gitim-daemon onboard` 应通过,包括既有 `test_onboard_new_user_joins_general` 和新增反向测试。
- [ ] **Step 1.7:** Commit: `feat(daemon): add join_general flag to Onboard request`。

**Acceptance:** `cargo test -p gitim-daemon onboard` 全绿;反向测试断言新 user 不在 general members,正向测试不变。

---

## Task 2: Client — `onboard()` 方法签名加参数 + 更新所有 caller

**Files:**
- Modify: `crates/gitim-client/src/client.rs`(`onboard()` 方法,目前 4 个参数 + JSON body)
- Modify: `crates/gitim-cli/src/commands/onboard.rs:439`(human onboard caller)
- Modify: `crates/gitim-cli/src/commands/onboard.rs:505`(human onboard caller,另一模式)
- Modify: `crates/gitim-runtime/src/agent.rs:92`(workspace owner provision caller)
- Modify: `crates/gitim-runtime/src/agent.rs:186`(agent provision caller 占位,Task 3 再接前端值)

**Steps:**

- [ ] **Step 2.1:** `client.rs` 的 `onboard()` 新增 `join_general: bool` 形参,放在 `guest` 之后;JSON body 里加 `"join_general": join_general`。
- [ ] **Step 2.2:** `cargo check --workspace` — 编译应失败,列出所有 caller。
- [ ] **Step 2.3:** 改 `gitim-cli/src/commands/onboard.rs:439` 和 `:505`,在末尾加 `true`(human onboard 永远 auto-join)。
- [ ] **Step 2.4:** 改 `gitim-runtime/src/agent.rs:92`(workspace owner)和 `:186`(agent provision),都先传 `true`(agent 路径在 Task 3 接入前端值)。
- [ ] **Step 2.5:** `cargo check --workspace` 应通过。
- [ ] **Step 2.6:** `cargo test -p gitim-client && cargo test -p gitim-cli` 应全绿。
- [ ] **Step 2.7:** Commit: `feat(client): thread join_general through onboard()`。

**Acceptance:** workspace 编译干净;client/cli 测试通过;runtime 行为暂时不变(都传 true)。

---

## Task 3: Runtime — `AgentAddRequest` 加字段、`provision_agent` 接收并透传

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`(`AgentAddRequest` 结构体 + `agents_add` handler)
- Modify: `crates/gitim-runtime/src/agent.rs`(`provision_agent` 函数签名 + agent path 的 onboard caller)

**Steps:**

- [ ] **Step 3.1:** 在 `http.rs` 找一个 add_agent 相关的现有 #[tokio::test] 集成测试(若无则跳到 3.3)。新增/扩展用例:POST `/agents/add` body 含 `"join_general": false`,断言 daemon 处理后 `channels/general.meta.yaml` 不含新 agent handler。
- [ ] **Step 3.2:** `cargo test -p gitim-runtime --test <文件>` 该用例应失败(字段未识别 / agent 仍被加入 general)。
- [ ] **Step 3.3:** `AgentAddRequest` 新增 `#[serde(default)] join_general: Option<bool>` 字段。
- [ ] **Step 3.4:** `provision_agent` 函数签名新增 `join_general: bool` 参数(独立于 `&AgentConfig`,因为该值是临时 provisioning 决策,不持久化到 `me.json`)。函数内部 `client.onboard(...)` 调用从传 `true` 改为传 `join_general`。
- [ ] **Step 3.5:** `agents_add` handler 调用 `provision_agent` 时传 `req.join_general.unwrap_or(true)`。
- [ ] **Step 3.6:** `cargo test -p gitim-runtime` 应通过(含新增反向测试)。
- [ ] **Step 3.7:** Commit: `feat(runtime): expose join_general on POST /agents/add`。

**Acceptance:** HTTP body 不传字段 → 默认 join(向后兼容);传 `false` → 新 agent 不在 general。

---

## Task 4: Frontend — Add Agent Dialog 增加 checkbox

**Files:**
- Modify: `products/gitim/frontend/src/lib/client.ts`(`addAgent()` 函数)
- Modify: `products/gitim/frontend/src/components/management/add-agent-dialog.tsx`(form state + 渲染 + submit)

**Steps:**

- [ ] **Step 4.1:** `client.ts` 的 `addAgent()` 在参数列表末尾加 `joinGeneral: boolean = true`(尾部带默认值,不破坏既有调用)。POST body 加 `join_general: joinGeneral`。
- [ ] **Step 4.2:** `add-agent-dialog.tsx`:
  - 新增 state `const [joinGeneral, setJoinGeneral] = useState(true)`(默认勾选)。
  - 在 Introduction 字段下、Provider 字段上的位置(form 中段)插入一个 checkbox row。Checkbox 和 label 用项目里已有的 `@radix-ui/react-checkbox` 或 `@/components/ui/checkbox`(按 DESIGN.md 选,先扫一遍现有 dialog 的同类用法)。Label 文案:`Auto-join #general channel`,helper text:`Uncheck if this agent should only post in specific channels.`。
  - submit handler 调 `addAgent(...)` 时把 `joinGeneral` 作为最后参数传进去。
  - 关闭/重置 dialog 时 reset 回 `true`。
- [ ] **Step 4.3:** 在 frontend 跑 `pnpm typecheck`(或 `tsc --noEmit`,按 `package.json` 的 script),确认无类型错误。
- [ ] **Step 4.4:** 跑 `pnpm build`(或对应 build 命令),确认产线构建通过。
- [ ] **Step 4.5:** Commit: `feat(frontend): add join-general checkbox to Add Agent dialog`。

**Acceptance:** Type check + build 通过;dialog 默认勾选;不勾选时 POST body 含 `join_general: false`(可在 devtools 网络面板验证,见 Task 5)。

---

## Task 5: 集成验证

**Steps:**

- [ ] **Step 5.1:** Workspace 全量 `cargo test`,确认无 regression(包括 daemon、runtime、cli、core、sync、client)。
- [ ] **Step 5.2:** 前端构建复查 `pnpm build`(若 Task 4 已跑过,可跳)。
- [ ] **Step 5.3 (可选,手测):** 在本地 workspace 启 runtime + 前端,创建一个新 agent 并把 checkbox 取消勾选,观察:
  - 浏览器 DevTools Network 面板:POST `/agents/add` body 含 `"join_general": false`。
  - 创建后查看 `channels/general.meta.yaml`,新 agent handler 不在 `members`。
  - 创建后查看 `channels/general.thread`,无该 agent 的 join 事件行。
  - 反过来勾选另一个新 agent,确认按旧路径 auto-join。
- [ ] **Step 5.4:** 若手测发现问题,创建追加 task;若无,准备进 Phase 6 review。

**Acceptance:** 全量 cargo test + frontend build 通过;手测两条路径(勾选 / 不勾选)都符合预期。

---

## 注意事项

- **不动 `me.json` schema:** `join_general` 是 provisioning 时的一次性决策,不持久化到 agent 配置文件。
- **不动 daemon 人类 onboard 默认:** CLI 的 `gitim onboard` 命令不暴露此 flag(human 一直是 `true`)。如果未来要给 human 也加可选,再单独开 task。
- **测试粒度:** 中间步骤只跑 scoped 测试(`-p <crate>` 或 `--test <file>`);Task 5 才跑全量(参考 CLAUDE.md "跑测试的节奏")。
- **Worktree:** 所有改动在 `/Users/lewisliu/ateam/GitIM/.claude/worktrees/nice-lovelace-4a8c61/` 内进行,不要切回主仓。
- **每个 Task 的 commit:** 单独提交(参考 `feedback_commit_each_task`)。
