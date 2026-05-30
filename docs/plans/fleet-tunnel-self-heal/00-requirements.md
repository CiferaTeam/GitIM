# Fleet Tunnel 自愈(self-heal)— 需求与设计

## 背景

Fleet observer 模式靠一条 SSH tunnel 把本机 `127.0.0.1:<local_port>` 转发到远端 runtime(loopback-bound)。配置持久化在 `~/.gitim/runtime.json` 的 `fleet_nodes[]`,daemon 重启时 `fleet::recover_from_config` 会恢复 **SSE observer**(订阅 `127.0.0.1:<local_port>` 的 agent events)。

但故障诊断暴露了一条三层断链:电脑重启后某个 fleet node 持续显示"未连接"。

| 层 | 问题 | 处置 |
|----|------|------|
| **L1 SSH 认证** | 重启后 ssh-agent 清空、ssh config 未绑定专用 key → ssh 回退默认 key 被拒 | 已在 `~/.ssh/config` 给目标 host 绑定 `IdentityFile` + `IdentitiesOnly`(key 无 passphrase,无需 agent)。不在本 feature 代码范围。 |
| **L2 Tunnel 进程** | tunnel 是独立 OS 进程,被重启杀掉;`recover_from_config` 只恢复 observer,不拉 tunnel | **本 feature 根治** |
| **L3 SSE observer** | 自动恢复了,但空连一个没有 tunnel 在监听的本地端口,每 2s 刷 `SSE request failed` | tunnel 一回来即自愈,无需改动 |

根因:**tunnel 拉起逻辑只挂在 CLI(`fleet tunnel up` / `--node` 按需)上,daemon 的启动恢复路径完全不碰 tunnel**。observer 与 tunnel 的生命周期脱节。

## 目标

让 daemon 对每个带 `ssh_tunnel` 配置的 fleet node 自动维持 tunnel 存活 —— 不仅启动时拉起,运行期意外断开(网络抖动、ssh keepalive 超时、远端临时下线)也自动重连。即 keep-alive 语义,而非"启动拉一次"。

## 方案:FleetNodeRuntime 内联 watcher

`FleetNodeRuntime::new` 在 node 带可用 tunnel 配置时,额外 spawn 一个 `tunnel_watcher_loop` task,与现有的 per-workspace observer task 并存,共享同一套 `AbortHandle` 生命周期(node 移除/重新 activate → `Drop` abort 所有 handle,watcher 随之停止)。

复用现有 `cli::tunnel::ensure_running` 的幂等性 —— 它本身就是"确保 tunnel 健康":活着且 health ok 则零动作,否则清理 stale state 并重建。watcher 只需周期性调用它。

### 改动范围(1 文件)

```
crates/gitim-runtime/src/fleet.rs
  ├─ 新增常量 TUNNEL_POLL_INTERVAL / TUNNEL_BACKOFF_INITIAL / TUNNEL_BACKOFF_MAX
  ├─ 新增 async fn tunnel_watcher_loop(launch: LaunchConfig)
  └─ FleetNodeRuntime::new — subscription 循环外 spawn watcher(每 node 至多一个)
```

`cli::tunnel::{ensure_running, LaunchConfig}` 已是 `pub`,fleet.rs 直接复用 —— watcher 只调 `ensure_running`(它内部已自行处理 pid 存活 / health / 重建),无需把 `process_alive` 改 `pub`,cli 层零改动。`recover_from_config` / `activate_node` / observer 逻辑均不改 —— 新行为在 `FleetNodeRuntime::new` 内自然触发。

### Watcher 逻辑

```
backoff = TUNNEL_BACKOFF_INITIAL
loop {
    match ensure_running(&launch).await {
        Ok(_)  => { backoff = INITIAL; sleep(TUNNEL_POLL_INTERVAL) }   # 健康,轻量轮询
        Err(e) => { warn!(...); sleep(backoff); backoff = (backoff*2).min(MAX) }  # 退避重试
    }
}
```

