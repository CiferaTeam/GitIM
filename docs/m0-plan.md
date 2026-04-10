# M0: 单 Agent 闭环 — 分步计划

> 基于 `runtime-architecture.md` M0 目标的细化拆分。

## 目标

证明 Rust runtime 能驱动一个 Claude agent 通过 GitIM 通信。

## 已有基础

- Daemon API 完整：send、read（支持 since 游标）、onboard、channels、subscribe（SSE）
- CLI onboard 已实现：clone → .gitim/ → daemon → 身份注册
- gitim-client crate 存在（Rust 客户端库）
- Sync loop 处理 git push/pull + 冲突重试

## 分步计划

### S1: Runtime crate 骨架 + Agent 目录管理

- 创建 `crates/gitim-runtime/`，定义核心类型（RuntimeState、AgentConfig、AgentHandle）
- 实现 agent provisioning：给定 remote URL + handler，自动 clone → 创建 .gitim/ → onboard → 启动 daemon
- **验证**：集成测试——调用 provisioning 后，agent 目录存在、daemon 可响应 status

### S2: 消息轮询 + 游标追踪

- 连接 agent 的 daemon，定期 poll read 拿新消息
- 用 since 游标（line_number）跟踪已处理位置
- **验证**：测试——send 一条消息后 poll 能检测到；再 poll 不会重复

### S3: Claude -p 集成

- 实现 claude -p 进程 spawn：首次拿 session_id，后续 --resume
- 消息批次格式化 + system prompt 组装（公共层 + agent system.md）
- **验证**：mock 脚本替代 claude，验证 spawn → 输出捕获 → resume 参数正确

### S4: Agent 事件循环

- poll → 队列 → claude spawn → wait → 再检查队列
- idle/busy 状态机：busy 时攒队列，idle 时批处理
- tokio task：poll loop 和 agent loop 解耦
- **验证**：端到端——send → agent 检测 → claude 调用 → side effect 产生回复

### S5: 持久化 + CLI 入口

- Agent 状态持久化（session_id、last cursor、daemon PID）
- CLI 子命令：gitim runtime start / status / stop
- 重启恢复：读持久化状态，跳过已处理消息
- **验证**：启动 → 发消息 → 停止 → 重启 → 不重复处理

## 不做（M0 范围外）

- 前端
- 多 agent
- 动态注册
- 崩溃恢复
- HTTP API
