# GitIM Runtime — Agent 编排运行时

> 本文档是 `orchestrator-design.md` 的演进版。基于已有的 GitIM daemon + CLI 运行时，定义一个消息驱动的 Agent 编排层。

## 定位

Runtime 是一个独立的 Rust 进程，跑在 GitIM daemon 之上。它不是 daemon 的一部分，也不替代 daemon。

```
Human (前端 UI)
  ↕
Runtime (crates/gitim-runtime)
  ├── Agent A: clone + daemon + claude -p
  ├── Agent B: clone + daemon + claude -p
  ├── Agent C: clone + daemon + claude -p
  └── Human:   clone + daemon + 前端 UI
  ↕
Git Remote (bare repo 或远程 URL)
```

**Runtime 做什么：**
- 管理多个 GitIM client 目录（clone、daemon、agent 进程）
- 轮询每个 agent 的 daemon 获取新消息
- 把消息作为 prompt 参数传给 `claude -p --resume`（每批消息一次 spawn + wait）
- 暴露 HTTP API 给前端
- 持久化运行状态，支持重启恢复

**Runtime 不做什么：**
- 不做消息路由（每个 daemon 天然只看到自己该看的消息）
- 不做智能决策（判断、分发、记忆全由 LLM 在 prompt 层完成）
- 不替代 daemon 的消息协议、Git 同步、索引功能

## 核心架构决策

### 1. 进程模型

Runtime 是单个 Rust 进程。每个 agent 是 Runtime 内部的一个 tokio task。

`claude -p` 不是长驻进程——每次 `--resume` 调用都是 spawn 一个新进程，执行完毕后退出。Runtime 的 tokio task 负责管理这个 spawn-wait 循环。

```
Runtime 进程 (1个)
  ├── tokio task: Agent A → 按需 spawn claude -p（每批消息一次）
  ├── tokio task: Agent B → 按需 spawn claude -p
  ├── tokio task: Agent C → 按需 spawn claude -p
  ├── tokio task: poll loop（轮询所有 daemon）
  └── tokio task: HTTP server（前端 API）
```

Agent task 之间完全并行，互不阻塞。`claude -p` 进程异常退出只影响对应的 agent task，Runtime 进程不受影响。

### 2. Git 模型

所有参与者（agent + 人类）共享一个 Git remote，各自持有独立的 clone。

```
Git Remote (source of truth)
  ├── Agent A clone (~/gitim-agents/agent-a/)
  ├── Agent B clone (~/gitim-agents/agent-b/)
  └── Human clone   (~/gitim-agents/human/)
```

- Remote 可以是本地 bare repo（本地开发）或远程 URL（分布式部署）
- 每个 clone 有自己的 `.gitim/`、`me.json`、daemon 实例
- 通过 push/pull 同步，不能共享工作目录
- Runtime 初始化时设置 remote 地址，后续每注册一个 agent 就 clone 一份

### 3. 每个 Agent 独立 Daemon

第一版中，每个 agent 目录各跑一个独立的 daemon 进程。Runtime 通过 `gitim` CLI 命令与每个 daemon 通信（跟狼人杀 demo 模式一致）。

未来优化：Runtime 可以 embed daemon 核心库（`gitim-core`、`gitim-sync`），不再 spawn daemon 进程。但这是性能优化，不是 M0 的事。

### 4. Claude 通信

每个 agent 维护一个 Claude Code session。每次有新消息时，Runtime spawn 一个 `claude -p` 进程，执行完毕后进程退出。通过 `--resume` 在同一个 session 上累积上下文。

```bash
# 首次启动（创建 session）
claude -p \
  --system-prompt "<runtime公共prompt> + <agent的system.md>" \
  --allowedTools "Bash(gitim *),Read,Write,Agent" \
  --model claude-sonnet-4-6 \
  "<首条消息>"
# → 进程执行完毕后退出，返回 session_id

# 后续消息（resume 已有 session）
claude -p --resume <session_id> "<新消息批次>"
# → spawn 新进程，加载 session 上下文，执行，退出
```

每次调用都是一个完整的 spawn → 执行 → 退出 周期，不是长连接。

关键约束：同一个 session_id 不能并发 `--resume`（会导致 session 文件消息交错）。因此每个 agent 的调用必须串行——上一次返回后才能发起下一次。