- **健康轮询** `TUNNEL_POLL_INTERVAL = 10s`:`ensure_running` 走一次 health probe(2s timeout),开销近零。
- **退避** `5s → 120s` 封顶:刚断时积极重连,持续失败则拉长间隔。
- **不加 auth 熔断**(区别于 sync_loop):ssh 重连只是本地进程尝试,不消耗任何远端 rate limit;无限退避重试正是 keep-alive 想要的 —— 远端关机数小时后重新上线,watcher 自己接回。

### 数据流:断连恢复闭环

```
远端网络抖动 → ssh ServerAliveCountMax 超时 → tunnel 进程退出(pid 死)
  ├─ observer loop:  连本地端口失败,每 2s 刷 WARN(既有行为)
  └─ watcher loop:   ≤10s 内 poll 发现 pid 死 → ensure_running 重建 tunnel
                     → 本地端口回来 → observer 下次重试(≤2s)connected ✓
```

watcher 正好补上了原本需要人工 `fleet tunnel up` 扮演的角色。

## 边界情况

- **每 node 单 watcher**:watcher 在 subscription 循环**外** spawn。一个 node 映射多 workspace 也只有一条 tunnel、一个 watcher(observer 才是 per-workspace 多个)。
- **detached ssh 不泄漏**:watcher task 被 abort 时,已 spawn 的 ssh 进程是 detached 的,不随 task 死。下个 watcher 通过 state 文件 + pid 检查复用它,最坏多一轮重启,不泄漏、不重复。
- **daemon 重启(电脑没关)**:旧 tunnel 进程仍活 → 新 watcher `ensure_running` 发现 pid 活 + health ok → 复用,tunnel 不中断。
- **本地端口被占**:`start_tunnel` 的 `ensure_port_available` 失败 → 退避重试,日志可见。
- **`local_port` 缺省**:`ssh_tunnel` 存在但 `local_port` 为 `None` 时,无固定端口可维持 → 不 spawn watcher,仅 observer(与当前行为一致)。

## 语义变化:`fleet tunnel down`

watcher 存在后,`fleet tunnel down`(`stop_pid` + `remove_state`)的效果从"停掉 tunnel"变为"**重启 tunnel**" —— down 完 watcher 会在 ≤10s 内重新拉起。要真正停掉一条 tunnel,需 `fleet remove <node>`(移除 node → `Drop` FleetNodeRuntime → abort watcher)。

这是有意的:在自愈系统里,"node 注册着 = 连接就该活着"。`tunnel down` 的 CLI help 文案需相应更新,说明它现在是"强制重启 tunnel"语义。

## Non-goals (v1)

- **WebUI tunnel 层状态可见性**:watcher 失败只 `tracing::warn`,与现有 observer 一致靠日志诊断。让 WebUI 区分"tunnel 重连中 / ssh 认证失败 / 远端 runtime 死" 需扩 `fleet_status` 模型 + SSE event + 前端,留 v2。
- **L1 SSH 认证的程序化管理**:专用 key 绑定靠 `~/.ssh/config`(用户级配置),gitim 不接管 ssh key 生命周期。
- **跨平台 keep-alive 守护**(launchd/systemd):由 daemon 内 watcher 统一处理,不引入 OS 级依赖。

## 测试要点

- watcher spawn 条件:`ssh_tunnel.is_some() && local_port.is_some()` 才起;缺 `local_port` 不起。
- 每 node 单 watcher:node 映射 N 个 workspace 时 watcher 数恒为 1(不随 subscription 增长)。
- backoff 状态机:连续失败 5→10→20…→120 封顶;一次成功后重置回 5。
- `ensure_running` 幂等:pid 活 + health ok 时不重启(复用)。
- `Drop` 行为:node 移除后 watcher task 被 abort(不再产生新的 ensure_running 调用)。
