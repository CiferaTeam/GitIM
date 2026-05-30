# Fleet Tunnel 自愈 实现 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development 或 superpowers:executing-plans 逐 task 实现。步骤用 `- [ ]` 跟踪。
>
> **本 plan 遵循项目惯例(CLAUDE.md):只写分工与契约(文件 / 函数签名 / 行为期望 / 验收点),不写实现体。** 执行者 indwell `00-requirements.md` 与现有 `fleet.rs` / `cli/tunnel.rs` 后编写实现与测试体。函数签名即接口契约,须严格遵守以保证跨 task 一致。

**Goal:** 让 daemon 对每个带可用 `ssh_tunnel` 的 fleet node 自动维持 tunnel keep-alive,重启与运行期断连都自愈。

**Architecture:** `FleetNodeRuntime::new` 在 node 带可用 tunnel 配置时,于 subscription 循环外额外 spawn 一个 `tunnel_watcher_loop` task。watcher 周期性调用既有的幂等 `cli::tunnel::ensure_running`,共享 `FleetNodeRuntime` 的 `AbortHandle` 生命周期。

**Tech Stack:** Rust, tokio(spawn / sleep / AbortHandle),复用 `cli::tunnel::{ensure_running, LaunchConfig}`。

---

## File Structure

| 文件 | 改动 | 职责 |
|------|------|------|
| `crates/gitim-runtime/src/fleet.rs` | 修改(核心) | 常量 + 2 个纯函数 + watcher loop + `FleetNodeRuntime::new` 接线 |
| `crates/gitim-runtime/src/bin/runtime.rs` | 修改(文案) | `fleet tunnel down` 的 doc comment 更新为"强制重启"语义 |

`cli/tunnel.rs` 零改动(`ensure_running` / `LaunchConfig` 已 `pub`)。

---

## Task 1: backoff 递增纯函数 + 常量

**Files:**
- Modify: `crates/gitim-runtime/src/fleet.rs`(顶部常量区 + 新纯函数 + 内联 `#[cfg(test)]`)

**契约:**
- 新增常量:`TUNNEL_POLL_INTERVAL = Duration::from_secs(10)`、`TUNNEL_BACKOFF_INITIAL = Duration::from_secs(5)`、`TUNNEL_BACKOFF_MAX = Duration::from_secs(120)`
- 新增 `fn next_backoff(current: Duration) -> Duration` —— 返回 `(current * 2)` 但封顶在 `TUNNEL_BACKOFF_MAX`

**行为 / 验收测试点:**
- `next_backoff(5s) == 10s`
- `next_backoff(10s) == 20s`
- `next_backoff(80s) == 120s`(翻倍 160 被封顶)
- `next_backoff(120s) == 120s`(已封顶,稳定)

- [ ] Step 1: 写失败测试 `next_backoff_doubles_and_caps`(断言上述 4 点)
- [ ] Step 2: `cargo test -p gitim-runtime --lib next_backoff` → 预期 FAIL(函数未定义)
- [ ] Step 3: 实现常量 + `next_backoff`
- [ ] Step 4: `cargo test -p gitim-runtime --lib next_backoff` → 预期 PASS
- [ ] Step 5: Commit `feat(fleet): add tunnel backoff helper + intervals`

---

## Task 2: tunnel_launch_config 决策纯函数

**Files:**
- Modify: `crates/gitim-runtime/src/fleet.rs`(新纯函数 + 内联测试)

**契约:**
- 新增 `fn tunnel_launch_config(entry: &FleetNodeEntry) -> Option<cli::tunnel::LaunchConfig>`
- 逻辑:`ssh_tunnel` 缺省 → `None`;`ssh_tunnel.local_port` 缺省 → `None`;两者俱全 → `Some(LaunchConfig)`,字段映射:
  - `node_id ← entry.node_id`
  - `ssh_target ← tunnel.ssh_target`
  - `remote_host ← tunnel.remote_host`
  - `remote_port ← tunnel.remote_port`
  - `local_port ← tunnel.local_port`(已解包)
- 实现提示:`let tunnel = entry.ssh_tunnel.as_ref()?; let local_port = tunnel.local_port?; Some(...)`

**行为 / 验收测试点:**
- node 带 `ssh_tunnel` + `local_port = Some(18068)` → `Some`,且 5 个字段逐一映射正确
- node 无 `ssh_tunnel` → `None`
- node 有 `ssh_tunnel` 但 `local_port = None` → `None`(无固定端口不可维持)

- [ ] Step 1: 写失败测试 `tunnel_launch_config_requires_port`(覆盖上述 3 case)
- [ ] Step 2: `cargo test -p gitim-runtime --lib tunnel_launch_config` → 预期 FAIL
- [ ] Step 3: 实现 `tunnel_launch_config`
- [ ] Step 4: `cargo test -p gitim-runtime --lib tunnel_launch_config` → 预期 PASS
- [ ] Step 5: Commit `feat(fleet): add tunnel_launch_config decision fn`

---

## Task 3: watcher loop + FleetNodeRuntime 接线

