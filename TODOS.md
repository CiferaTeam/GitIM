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

### cell-frontend 前端测试基建
**What:** `products/cell/frontend` 引入 vitest + 至少一个 MemberPicker 等组件的 unit test 范例。
**Why:** 群聊邀请功能落地时发现前端缺少测试基建，只能靠 cargo test 覆盖后端。长期看前端逻辑（搜索过滤、多选、排除）应有单元测试。
**Context:** 见 `docs/plans/group-chat-invite-members/00-requirements.md` 测试要点。
**Added:** 2026-04-17 via /plan-eng-review

### gitim-runtime HTTP 层 integration test
**What:** `crates/gitim-runtime/src/http.rs` 加 integration test，覆盖 `/im/create-channel` 带 `invitees`、`/im/join` 带 `targets`、以及两个 endpoint 的 backward compat（旧请求无新字段）。
**Why:** 群聊邀请 feature 的 HTTP 层只是 2-行透传，风险低，所以本期 defer 测试。但历史 bug（`/im/join` 把 targets 硬编码为 `&[]`）正是这种 regression。daemon 测试覆盖不到 HTTP 层布线。
**Context:** Phase 6 code review (Claude) Important-I2. runtime tests/ 目录已有 provision / poller 模式可参考。**2026-04-18 update**：multi-workspace 改造顺手补了 `tests/http_workspaces.rs` + `tests/multi_workspace.rs`(共 +24 tests),workspace CRUD + SSE 层已覆盖。剩余 `/im/*` `/agents/*` nested 路由的端到端 integration 覆盖仍缺。
**Added:** 2026-04-17 via /plan-eng-review (Phase 6 finding)

### Global `GET/DELETE /workspaces/:slug` 不走 `WorkspaceSlug` 校验
**What:** `crates/gitim-runtime/src/http.rs:1603, 1641` 两个全局 workspace routes 直接用 `axum::extract::Path<String>`,跳过 `slug::validate` 的 regex。nested 路由 `/workspaces/:slug/...` 走 extractor,会返 400;两个全局路由对非法 slug 返回 404。
**Why:** 验证一致性。非法 slug 两条路返回不同状态码,路由表对 API 消费者语义不统一。
**Pros:** 小改动,路由更整齐。
**Cons:** 实际非法 slug 都到不了任何有意义的逻辑(HashMap lookup 必 miss),所以现状不产生 bug,只是 API 面的美观问题。
**Context:** 2026-04-18 Codex review P2 for multi-workspace-runtime feature。
**Added:** 2026-04-18 via /plan-eng-review (multi-workspace review)

### WebUI `activeSlug` 失联时的自动 fallback
**What:** `products/cell/frontend/src/hooks/use-workspace-store.ts`: 当 `activeSlug` 引用的 workspace 被其它客户端(或另一台机)删除后,poll 轮询 / SSE 重连发现 404/stream 关闭时,应自动 trigger `fetchAll()` 并切到第一个可用 workspace。当前仅在 `fetchAll` 内部修复(启动 + 本地 create/remove 时触发),线上 workspace 消失时 UI 卡住。
**Why:** 多设备或多 runtime 实例场景下 workspace 列表会异步变化。
**Context:** 2026-04-18 Codex review P2 for multi-workspace-runtime feature。`use-agent-activity.ts:56` 的 `onerror` 当前是 no-op。
**Added:** 2026-04-18 via /plan-eng-review (multi-workspace review)

### `recover_from_config` 对同 path 多 slug 条目的去重
**What:** `crates/gitim-runtime/src/http.rs` `recover_from_config` 只按 slug 去重。如果手改 `~/.gitim/runtime.json` 塞了同 path 不同 slug,两个 recovery 任务会并发操作同一 `.gitim-runtime/human` + agents 子目录,造成 daemon 双开。
**Why:** `POST /workspaces` 已拒同 path(本次改造),但 hand-edit config.json 是常见操作,边界仍需守护。
**Context:** 2026-04-18 Codex review P2 for multi-workspace-runtime feature。
**Added:** 2026-04-18 via /plan-eng-review (multi-workspace review)

### `recover_agents_for_workspace` 的 `.expect("ws exists")` 改为 warn+skip
**What:** `crates/gitim-runtime/src/http.rs:1442, 1452, 1485` 的 `.expect("ws exists")` 在 recovery storm 下可能 panic 整个 runtime。改为 `tracing::warn!` + `continue`,保持 best-effort 语义。
**Why:** 现状 invariant 对(caller 先 insert ctx 再 call),但 future-proof 一下。
**Context:** 2026-04-18 Claude review P2 for multi-workspace-runtime feature。
**Added:** 2026-04-18 via /plan-eng-review (multi-workspace review)

### Release artifact L2 signing (minisign/GPG over SHA256SUMS)
**What:** 当前 release pipeline 用 SHA256SUMS 做 artifact integrity 校验 (L1)。未来升级到 L2:maintainer 用 `minisign` 或 GPG 对 `SHA256SUMS` 文件签名,公钥嵌入 `gitim-updater` binary / 发布在 `CiferaTeam/gitim-releases` repo 根目录,`install.sh` 和 `gitim-updater` 在校验 SHA 前先验 SHA 文件的签名。
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
