# 群聊邀请成员 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 打通现有 JoinChannel targets 能力到 HTTP + WebUI，让创建时可选初始成员、创建后群内成员可邀请他人。

**Architecture:** daemon 层 `handle_create_channel` 扩展接受 `invitees`（初始成员集），`handle_join_channel` targets 机制已就绪；gitim-runtime HTTP 两端点透传新字段；webui 新增 `MemberPicker` 组件复用于创建 Dialog 和 InviteDialog，成功后复用既有 `/im/channels` 全量 refetch 刷新成员。

**Tech Stack:** Rust (tokio / axum / serde) · React 19 / TypeScript / Radix UI / Zustand · TDD inline `#[cfg(test)]` for daemon unit tests · manual QA for webui

**约定：**
- 本 plan 遵循用户偏好 `plan_no_code`：只写分工、文件、验收，不写代码。
- TDD 节奏：先红（写失败测试）→ 绿（实现）→ commit。每任务可独立 commit。
- 工作目录：`/Users/lewisliu/ateam/GitIM/.worktrees/group-chat-invite-members`
- 分支：`feature/group-chat-invite-members`

---

## 任务依赖图

```
T1 api 字段  ─┐
              ├→ T2 daemon 测试(红) ─→ T3 handler 实现(绿) ─┐
T5 HTTP create ─┘                                            │
T6 HTTP join  ──────────────────────────────────────────────┤
T4 client 签名 ─→ T5/T6 (两个 HTTP 依赖 client 签名)        │
                                                              ├→ T11 全量 QA
T7 webui client ─→ T8 MemberPicker ─┬→ T9 sidebar 接入 ─────┤
                                     └→ T10 InviteDialog+header┘
```

**并行度**：T1 可独立；T2+T3 一组（daemon 纵向）；T5/T6 依赖 T4；T7~T10 前端可与 T5/T6 并行后期汇合。

---

## Task 1: daemon Command 扩展 invitees 字段

**Files:**
- Modify: `crates/gitim-daemon/src/api.rs:116`（Command::CreateChannel 变体）

**变更描述：**
- `CreateChannel` 变体新增字段 `invitees: Vec<String>`，标注 `#[serde(default)]`
- 不触碰其他 Command 变体

**验收标准：**
- `cargo build -p gitim-daemon` 通过
- `cargo test -p gitim-daemon` 全绿（既有序列化 / handler 测试不破坏）
- 旧 JSON 请求（无 `invitees` 字段）反序列化仍成功

**Steps:**
- [ ] Step 1：编辑 api.rs 加字段
- [ ] Step 2：`cargo build -p gitim-daemon`
- [ ] Step 3：`cargo test -p gitim-daemon` 全绿
- [ ] Step 4：commit `feat(api): CreateChannel 接受 invitees 字段`

---

## Task 2: daemon handle_create_channel 测试先行（红）

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`（inline `#[cfg(test)] mod tests` 末尾）

**变更描述：** 新增 5 个 `#[tokio::test]` 测试用例（仅骨架 + assert，先让它们失败）：

1. `test_create_channel_with_invitees` — 创建时附 invitees=[bob, carol]；assert meta.members == [author, bob, carol]；assert commit 已产生
2. `test_create_channel_invitee_dedup_duplicates` — invitees=[bob, bob]；assert members 只含一个 bob
3. `test_create_channel_invitee_dedup_self` — invitees 含 author 自己；assert members 不重复 author
4. `test_create_channel_invitee_unregistered_rejects` — invitees 含未注册 handle；assert 返回 error；assert meta 文件未产生
5. `test_create_channel_without_invitees` — invitees=[]；**回归验证** author 独自成员的旧行为

**测试复用：**
- 参考 `handlers.rs:1583-1668` 既有 `test_join_channel_*` 的 setup helper（TestEnv / GitStorage 等）

**验收标准：**
- 5 个测试可编译
- 运行 5 个测试全部 **FAIL**（因为 T3 还没改 handler）
- 旧测试不受影响

**Steps:**
- [ ] Step 1：编辑 handlers.rs 写 5 个测试（骨架 + assert，不改 handler）
- [ ] Step 2：`cargo test -p gitim-daemon test_create_channel` 观察 FAIL
- [ ] Step 3：其他既有测试 `cargo test -p gitim-daemon` 仍然绿
- [ ] Step 4：commit `test(daemon): create_channel invitees 测试（红）`

