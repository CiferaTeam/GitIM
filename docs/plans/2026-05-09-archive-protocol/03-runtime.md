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

## B.4 — agent_loop self-departed 自愈

**背景**:agent 调 `gitim burn-self`(C.3)→ daemon 完成 depart_user → daemon process 仍在跑,但 alice 自己的 user entry 已经 archived。runtime 的 agent_loop 在 polling alice 的 daemon,目前不会自动发现"alice 已 departed"。这个 task 给 runtime 加自愈检测,让 self-burn 路径不需要 user 再去 WebUI 手动触发 cleanup。

**文件**:
- [crates/gitim-daemon/src/handlers/poll.rs](../../../crates/gitim-daemon/src/handlers/poll.rs)
- [crates/gitim-runtime/src/agent_loop.rs](../../../crates/gitim-runtime/src/agent_loop.rs)

**改动**:

daemon 侧(每个 agent 的 daemon 都有 self-handler 上下文):
- handle_poll 入口加 self-departed 检测:`stat archive/users/<self_handler>.meta.yaml`
- 存在 → return error response with `error_code: "self_departed"` + message "agent self-departed via burn-self"
- 检测在 handler 入口最早处,避免无谓的后续 IO

runtime 侧:
- agent_loop poll 错误处理路径识别 `error_code == "self_departed"`
- 命中时:**不**走常规 backoff retry,改为触发 self-cleanup
  - 复用 [crates/gitim-runtime/src/http.rs](../../../crates/gitim-runtime/src/http.rs) `agents_burn` 步骤 4-7(rm clone + 删 hermes profile + ctx.agents 移除 + SSE)
  - **不**调 daemon depart_user(已经被 self-burn 流程做完,幂等 return 也无意义)
  - 抽出共享 `cleanup_agent_runtime_side(state, slug, agent_id)` helper,被 burn endpoint 和 self-departed 路径复用

**关键边界**:
- WebUI burn 路径(B.1) 和 self-burn 自愈路径,**runtime 最终状态一致**(ctx.agents 移除 + clone dir 清理 + SSE 通知)
- 两路 idempotent:user 在 self-burn 完成前抢先点 WebUI burn → 走 B.1 路径,daemon 走 depart_user 幂等(已完成 → return success)→ runtime cleanup。后续 self-burn 自愈检测发现已 cleanup,no-op
- agent_loop 触发 self-cleanup 后,自身 task 自然结束(loop_handle.abort 在 cleanup 时调,等同于现有 stop / remove 路径)

**验收**:
- self-burn e2e:agent CLI burn-self → daemon 完成 depart_user → 下次 runtime poll(几秒内)→ daemon return self_departed → runtime 自动 cleanup → WebUI SSE 收到 burned 事件 + agent 列表刷新
- WebUI burn 与 self-burn 自愈两路 cleanup 结果一致(ctx.agents 都不见,clone dir 都清,hermes profile 都删)
- 两路并发触发(罕见 race):user 看到 burn 成功 + self-burn 自愈走 no-op,无 panic / leak

**依赖**:A.4(daemon depart_user 落 archive/users/<handler>.meta.yaml,这是 self-departed 信号源)+ B.1(共享 cleanup helper)

---

## 整体依赖

```
B.2 (deprecate remove)         独立,可任意时机
B.1 (burn endpoint)        ───┬─→ B.3 (runtime 测试)
A.4 (daemon depart_user)   ───┤
                              └─→ B.4 (self-departed 自愈) ← 也依赖 B.1 抽 helper
```

B.2 完全独立。B.1 / B.4 / B.3 都依赖 A.4。B.4 还依赖 B.1 抽出的 cleanup helper。
