# TODOS

## Pending

### Social Cognition Layer for Coordinator Prompt
**What:** 当记忆系统就绪后，给 coordinator prompt 加上 Social Cognition 层 + 记忆工具使用说明。
**Why:** 协调者设计的最终形态包含社会认知（agent 能力画像、信任度、历史表现），但当前缺少持久化能力。
**Pros:** 协调者获得路由学习能力，实现从 subagent 到 channel 委托的渐进演化。
**Cons:** 依赖记忆系统设计，可能需要同时改 prompt 和工具接口。
**Context:** 设计文档 (`docs/agent-coordinator-prompt-design.md`) 的 Deferred Layer 部分已有内容设想。Codex 在 office-hours 中提出了"社会化协调器"框架作为更远期的方向。
**Depends on:** 记忆系统研究完成。
**Added:** 2026-04-13 via /plan-eng-review

### channels/*.meta.yaml 并发合并
**What:** 给 `gitim-sync/src/conflict.rs` 增加 `.meta.yaml` 冲突处理，对 `members: Vec<String>` 做 set-union 合并。
**Why:** 现冲突解决只覆盖 `.thread` 行号。两个成员同时邀请不同人会触发 git 冲突 marker，需手工解决。群聊邀请成员功能上线后此风险暴露面增大。
**Context:** 见 `docs/plans/group-chat-invite-members/00-requirements.md` 非目标段。
**Added:** 2026-04-17 via /plan-eng-review

### SSE 推送频道 members 变更
**What:** 邀请成功后，前端通过 SSE 接收精细化 members 更新，而非全量 refetch `/im/channels`。
**Why:** 当前邀请对话框成功后复用全量 refetch，频道多时开销大；MVP 可接受。
**Context:** 见 `docs/plans/group-chat-invite-members/00-requirements.md` Perf P1。
**Added:** 2026-04-17 via /plan-eng-review

### webui-v2 前端测试基建
**What:** webui-v2 引入 vitest + 至少一个 MemberPicker 等组件的 unit test 范例。
**Why:** 群聊邀请功能落地时发现前端缺少测试基建，只能靠 cargo test 覆盖后端。长期看前端逻辑（搜索过滤、多选、排除）应有单元测试。
**Context:** 见 `docs/plans/group-chat-invite-members/00-requirements.md` 测试要点。
**Added:** 2026-04-17 via /plan-eng-review

### gitim-runtime HTTP 层 integration test
**What:** `crates/gitim-runtime/src/http.rs` 加 integration test，覆盖 `/im/create-channel` 带 `invitees`、`/im/join` 带 `targets`、以及两个 endpoint 的 backward compat（旧请求无新字段）。
**Why:** 群聊邀请 feature 的 HTTP 层只是 2-行透传，风险低，所以本期 defer 测试。但历史 bug（`/im/join` 把 targets 硬编码为 `&[]`）正是这种 regression。daemon 测试覆盖不到 HTTP 层布线。
**Context:** Phase 6 code review (Claude) Important-I2. runtime tests/ 目录已有 provision / poller 模式可参考。
**Added:** 2026-04-17 via /plan-eng-review (Phase 6 finding)
