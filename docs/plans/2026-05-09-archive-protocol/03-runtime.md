# 03 — runtime: burn endpoint + remove deprecation

> 对应 [01-plan.md](01-plan.md) Part B。runtime 层的核心是新加 burn endpoint + 标 remove deprecated。3 个 task。

## B.1 — POST /agents/burn endpoint

**文件**:[crates/gitim-runtime/src/http.rs](../../../crates/gitim-runtime/src/http.rs)

**新加**:
- 路由:`POST /workspaces/{slug}/agents/burn`(在 [http.rs:3075](../../../crates/gitim-runtime/src/http.rs:3075) 附近,与 agents/remove 同区)
- handler `agents_burn` — 编排 [01-plan.md](01-plan.md) "Agent burn 工作流" 步骤 1-7

**编排顺序**:
1. **type 校验**(P1.c):target id 必须在 `ctx.agents`。不在 → 4xx + `error_code: "not_an_agent"`(防止误把人类 user 走 burn 路径)
2. abort agent loop(取 loop_handle.take + abort)+ kill agent daemon process(`kill_agent_daemon` 现有)
3. 调 daemon `depart_user { handler }` — daemon 走 A.4 的幂等多 commit
   - daemon error → return 5xx,**不**执行步骤 4-7,user 重试(走幂等)
4. `hard_delete_agent_dir`(同现有 hard_delete=true 流程)
5. 删 hermes profile(best-effort,失败仅 warn — 同现有逻辑)
6. 从 `ctx.agents` 移除 agent
7. SSE broadcast `AgentActivityEvent::Burned`(新增 event 类型)+ WebUI 刷新

**对称参考**:[http.rs:1923 agents_remove](../../../crates/gitim-runtime/src/http.rs:1923) — 整体编排骨架抄,差异是步骤 3(remove 不调 depart_user)

**验收**:
- agent.id 不在 ctx.agents → 4xx,无副作用
- happy path:burn → daemon 完成全部 phase + runtime 清 clone + 移除 in-memory + SSE 事件触发
- daemon 半态(commit 部分成功 push 失败) → runtime 收到 error,**不** rm clone dir。user 重试 → daemon 幂等 → runtime 完成 cleanup
- hermes provider:profile 删除失败仅 warn,burn 整体成功
- non-hermes provider:跳过 hermes 步骤

**依赖**:A.4(daemon depart_user)

---

## B.2 — agents/remove 标 deprecated

**文件**:[crates/gitim-runtime/src/http.rs](../../../crates/gitim-runtime/src/http.rs)

**改动**:
- agents_remove handler 函数 + 路由保留,但加 `#[deprecated]` attribute + tracing warn 日志("agents/remove is deprecated, use agents/burn or agents/stop")
- README / docs 里把 agents/remove 的描述加 "(deprecated)" 标注
- WebUI 不再调用(由 E.3 切换)
- **不**在本 PR 里删除 endpoint(避免破坏可能的外部 caller),deprecation 期至少跨 1 个 release,后续 plan 真正清理

**验收**:
- agents/remove 仍然工作(旧逻辑不变),但 server log 有 deprecation warn
- WebUI E.3 改完后不再发起此请求

**依赖**:无(独立改动)

---

## B.3 — runtime 层测试

**文件**:`crates/gitim-runtime/tests/burn_test.rs`(新建)

**测试 case**(端到端,需要启动真实 daemon — 参考 poller 测试模式 + serial_test):
- burn 不存在的 agent id → 4xx
- burn 一个 agent id 但实际是人类 user(虚构场景)→ 4xx(P1.c)
- burn happy path:agent 在 channels/DMs 都有活动 → end-to-end 完成
- burn 中 daemon 故意 push 失败 → runtime 不 cleanup → 重试 → 幂等完成
- burn hermes agent → profile 删除失败仅 warn,整体成功

**测试节奏**:scoped `cargo test -p gitim-runtime --test burn_test`,标 serial(daemon 需独占)

**依赖**:B.1 + A.4

---

## 整体依赖

B.1 和 B.2 可并行(独立)。B.3 依赖 B.1。B.1 必须等 A.4 完成才能跑。
