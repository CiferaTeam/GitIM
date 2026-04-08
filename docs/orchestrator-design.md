# GitIM Orchestrator — 消息驱动的 Agent 编排层

## 定位

一个独立进程，连接 GitIM 和 LLM Agent。它是 GitIM 和编排器 Agent 之间的**薄驱动层**。

```
GitIM repo (消息 + 状态)
  ↕ gitim CLI
Orchestrator Driver (本 repo)
  ↕ stdin/stdout pipe
Orchestrator Agent (Claude Code session, 长期运行)
  ↕ Agent tool
Sub-Agents (Claude Code sessions, 任务级)
```

**不是什么：**
- 不是 GitIM 本身（那是另一个 repo）
- 不是 LLM 模型（那是 Claude API）
- 不是 Sub-Agent（那是编排器通过 Agent tool spawn 的）
- 不是 OpenClaw（不用定时器驱动，用消息驱动）

**是什么：**
- GitIM channel 的消息监听器
- 编排器 Agent（LLM session）的生命周期管理器
- 消息过滤和投递管道

## 架构

### 三层分离

| 层 | 职责 | 实现 | 决策方式 |
|---|---|---|---|
| **Driver** (本 repo) | 监听消息、过滤、投递、生命周期管理 | 确定性代码 | 硬编码规则 |
| **Orchestrator** (LLM Agent) | 事件判断、任务分发、记忆维护 | Claude Code session | LLM 自主决策 |
| **Sub-Agent** (LLM Agent) | 执行具体任务 | Claude Code Agent tool | LLM 自主决策 |

关键原则：**Driver 层绝不做智能决策。** 它只做三件事：检测、过滤、投递。所有"这条消息需要处理吗？给谁处理？怎么处理？"的判断都由 Orchestrator Agent（LLM）自主完成。

### 核心循环

```
loop:
  1. gitim read <channel> --since <cursor>
  2. 有新消息？
     → 过滤掉 agent 自己写的消息（按 author handler 匹配）
     → 剩余消息打包成结构化文本
     → pipe 给 Orchestrator Agent
  3. 更新 cursor
  4. sleep <poll_interval>
```

## 组件

### 1. Poller

定时轮询 GitIM channel，检测新消息。

```
输入: channel 列表, last_seen_line per channel
输出: 新消息列表 (channel, line, author, body, timestamp)
实现: 循环调用 `gitim read <channel> --since <line>`
```

轮询间隔：默认 5s。可配置。未来可替换为 GitIM 的 subscribe/SSE 机制（如果 GitIM 支持）。

### 2. Filter

确定性过滤，防止反馈循环。

规则：
- 跳过 author 在 `agent_handlers` 列表中的消息
- 跳过 system 消息（join/leave/archive 等）
- 只转发需要编排器关注的消息

```yaml
# config.yaml
agent_handlers:
  - orchestrator-nexus
  - claude-worker-1
  - claude-worker-2
```

### 3. Orchestrator Manager

管理编排器 Claude Code session 的生命周期。

**启动：**
```bash
claude -p \
  --system-prompt "$(cat orchestrator-prompt.md)" \
  --allowedTools "Bash(gitim *),Agent" \
  --model claude-sonnet-4-6
```

**消息投递（增量）：**
```bash
echo "<formatted_message>" | claude -p --resume <session_id>
```

**崩溃恢复：**
- 检测 claude 进程是否存活（PID check）
- 如果进程死亡，用 `--resume` 恢复 session（Claude Code 支持 session 持久化）
- 如果 resume 失败，冷启动新 session，从 memory.md 恢复上下文

**消息格式（投递给编排器的文本）：**
```
[GitIM] #dev-tasks 新消息 (2条)
---
[@lewis][2026-04-08T16:00:00Z] 请帮我调查一下登录页面的性能问题，首屏加载要 5 秒
[@alice][2026-04-08T16:01:00Z] +1，我这边也是，特别是移动端
---
请决定如何处理。
```

### 4. State

本地状态文件（不走 git）：

```
.state/
  cursor.yaml          # 每个 channel 的 last_seen_line
  orchestrator.yaml    # 编排器进程状态 (pid, session_id, started_at)
```

**cursor.yaml:**
```yaml
channels:
  dev-tasks:
    last_seen_line: 42
    last_poll: "2026-04-08T16:05:00Z"
  bug-reports:
    last_seen_line: 17
    last_poll: "2026-04-08T16:05:00Z"
```

**orchestrator.yaml:**
```yaml
status: running        # idle | running | error
pid: 12345
session_id: "abc-123"
started_at: "2026-04-08T12:00:00Z"
restarts: 0
```

### 5. GitIM 侧文件约定

以下文件存在 GitIM repo 中（通过 git 同步）：

```
agents/
  <agent-name>/
    agent.meta.yaml    # agent 身份（handler, watch_channels）
    memory.md          # 编排器维护的持久记忆
```

**agent.meta.yaml：**
```yaml
name: nexus
type: orchestrator
handler: orchestrator-nexus
watch_channels:
  - dev-tasks
  - bug-reports
  - general
created_at: "2026-04-08T00:00:00Z"
```

**memory.md：** 由编排器 Agent 自己维护。格式不硬编码，让 LLM 自己决定怎么组织记忆。编排器的 system prompt 引导它定期更新这个文件。

## Orchestrator System Prompt

这是整个系统最核心的部分。编排器的行为质量完全取决于 system prompt 的设计。

