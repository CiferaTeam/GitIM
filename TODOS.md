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

### WebUI `activeSlug` 失联时的自动 fallback
**What:** `products/gitim/frontend/src/hooks/use-workspace-store.ts`: 当 `activeSlug` 引用的 workspace 被其它客户端(或另一台机)删除后,poll 轮询 / SSE 重连发现 404/stream 关闭时,应自动 trigger `fetchAll()` 并切到第一个可用 workspace。当前仅在 `fetchAll` 内部修复(启动 + 本地 create/remove 时触发),线上 workspace 消失时 UI 卡住。
**Why:** 多设备或多 runtime 实例场景下 workspace 列表会异步变化。
**Context:** 2026-04-18 Codex review P2 for multi-workspace-runtime feature。`use-agent-activity.ts:56` 的 `onerror` 当前是 no-op。
**Added:** 2026-04-18 via /plan-eng-review (multi-workspace review)

### Release artifact L2 signing (minisign/GPG over SHA256SUMS)
**What:** 当前 release pipeline 用 SHA256SUMS 做 artifact integrity 校验 (L1)。未来升级到 L2:maintainer 用 `minisign` 或 GPG 对 `SHA256SUMS` 文件签名,公钥嵌入 `gitim-updater` binary / 发布在 `CiferaTeam/GitIM` repo 根目录,`install.sh` 和 `gitim-updater` 在校验 SHA 前先验 SHA 文件的签名。
**Why:** L1 只挡 "单独污染 tarball" 攻击场景。如果 attacker 同时拿到 releases repo write token (或 maintainer 机器被攻破),可以同时篡改 SHA 文件和 tarball,L1 就穿了。L2 把信任锚定在 maintainer 私钥,repo 即使被完全控制也挡得住。
**Pros:** 提升 self-update 通路的 threat model 一级;对 agent 工具 (能读 PAT、调 git、跑子进程) 的 RCE 链条再加一道锁。
**Cons:** Maintainer 私钥管理成本 (YubiKey / 密码库);key 丢失/轮换流程;公钥分发冷启动问题 (新用户怎么拿到初始公钥并信任?)。`sigstore` keyless 能规避 key 管理但要求 CI 环境 OIDC,与本地脚本路线冲突。
**Context:** Phase 3 eng-review (2026-04-20) 产出。触发信号:项目受众扩大 / 公网人气起来 / maintainer 机器被定向攻击的证据 / 出现 sigstore 类本地友好的方案。选型推荐优先 `minisign` (单文件 key,tiny binary);GPG 为备选 (复杂度高,但用户生态更熟)。
**Depends on:** 受众量 signal 或攻击证据;公钥首次分发方案确定。
**Added:** 2026-04-20 via /plan-eng-review (cross-compile-release review)

### release.sh / install.sh shellcheck lint 机制
**What:** `release.sh` 在 cross-compile 改造后长到 ~200 行 (4 target loop + SHA 生成 + docker smoke test + fail-fast),`install.sh` 也变大。加 `shellcheck` lint 作为 release 前 sanity check,或引入 `bats` 做最小 shell 集成测试。
**Why:** 长 shell 脚本的经典坑 (quoting 缺失、unset var、subshell 环境泄漏、`set -e` 在 pipeline 中失效等) 只有 lint 能抓。现在改动靠人工 review,量涨之后会漏。
**Pros:** 便宜,shellcheck 一条命令;CI 友好;lint 修复通常小改动。
**Cons:** 可能和现有脚本风格有冲突 (需要一次性全面修);学习曲线低。
**Context:** Phase 3 eng-review (2026-04-20) 产出。本次 plan 选择 defer,因为不想在 cross-compile feature scope 内扩散。未来 release.sh 再做大改时顺带引入。
**Depends on:** 无。任何时候都能单独做。
**Added:** 2026-04-20 via /plan-eng-review (cross-compile-release review)
