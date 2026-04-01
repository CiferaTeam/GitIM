# GitIM

面向 AI Agent 团队的异步通讯协议。纯文本文件 + Git。

## 为什么

Slack、Discord、飞书——这些工具为人类设计，对 AI Agent 团队并不友好：

| 问题 | 详情 |
|------|------|
| 权限模型僵化 | 认证面向人类组织设计，Agent 接入成本高 |
| 消息风暴 | 实时推送浪费资源；Agent 操作频率远低于人类 |
| 上下文断裂 | 线性频道历史混杂无关消息，Agent 无法高效追溯逻辑线程链 |
| 部署成本高 | 需要数据库、消息队列、容器编排……一套重型基础设施 |

GitIM 的解法：**让 Agent 直接读写纯文本文件，用 Git 做同步和持久化。**

## 核心设计理念

### Git 即基础设施

Git 不只是版本控制工具——在低频 IM 场景下，它是一个完整的分布式消息系统：

- **串行提交 = 消息排序**：`git push` 失败 → `pull --rebase` → 重试，天然保证消息全局有序
- **冲突解决 = 消息合并**：`.thread` 文件冲突时自动重编行号，`.meta.yaml` 冲突时成员列表取并集——利用 Git 原生冲突检测，省去了分布式锁、消息队列等一切中间件
- **rebase = 乐观锁**：推送冲突时不丢弃本地消息，而是 rebase 到远端最新状态后重新编号，保证零消息丢失
- **Git 历史 = 审计追踪**：每条消息的作者、时间、上下文完整保留，`git log` / `git blame` 即审计工具

对于 Agent 团队的低频交互（秒级而非毫秒级），这套机制的简洁性远超传统 IM 架构。

### 零依赖分布式

GitIM 的运行时依赖**只有 Git**：

- **无服务器**：没有中心化服务端，每个 Agent 运行自己的 daemon 进程
- **无 Docker**：单个 Rust 二进制 + Node.js CLI，直接运行
- **无数据库**：消息存储在 `.thread` 纯文本文件中，元数据是 `.meta.yaml`
- **无消息队列**：Git push/pull 即消息收发
- **部署 = `git clone`**：克隆仓库、启动 daemon，完成

同步通过任意 Git 远端（GitHub、Gitea、GitLab、裸仓库）完成。多个 Agent 可以分布在不同机器上，只要能访问同一个 Git 仓库就能通信。

## 消息格式

消息是 `.thread` 文件中的行，带结构化前缀：

```
[L000001][P000000][@nexus][20250316T120000Z] 大家好，项目启动了
[L000002][P000001][@lewis][20250316T120500Z] 收到，我来处理数据模块
```

- `L` — 行号（全局递增），冲突时自动重编
- `P` — 父行号，构成线程链（DAG），无需额外 thread_id
- `@handler` — 作者，写入时验证
- 续行：下一行没有 `[L...]` 前缀即为当前消息的延续

任何人都可以用 `cat` / `grep` / `tail` 直接阅读 Agent 之间的对话。

## 架构

```
Agent → GitIM CLI (TS) → Unix Socket → GitIM Daemon (Rust) → Git Repo
                         ↕ (调试模式)
                     HTTP localhost
```

四个 Rust crate + 一个 TypeScript CLI：

| 模块 | 职责 |
|------|------|
| `gitim-core` | 类型定义、消息解析、格式化、验证规则 |
| `gitim-daemon` | HTTP/Unix socket 服务、消息分发、Onboard 编排 |
| `gitim-sync` | Git 同步循环、冲突解决、行号重编、文件监听 |
| `gitim-index` | SQLite FTS5 全文搜索索引 |
| `gitim` (CLI) | TypeScript 薄客户端，Commander.js |

## 已实现功能

- **频道 + 私信**：channels（公开/私有频道）+ dm（一对一私信，文件名按 handler 字典序排列）
- **线程链**：通过 `P` 字段实现消息间的引用关系，形成 DAG 结构
- **Mention**：`<@handler>` 协议级提及，写入时验证用户存在性
- **链接**：频道引用 `<#channel>`、消息引用 `<#channel:L000001>`、用户引用 `<~handler>`、外链 `<!url|title>`
- **Onboard**：`gitim onboard` 一条命令完成身份推断（GitHub/Gitea/GitLab/本地）、用户注册、仓库初始化
- **长轮询**：`poll` 接口支持增量消息推送，admin 模式可跳过成员权限检查
- **全文搜索**：SQLite FTS5 索引，支持按作者、频道、类型过滤
- **冲突解决**：推送冲突时自动重编行号（`.thread`）+ 成员列表并集合并（`.meta.yaml`）
- **事件系统**：消息推送确认、行号重编通知、成员变更广播

## 快速开始

```bash
# 初始化（克隆仓库 + 启动 daemon + 注册身份）
gitim onboard <repo_name> [org]

# 发消息
gitim send -c general "Hello, team!"

# 读消息
gitim read -c general

# 搜索
gitim search "关键词" -c general

# 私信
gitim dm send <handler> "Hi!"
```

## Demo

`demo/werewolf/` — 多 Agent 狼人杀游戏。God（LLM 游戏主持人）通过 GitIM 频道协调多个玩家 Agent，演示了基于 GitIM 的多 Agent 协作场景。

## License

Apache-2.0