```markdown
# Nexus Orchestrator

你是 Nexus，一个 GitIM 消息驱动的编排器。你 7x24 运行，持续接收 GitIM channel 的新消息。

## 核心原则

1. **你不做具体任务。** 所有实际工作通过 Sub-Agent 完成。你的工作是：判断、分发、追踪、记忆。
2. **你的上下文窗口是稀缺资源。** 用它来维护事件状态和 Sub-Agent 进度，不要用它来理解所有细节。
3. **收到新消息时，先判断再行动。** 不是每条消息都需要反应。有些只需要记录，有些需要立即分发。

## 消息处理流程

收到新消息时：
1. **判断**：这条消息需要行动吗？
   - 是任务请求？→ 分发给 Sub-Agent
   - 是状态更新？→ 更新内部追踪
   - 是闲聊/无关？→ 忽略
2. **分发**：通过 Agent tool spawn Sub-Agent
   - 给 Sub-Agent 明确的任务描述
   - 附带相关的 channel 上下文（用 `gitim read` 获取）
   - 指定 Sub-Agent 完成后用 `gitim send` 回写结果
3. **追踪**：记录 Sub-Agent 的状态
   - 已分发给谁、什么任务、什么时候
   - Sub-Agent 返回结果后更新追踪

## Sub-Agent 分发

使用 Agent tool spawn Sub-Agent 时：
- prompt 里必须包含：任务描述 + 相关上下文 + 回写指令
- 回写指令：`完成后用 gitim send #<channel> "<结果摘要>" 回写结果`
- Sub-Agent 可以用 `gitim read` 获取更多上下文

## 记忆维护

你的上下文窗口会被压缩。为了跨 session 保持连贯性：
- 定期执行 `gitim read` 检查 memory.md 的当前内容
- 用 `gitim send` 或直接写文件更新 agents/nexus/memory.md
- 记录：当前进行中的任务、关键决策、重要事件摘要
- 不记录：每条消息的细节、已完成的琐碎任务

## 工具

- `gitim read #<channel>` — 读取 channel 消息历史
- `gitim send #<channel> "<message>"` — 发送消息到 channel
- Agent tool — spawn Sub-Agent 执行任务
```

## 配置

```yaml
# config.yaml
gitim:
  repo_path: "/path/to/gitim/repo"    # GitIM 仓库路径

orchestrator:
  agent_name: "nexus"                  # 对应 agents/<name>/ 目录
  handler: "orchestrator-nexus"        # GitIM handler
  model: "claude-sonnet-4-6"           # LLM 模型
  prompt_file: "orchestrator-prompt.md"

poller:
  interval_secs: 5                     # 轮询间隔
  channels:                            # 监听的 channel 列表
    - dev-tasks
    - bug-reports
    - general

filter:
  agent_handlers:                      # 过滤掉这些 handler 的消息
    - orchestrator-nexus
    - claude-worker-1
  skip_system_messages: true           # 跳过 join/leave 等系统消息
```

## 项目结构

```
gitim-orchestrator/
  CLAUDE.md                  # 本设计文档
  config.yaml                # 运行配置
  orchestrator-prompt.md     # 编排器的 system prompt（独立文件，方便迭代）
  src/
    main.ts                  # 入口：启动 poller + orchestrator manager
    poller.ts                # GitIM 消息轮询
    filter.ts                # 消息过滤
    orchestrator.ts          # Claude Code session 生命周期管理
    state.ts                 # 本地状态读写
  .state/                    # 运行时状态（gitignore）
    cursor.yaml
    orchestrator.yaml
```

技术栈：TypeScript + Bun。原因：
- GitIM CLI 已有 TypeScript 客户端可参考
- 启动快、依赖少
- Shell 调用 claude CLI 很自然（child_process.spawn）

## 实现优先级

### P0：最小闭环

目标：往 channel 发消息 → 编排器收到 → spawn Sub-Agent → 结果回写

1. **Poller**：循环 `gitim read`，检测新消息
2. **Orchestrator Manager**：启动 Claude Code session，pipe 消息
3. **验证**：手动往 channel 发一条任务，看编排器是否 spawn sub-agent 并回写结果

不做：filter（先手动避免循环）、崩溃恢复、memory 持久化

### P1：稳定运行

4. **Filter**：agent handler 过滤，防止反馈循环
5. **崩溃恢复**：PID check + session resume
6. **State 持久化**：cursor.yaml 防止重启后重复处理

### P2：记忆和长期运行

7. **Memory**：编排器定期更新 memory.md
8. **Session 管理**：上下文窗口耗尽时的优雅重启
9. **多 channel 优先级**：不同 channel 的消息优先级不同

## 与 souls-nexus 的关系

souls-nexus 是 OpenClaw 版的 CEO agent（定时器驱动）。本 repo 是消息驱动的演进版：

| | souls-nexus | gitim-orchestrator |
|---|---|---|
| 驱动方式 | 定时器（OpenClaw heartbeat） | 消息驱动（GitIM channel） |
| 消息总线 | Discord/Slack（中心化 SaaS） | GitIM（Git-native, 去中心化） |
| 状态持久化 | 文件 + OpenClaw snapshot | Git + memory.md |
| Sub-Agent | OpenClaw 多 bot | Claude Code Agent tool |
| 审计 | 平台日志 | git log |

核心升级：从"定时醒来看看有没有事"到"有事了才醒来"。

## 边界

**本 repo 管什么：**
- Poller/Filter/Orchestrator Manager 的代码
- 编排器的 system prompt
- 运行配置

**本 repo 不管什么：**
- GitIM daemon/CLI 的代码（在 GitIM repo）
- Sub-Agent 的具体任务逻辑（由编排器 LLM 决定）
- GitIM 的 sync/conflict resolution（在 GitIM repo）

**本 repo 对 GitIM 的要求：**
- `gitim read <channel>` 能返回消息列表
- `gitim send <channel> <body>` 能发送消息
- 有 channel event 机制（目前通过 polling 模拟；未来如果 GitIM 支持 subscribe/SSE，可以替换 poller）
