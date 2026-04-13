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