**Files:**
- Modify: `crates/gitim-runtime/src/fleet.rs`(新 async fn + `FleetNodeRuntime::new` 末尾接线)

**契约:**
- 新增 `async fn tunnel_watcher_loop(launch: cli::tunnel::LaunchConfig)`,行为:
  ```
  backoff = TUNNEL_BACKOFF_INITIAL
  loop {
      match cli::tunnel::ensure_running(&launch).await {
          Ok(_)  => { backoff = TUNNEL_BACKOFF_INITIAL; sleep(TUNNEL_POLL_INTERVAL).await }
          Err(e) => { tracing::warn!(node_id=%launch.node_id, error=%e,
                       "fleet tunnel watcher: ensure_running failed, retrying");
                      sleep(backoff).await; backoff = next_backoff(backoff) }
      }
  }
  ```
- `FleetNodeRuntime::new`:在现有 `for subscription in workspace_subscriptions(&entry)` 循环**之后**(循环外,每 node 仅一次):
  ```
  if let Some(launch) = tunnel_launch_config(&entry) {
      let handle = tokio::spawn(tunnel_watcher_loop(launch));
      handles.push(handle.abort_handle());
  }
  ```

**关键约束(review 时核对):**
- watcher spawn 必须在 subscription 循环**外** —— 每 node 至多一个 watcher,与映射的 workspace 数无关。
- abort_handle 必须 push 进 `handles`,使 `Drop` 能随 node 移除一并 abort。

**验收:**
- `cargo build -p gitim-runtime` 通过(`pre-commit` 的 clippy 同时把关)
- spawn 决策由 Task 2 的 `tunnel_launch_config` 单测覆盖
- watcher loop 本身是无限循环 + 真实 ssh IO,不做单元测试;运行期自愈由 Task 5 手动集成验证(理由:CLAUDE.md「测试要抓真 bug,不为覆盖率灌水」)

- [ ] Step 1: 实现 `tunnel_watcher_loop`
- [ ] Step 2: `FleetNodeRuntime::new` 循环外接线 spawn + push handle
- [ ] Step 3: `cargo build -p gitim-runtime` → 预期通过
- [ ] Step 4: `cargo test -p gitim-runtime --lib fleet` → 预期既有 + 新增测试全 PASS
- [ ] Step 5: Commit `feat(fleet): spawn tunnel watcher per node for self-heal`

---

## Task 4: `fleet tunnel down` help 文案

**Files:**
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`(`FleetTunnelCommand::Down` 的 `///` doc comment,约 304 行)

**契约:**
- 现文案类似 `Stop the local SSH tunnel for a node.`
- 改为说明 watcher 存在下的真实语义,例如:`Force-restart the local SSH tunnel (the node's watcher will re-establish it within ~10s). To stop permanently, use 'fleet remove <node>'.`

**验收:**
- `cargo build -p gitim-runtime` 通过
- `gitim-runtime fleet tunnel down --help` 输出含新文案

- [ ] Step 1: 更新 doc comment
- [ ] Step 2: `cargo build -p gitim-runtime` → 预期通过
- [ ] Step 3: `~/.gitim/bin/...` 或 target binary `fleet tunnel down --help` 人工核对文案
- [ ] Step 4: Commit `docs(fleet): clarify 'tunnel down' is now force-restart`

---

## Task 5: 真实环境自愈手动验证

**前置:** worktree 编出的 runtime 已替换运行 / 或在隔离 workspace 跑;mac-mini 在线、ssh config 已绑 key(L1 已修)。

**验收剧本:**
- [ ] Step 1: 确认 fleet status 为 `connected`、tunnel pid 在跑(`fleet tunnel status --node-id mac-mini`)
- [ ] Step 2: 手动 `kill <tunnel-pid>` 模拟运行期断连
- [ ] Step 3: 观察:observer 短暂刷 `SSE request failed`;**≤10s 内** watcher 重建 tunnel(新 pid),`fleet status` 回到 `connected`
- [ ] Step 4: 记录恢复耗时,确认无需任何手动 `fleet tunnel up`

> 不写自动化集成测试 —— 需真实 ssh + 远端 runtime,环境敏感(参见 poller 测试既有痛点)。手动剧本一次性确认行为闭环。

---

## Self-Review(plan 作者已核对)

- **Spec coverage:** L2 根治(watcher)→ Task 1-3;语义变化 help → Task 4;自愈闭环验证 → Task 5;边界(local_port None / per-node 单 watcher)→ Task 2 + Task 3 约束。L1/L3 spec 标注无需改动 ✓。
- **Placeholder scan:** 无 TBD/TODO;每 task 有具体签名、断言点、命令 ✓。
- **Type consistency:** `next_backoff(Duration)->Duration`、`tunnel_launch_config(&FleetNodeEntry)->Option<LaunchConfig>`、`tunnel_watcher_loop(LaunchConfig)` 跨 task 引用一致;`LaunchConfig` 字段与 `cli/tunnel.rs` 既有定义一致 ✓。