---

## Task 3: daemon handle_create_channel 实现 invitees（绿）

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs:1055-1107`（`handle_create_channel`）
- Modify: 相关调用方 dispatcher（如 `dispatch_command` 或 `handle_command` 处接收 invitees 参数并传递）

**变更描述：**
1. handler 接收 `invitees: Vec<String>` 参数
2. 校验每个 invitee：存在于 `state.users` → 否则返回错误 `"user <x> not registered"` 且不写任何文件
3. 构造 `members = [author] ∪ invitees` 保序去重（author 首位，invitees 按传入顺序去重）
4. 既有 meta.yaml / thread 初始化流程复用；members 字段用新值
5. 单次 git commit 含 meta + thread 两文件

**验收标准：**
- Task 2 的 5 个测试全部 **PASS**
- `cargo test -p gitim-daemon` 全绿
- `cargo test --workspace` 全绿（无下游 break）

**Steps:**
- [ ] Step 1：实现 handler 改动
- [ ] Step 2：`cargo test -p gitim-daemon test_create_channel` 绿
- [ ] Step 3：`cargo test --workspace` 全绿
- [ ] Step 4：commit `feat(daemon): handle_create_channel 支持 invitees`

---

## Task 4: gitim-client Rust `create_channel` 签名扩展

**Files:**
- Modify: `crates/gitim-client/src/client.rs:192-206`（create_channel 方法）
- Modify: `crates/gitim-cli/src/commands/channels.rs:24-31`（cmd_create_channel 调用）

**变更描述：**
- `create_channel` 新增尾参 `invitees: &[String]`，JSON payload 附 `"invitees": invitees`
- `cmd_create_channel` 本期传 `&[]`（CLI 暂不暴露 `-t` 选人，follow-up）

**验收标准：**
- `cargo build --workspace` 通过
- `cargo test --workspace` 全绿
- `gitim create-channel foo` 旧 CLI 行为不变

**Steps:**
- [ ] Step 1：修改 client.rs 签名
- [ ] Step 2：修改 cmd_create_channel 调用
- [ ] Step 3：`cargo build --workspace`
- [ ] Step 4：`cargo test --workspace`
- [ ] Step 5：commit `feat(client): create_channel 接受 invitees`

---

## Task 5: gitim-runtime HTTP `/im/create-channel` 透传 invitees

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:345-369`（CreateChannelRequest + im_create）

**变更描述：**
- `CreateChannelRequest` 新增 `#[serde(default)] invitees: Vec<String>`
- `im_create` 调用 `client.create_channel(..., &req.invitees)`

**验收标准：**
- `cargo build -p gitim-runtime` 通过
- 旧请求（payload 不含 invitees）仍 200
- 新请求 invitees=[] 与无字段行为一致

**Steps:**
- [ ] Step 1：修改 request 结构和 handler
- [ ] Step 2：`cargo build -p gitim-runtime`
- [ ] Step 3：`cargo test --workspace` 全绿
- [ ] Step 4：commit `feat(runtime): /im/create-channel 透传 invitees`

---

## Task 6: gitim-runtime HTTP `/im/join` 透传 targets

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:371-388`（JoinRequest + im_join）

**变更描述：**
- `JoinRequest` 新增 `#[serde(default)] targets: Vec<String>`
- `im_join` 调用 `client.join_channel(&req.channel, &req.targets)` 替代当前硬编码 `&[]`

**验收标准：**
- `cargo build --workspace` 通过
- 旧请求（无 targets）= author 自加入，行为不变
- 新请求带 targets → daemon 侧 `handle_join_channel` 校验 caller 是现有成员再加入 targets

**Steps:**
- [ ] Step 1：修改 request 结构和 handler
- [ ] Step 2：`cargo build --workspace`
- [ ] Step 3：`cargo test --workspace` 全绿
- [ ] Step 4：commit `feat(runtime): /im/join 透传 targets`

---

## Task 7: webui-v2 lib/client.ts joinChannel/createChannel 签名扩展

**Files:**
- Modify: `webui-v2/src/lib/client.ts:72-80`（joinChannel）
- Modify: `webui-v2/src/lib/client.ts`（createChannel 所在位置）