### 5. 消息调度

每个 agent 有一个消息队列。Runtime 的 poll loop 和 agent 的处理 loop 解耦：

```
Poll loop:  poll daemon → 新消息入队 → poll daemon → 新消息入队 → ...
Agent loop: 等消息 → 取出全部 → claude --resume → 等消息 → ...
```

流程：
1. Poll loop 每隔 N 秒 poll 每个 agent 的 daemon
2. 有新消息 → 入对应 agent 的队列
3. Agent 当前 idle → 取出队列所有消息，打包调 `claude -p --resume`，状态变 busy
4. Agent 当前 busy → 消息留在队列里，不做任何事
5. `claude -p` 返回 → 状态变 idle → 检查队列 → 有积压就立即再调一轮

天然的批处理效果：agent 忙时来的消息会被攒成一批，减少 LLM 调用次数。

```rust
// 伪代码
loop {
    let messages = queue.recv_batch().await;
    // spawn claude -p 进程，等待执行完毕
    let output = Command::new("claude")
        .args(["-p", "--resume", &session_id, &format_messages(&messages)])
        .current_dir(&agent_repo_root)
        .output().await?;
    // output.stdout 是 claude 的回复，side effect 已在执行中完成（gitim send 等）
}
```

### 6. Agent 间通信

Agent 之间不直接通信，全走 GitIM channel。

```
Agent A: gitim send #dev-tasks "分析完了，结论是..."
  → Git push
  → Git remote
  → Agent B 的 daemon git pull
  → Agent B 的 poll 拿到消息
  → Runtime 喂给 Agent B 的 claude -p
```

跟人类用户之间的通信走完全一样的路径。没有特殊通道。

### 7. 人类用户

人类用户跟 agent 完全对称：有自己的 clone 目录 + daemon + handler 身份。唯一区别是消息驱动源是前端 UI，不是 `claude -p`。

管理界面里，人类和 agent 并列显示，类型不同。

### 8. Agent 行为定义

一个 agent 的行为由一个 `system.md` 文件定义。

最终传给 `claude -p` 的 system prompt = **Runtime 公共层（硬编码在 Rust 里）** + **agent 的 system.md**。

公共层包含协议级行为：
- 你是一个 GitIM agent
- 用 `gitim send` 回写结果
- 用 `gitim read` 获取上下文
- 收到消息先判断再行动

Agent 的 `system.md` 定义角色特有行为（开发者、审查者、编排者等）。

### 9. 配置模式

零配置启动。`gitim runtime start` 不需要预先编写配置文件。

- 首次启动：创建默认工作目录
- 通过 CLI / UI 添加 agent、加入 channel 等操作
- 变更自动持久化到本地状态文件
- 重启后从持久化状态恢复

### 10. 前端

前端只跟 Runtime 通信（单一后端），两层界面：

| 层 | 功能 | 是否需要 GitIM 身份 |
|---|---|---|
| 管理层 | 查看 agent 状态、添加/移除 agent、部署监控 | 不需要 |
| 聊天层 | 发消息、看 channel、参与对话 | 需要（人类注册后） |

每台机器一个 Runtime + 一个前端，管理本机的 agent。

### 11. CLI 接口

Runtime 作为 `gitim` CLI 的子命令：

```bash
gitim runtime start           # 启动 runtime
gitim runtime status          # 查看运行状态
gitim runtime add-agent       # 注册新 agent
gitim runtime remove-agent    # 移除 agent
gitim runtime stop            # 停止 runtime
```

## 项目结构

```
crates/
  gitim-core/          # 类型、解析、验证
  gitim-daemon/        # API handler、server
  gitim-sync/          # Git 同步
  gitim-index/         # 搜索索引
  gitim-runtime/       # ← 新增：Agent 编排运行时
cli/                   # TypeScript CLI（增加 runtime 子命令）
webui/                 # React 前端（扩展管理层）
```

## 落地节奏

### M0：单 Agent 闭环

目标：证明 Rust runtime 能驱动一个 Claude agent 通过 GitIM 通信。

交付物：
- `crates/gitim-runtime/` 基本骨架
- 启动一个 agent（clone + daemon + claude -p）
- Poll loop → 队列 → resume 闭环
- 持久化每个 agent 的 last_commit cursor（重启后不重复处理消息）
- 人类用 `gitim send`（CLI）发消息，agent 收到并回写

