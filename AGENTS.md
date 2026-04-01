# GitIM

面向 Agent 团队的 AI 原生 IM 协议。纯文本文件 + Git。

## 关键文件

- `docs/superpowers/specs/2026-03-16-gitim-v1-design.md` — v1 协议设计文档
- `legacy/` — 废案参考文档（design.md, message-format.md, directory-config.md）

## 架构

- 消息是 `.thread` 文件中的行，前缀格式：`[L<行号>][P<父行号>][@<handler>][<时间戳>] <正文>`
- 通过 `P` 字段实现线程链 — 无需 thread_id
- 续行：下一行没有 `[L...]` 开头即为当前消息的续行
- 用户：`users/<handler>.meta.yaml`，handler = GitHub handle（小写）
- 技术栈：Rust daemon（核心引擎）+ TypeScript CLI（薄客户端）
- 通信：Unix socket（默认）+ HTTP（调试模式）
- Git 负责持久化、同步和审计追踪
- 合规性：daemon 写入验证（主防线）+ 读取检测（第二防线）

## Onboard 流程

CLI 现已完全委托 daemon 处理身份推断和仓库初始化：

1. **CLI 阶段**：收集用户参数（git 类型、token 等）
2. **仓库克隆/初始化**（CLI）：克隆或创建 git 仓库，创建 `.gitim/` 目录（git 忽略）
3. **Daemon 阶段**：
   - **身份推断**（Onboard 处理）：根据 git 类型和 token 推断 handler + 信息
   - **用户注册**（RegisterUser 处理）：创建 `users/<handler>.meta.yaml`
   - **Repo 初始化**：生成 `.gitim/config.yaml`、初始化 `me.json`
   - **Git 提交**：各文件变更提交到 git

支持的身份推断渠道：
- **git 本地模式**：直接指定 handler + display_name
- **GitHub**：通过 token 调用 API 获取用户信息
- **Gitea/GitLab**：通过 token + 自定义 URL 调用对应 API

## v1 范围

- 三个模块：用户（users）、频道（channels）、私信（dm）
- 消息格式：普通消息 + 回复，无特殊消息类型
- 行号：最少 6 位零填充，可无限增长
- 并发：乐观锁 + 冲突时 git pull --rebase
- 延后：特殊消息类型、MCP Server、归档、GUI、桥接

## 约定

- 所有文档使用中文
- Handler：小写 a-z 0-9 连字符，1-39 字符，`system` 为保留字
- DM 文件名：两个 handler 按字典序排列，`--` 连接