**变更描述：**
- `createChannel(name, displayName?, intro?, invitees?: string[])` — 仅当 invitees 非空时往 payload 附 `invitees`，否则保留原 payload 形态（避免序列化冗余字段）
- `joinChannel(channel, targets?: string[])` — 仅当 targets 非空时往 payload 附 `targets`
- 类型定义同步更新

**验收标准：**
- `cd webui-v2 && npm run build` 通过（tsc + vite 都过）
- 旧调用点（不传第四/第二新参）编译无警告

**Steps:**
- [ ] Step 1：修改 client.ts 两处签名
- [ ] Step 2：`cd webui-v2 && npm run build`
- [ ] Step 3：commit `feat(webui): client.ts createChannel/joinChannel 扩展 invitees/targets`

---

## Task 8: webui-v2 新增 MemberPicker 组件

**Files:**
- Create: `webui-v2/src/components/chat/member-picker.tsx`

**变更描述：**
- Props 契约（review 决议 CQ3）：
  - `allUsers: string[]`
  - `excludeHandlers: string[]`（自己 / 已在群）
  - `value: string[]`（当前已选）
  - `onChange: (selected: string[]) => void`
- 组件内部：
  - 文本搜索框，子串过滤（大小写不敏感）
  - 候选列表 = allUsers − excludeHandlers；搜索后再过滤
  - 每项 checkbox；选中态绑定 value
  - 已选集合在顶部/底部用 chip 显示，可点 chip 取消
- 视觉：**先读 `DESIGN.md`** 确定 spacing / color / typography；选择 Radix UI 现有组件（Checkbox / Input）与项目保持一致
- 不直接消费 `useChatStore`（保留可测性）

**验收标准：**
- `cd webui-v2 && npm run build` 通过
- 组件可渲染（在 Storybook 或临时 route 手测；若无 storybook 则放到下一 task 里集成时验证）
- 过滤 / 多选 / 排除三个交互都手测通过

**Steps:**
- [ ] Step 1：读 `DESIGN.md` 记笔记视觉参数
- [ ] Step 2：创建组件文件
- [ ] Step 3：`npm run build`
- [ ] Step 4：commit `feat(webui): MemberPicker 多选用户组件`

---

## Task 9: webui-v2 sidebar 创建对话框接入 MemberPicker

**Files:**
- Modify: `webui-v2/src/components/chat/sidebar.tsx:71-227`

**变更描述：**
- 新 state：`createInvitees: string[]`
- `resetCreateForm` 中重置 `setCreateInvitees([])`
- 在 create Dialog 表单"Introduction"字段下方新增"Invite members (optional)"区域
- 区域内容：`<MemberPicker allUsers={users} excludeHandlers={[currentUser]} value={createInvitees} onChange={setCreateInvitees} />`
- `handleCreateChannel` 调用 `client.createChannel(name, displayName, intro, createInvitees)`
- 错误反馈复用现有 `createError` inline 显示（review 决议 CQ2）

**验收标准：**
- `npm run build` 通过
- 手测：
  - 创建频道不选人 → 正常（回归）
  - 创建频道选 1-2 人 → 新频道 members 含 [self, ...invitees]
  - MemberPicker 候选不含 self
  - 提交失败（如后端错误）错误 inline 显示、Dialog 不关闭

**Steps:**
- [ ] Step 1：修改 sidebar.tsx
- [ ] Step 2：`npm run build`
- [ ] Step 3：启动 daemon + webui，手测三个场景
- [ ] Step 4：commit `feat(webui): 创建频道时可选邀请成员`

---

## Task 10: webui-v2 InviteDialog + ChannelHeader 接入

**Files:**
- Create: `webui-v2/src/components/chat/invite-dialog.tsx`
- Modify: `webui-v2/src/components/chat/header.tsx:52-95`（ChannelHeader DropdownMenuContent）

**变更描述：**

**InviteDialog 组件：**
- Props：`open, onOpenChange, channel, allUsers, excludeHandlers, onInvited`
- 内部：MemberPicker + "Invite" 按钮 + 错误区
- 提交逻辑：调 `client.joinChannel(channel, selected)`；成功 → `onInvited()`（让父组件 refetch channels）→ 关闭；失败 → inline 显示 error 不关闭

