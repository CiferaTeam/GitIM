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

### SSE 推送频道 members 变更
**What:** 邀请成功后，前端通过 SSE 接收精细化 members 更新，而非全量 refetch `/im/channels`。
**Why:** 当前邀请对话框成功后复用全量 refetch，频道多时开销大；MVP 可接受。
**Context:** 见 `docs/plans/group-chat-invite-members/00-requirements.md` Perf P1。
**Added:** 2026-04-17 via /plan-eng-review

### Release artifact L2 signing (minisign/GPG over SHA256SUMS)
**What:** 当前 release pipeline 用 SHA256SUMS 做 artifact integrity 校验 (L1)。未来升级到 L2:maintainer 用 `minisign` 或 GPG 对 `SHA256SUMS` 文件签名,公钥嵌入 `gitim-updater` binary / 发布在 `CiferaTeam/GitIM` repo 根目录,`install.sh` 和 `gitim-updater` 在校验 SHA 前先验 SHA 文件的签名。
**Why:** L1 只挡 "单独污染 tarball" 攻击场景。如果 attacker 同时拿到 releases repo write token (或 maintainer 机器被攻破),可以同时篡改 SHA 文件和 tarball,L1 就穿了。L2 把信任锚定在 maintainer 私钥,repo 即使被完全控制也挡得住。
**Pros:** 提升 self-update 通路的 threat model 一级;对 agent 工具 (能读 PAT、调 git、跑子进程) 的 RCE 链条再加一道锁。
**Cons:** Maintainer 私钥管理成本 (YubiKey / 密码库);key 丢失/轮换流程;公钥分发冷启动问题 (新用户怎么拿到初始公钥并信任?)。`sigstore` keyless 能规避 key 管理但要求 CI 环境 OIDC,与本地脚本路线冲突。
**Context:** Phase 3 eng-review (2026-04-20) 产出。触发信号:项目受众扩大 / 公网人气起来 / maintainer 机器被定向攻击的证据 / 出现 sigstore 类本地友好的方案。选型推荐优先 `minisign` (单文件 key,tiny binary);GPG 为备选 (复杂度高,但用户生态更熟)。
**Depends on:** 受众量 signal 或攻击证据;公钥首次分发方案确定。
**Added:** 2026-04-20 via /plan-eng-review (cross-compile-release review)

### uptime_secs 真实值
**What:** `GET /runtime/status` 的 `uptime_secs` 字段当前硬编码为 0。需要在 `RuntimeState` 中记录 `started_at: SystemTime`，并在响应构造时计算 `(now - started_at).as_secs()`。
**Why:** Agent 用 `cli status` 做健康检查时，uptime 为 0 让人误以为 runtime 刚刚重启，掩盖了真实的存活时间信息。
**Context:** 硬编码位置：`crates/gitim-runtime/src/cli/cmd_status.rs:55`。注释已说明需要 `RuntimeState` 记 `started_at`。`RuntimeState::default()` 是注入点；HTTP handler 读取并减法即可。
**Added:** 2026-06-12 via architecture audit

### update-agent --clear-env flag
**What:** `update-agent` 子命令当前无法清空 agent 的 `.env` 文件。需要加 `--clear-env` flag，body builder 发送 `dotenv: null`，runtime PATCH handler 删除 `<agent-clone>/.env`。
**Why:** Agent 被重新配置时可能需要彻底移除旧的 secret，但 v1 只支持覆盖写，不支持清空。
**Context:** 设计预留位置：`crates/gitim-runtime/src/cli/cmd_update_agent.rs:182`（注释提及 v2 `--clear-env`）。三态语义（absent / null / set）的 `deser_triple_option` 基础设施已存在，只需在 CLI 层增加 flag 并路由到 `Some(None)`。
**Added:** 2026-06-12 via architecture audit

### Agent "burned" 事件独立 UI 渲染
**What:** `AgentStatusPanel` 当前对未知事件类型（包括 `"burned"`、`"usage"` 等）做字符串 fallback 渲染，导致显示标签缺失。需要为 `"burned"` 事件加专用渲染分支，显示如"Agent 已归档"之类的用户友好信息。
**Why:** Burn 操作后 agent 状态面板无明确视觉反馈，用户不确定操作是否成功。
**Context:** TODO 标记位置：`products/gitim/frontend/src/components/chat/agent-status-panel.tsx:111`（TODO E.3）。该 panel 已有 `"connected"` / `"disconnected"` 等分支，加 `"burned"` 分支是直接扩展。
**Added:** 2026-06-12 via architecture audit

### Cron run 深度链接
**What:** Cron day panel 显示每次 cron run 的结果，但点击无法跳转到对应的 channel/thread。需要在 run 条目上加链接，路由到触发该 run 的频道消息。
**Why:** 用户想排查某次 cron run 的输出时，需要手动去频道里翻找，体验差。
**Context:** 注释位置：`products/gitim/frontend/src/components/crons/cron-day-panel.tsx:27`（"v2 nice-to-have"）。每个 cron run 已有关联的 `channel`，前端可以用 `useNavigate` 跳转到该频道并高亮对应消息。
**Added:** 2026-06-12 via architecture audit