不做：前端、多 agent、动态注册、崩溃恢复。

### M1：多 Agent + 人类对称

目标：证明多个 agent 能通过 GitIM channel 协作。

交付物：
- Runtime 管理 N 个 agent
- 人类也是一个对称参与者（CLI 交互）
- Agent 之间通过 channel 互相通信

### M2：前端管理层

目标：可视化管理本地 agent。

交付物：
- Runtime 暴露 HTTP API
- 前端显示 agent 状态（运行/空闲/出错）
- 前端支持添加/移除 agent

### M3：前端聊天层

目标：人类通过浏览器参与对话。

交付物：
- 人类的 clone + daemon 由 runtime 管理
- 复用现有 webui 聊天组件
- 统一管理 + 聊天体验

### M4：稳定性

目标：可靠长期运行。

交付物：
- `claude -p` 崩溃自动恢复（重启 + resume）
- Daemon 崩溃恢复
- 状态持久化，runtime 重启后恢复所有 agent

## 与 orchestrator-design.md 的关系

本文档取代 `orchestrator-design.md` 的架构设计部分。关键演进：

| | orchestrator-design.md | 本文档 |
|---|---|---|
| 语言 | TypeScript + Bun | Rust |
| 编排模型 | 单一 Orchestrator Agent 做智能路由 | 无中心编排，每个 agent 独立 poll |
| 消息路由 | Driver 层做过滤+投递给 Orchestrator | 不需要路由，daemon 天然过滤 |
| Agent 管理 | Orchestrator 通过 Agent tool spawn | Runtime 直接管理 claude -p 子进程 |
| 配置 | 预先声明 config.yaml | 零配置启动，状态积累 |
| 人类参与 | 不在架构内 | 跟 agent 对称 |

## 已知限制和风险

以下是架构审查中识别的风险点。短期（3-4 个 agent）不构成阻塞，记录在此供后续迭代参考。

### 产品层

- **定位声明缺失**：GitIM Runtime 不是"更快的 agent 编排"（CrewAI/AutoGen 走内存调用，毫秒级），而是"可审计、可离线、人机对称的异步 agent 协作"。Git 作为通信层带来审计追踪和松耦合，代价是消息延迟（一个来回 5-10s）。这个 tradeoff 需要在对外文档中明确。
- **成本模型**：每个 agent 是持久 Claude session，context 随消息累积增长。需要 session 轮换策略（context window 耗尽时优雅重启），以及可选的预算控制机制。计划在 M4 解决。
- **LLM 绑定**：M0-M4 只支持 Claude（`claude -p`）。LLM provider 抽象层是后续版本的事。

### 工程层

- **Git sync 在多 agent 下的压力**：多个 daemon 同时 push 到同一个 remote 会产生冲突重试。现有 sync_loop 有 3 次重试但无随机 backoff，高并发时可能重试风暴。20 个 agent 对 GitHub remote 的请求量（~28800/hr）会超 rate limit（5000/hr）。短期用本地 bare repo 或 3-4 个 agent 不会触及。
- **Orphan 进程清理**：Runtime 被 kill -9 后，daemon 和 claude -p 子进程变成孤儿进程。需要在持久化状态中记录 managed PID，启动时检测并清理。计划在 M4 解决。
- **Session 文件脆弱性**：Claude Code 的 session 文件存储在 `~/.claude/` 下，不受 Runtime 控制。磁盘满、CLI 升级、手动清理都可能导致 `--resume` 失败。恢复策略：检测失败后回退到新 session（丢失对话历史，保留角色定义）。
- **Subscribe vs Poll**：现有 daemon 已支持 subscribe 模式（实时推送事件），比 poll 延迟更低。M0 先用 poll（实现简单），后续可切换到 subscribe 提升实时性。
- **Agent 间错误循环**：两个 agent 可能形成乒乓循环（A 报错 → B 尝试修复 → 又触发 A → 无限循环）。daemon 层只过滤"该不该看到"，不过滤"该不该响应"。需要某种限流或 circuit breaker 机制，通过 system prompt 约束或 Runtime 层检测。
- **可观测性**：多 agent 运行时的日志归集、状态监控、问题排查机制待设计。前端管理层（M2）是第一步，但不够。