**ChannelHeader 改动：**
- 计算 `excludeHandlers = [currentUser, ...members]`
- DropdownMenuContent 顶部加"Invite members"菜单项（图标 + 文字），点击打开 InviteDialog
- 父级/内部维护 InviteDialog 的 open 状态
- `onInvited` 回调触发全量 `client.channels()` refetch → `setChannels(...)`（复用 review 决议 A4 的既有 pattern）

**DESIGN.md 合规：** 所有视觉参数复查 DESIGN.md

**验收标准：**
- `npm run build` 通过
- 手测：
  - 点"Invite members" → Dialog 打开
  - 已在群者不在 MemberPicker 候选中
  - 选人提交 → Dialog 关闭，DropdownMenu 的 members 数增加
  - 后端错误 → inline error，Dialog 保留
- channel.members 刷新后立即反映（无需手动刷页面）

**Steps:**
- [ ] Step 1：创建 invite-dialog.tsx
- [ ] Step 2：修改 header.tsx 接入
- [ ] Step 3：`npm run build`
- [ ] Step 4：手测 4 个场景
- [ ] Step 5：commit `feat(webui): 频道内成员可邀请他人入群`

---

## Task 11: 全量回归 + E2E QA

**Files:** 无代码改动

**变更描述：** 跑完整测试 + 端到端手测 + DESIGN.md 合规核查

**验收标准：**
- `cargo test --workspace` 全绿（包含 ~270 既有 + 5 新 create_channel 测试）
- `cargo clippy --workspace` 无 warning（或仅有既有的可接受 warning）
- `cd webui-v2 && npm run build` 通过
- 端到端 5 个场景全部通过：
  1. **回归**：无邀请创建频道 → members=[self]
  2. **创建选人**：创建时邀 bob + carol → members=[self, bob, carol]，bob / carol 能看到并进入频道
  3. **群内加人**：进入既有频道 → 右上角 → Invite → 邀 dave → members 列表刷新含 dave
  4. **排除已在群**：第 3 步重新打开 Invite 对话框，dave 不在可选列表
  5. **错误路径**：手 curl `/im/create-channel` invitees=["nonexistent"] → 返回 4xx 错误，meta 文件未生成
- DESIGN.md 合规：新 MemberPicker / InviteDialog 的 spacing、颜色、typography 符合

**Steps:**
- [ ] Step 1：`cargo test --workspace`
- [ ] Step 2：`cargo clippy --workspace`
- [ ] Step 3：`cd webui-v2 && npm run build`
- [ ] Step 4：端到端 5 个场景手测
- [ ] Step 5：DESIGN.md 视觉核对
- [ ] Step 6：如有小问题回到相应 task 修；如全绿则 commit `chore: Phase 5 QA 完成` 或无需 commit

---

## Self-Review

**Spec coverage** — 需求共识文档每条都能映射到 task：
- 创建时可选人 → T1/T2/T3 + T5 + T7 + T8 + T9
- 创建后加人 → T4 + T6 + T7 + T8 + T10
- 被动入群语义 → T3（daemon 直接写 members）
- Thread 不留痕 → 默认行为，无需额外 task
- 权限（群内成员） → T6 + daemon 既有 join 校验，无新代码

**Architecture 决议映射**：
- A1 serde 兼容性 → T1/T5/T6（均 `#[serde(default)]`）
- A2 签名 fan-out → T4
- A3 unregistered reject → T2 test 4 + T3 实现
- A4 refetch 刷新 → T10

**Code Quality 决议映射**：
- CQ1 dedup author 首位 → T2 test 2,3 + T3 实现
- CQ2 inline 错误 → T9 / T10 手测验收
- CQ3 MemberPicker contract → T8

**Placeholder 扫描**：无"TBD"/"详见实现"/"类似前面的 task"。

**Type 一致性**：
- `invitees` / `targets` 命名沿用（daemon invitees ↔ client invitees / webui invitees；join 用 targets 保留既有命名）
- `MemberPicker` props 名在 T8 定义，T9/T10 使用一致

---

## Execution Handoff

Plan 已完成并保存至 `docs/plans/group-chat-invite-members/01-plan.md`。Phase 5 两种执行方式：

1. **Subagent-Driven**（默认，SOP 推荐）：每个 task 分派一个 subagent 实现 + 自动走 spec/quality 两道 review
2. **Inline 执行**：在当前会话按 batch 执行并在 checkpoint 处等你确认

默认走 Subagent-Driven；想走 Inline 告诉我。
