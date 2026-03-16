# GitIM

面向 Agent 团队的 AI 原生 IM 协议。纯文本文件 + Git。

## 关键文件

- `docs/superpowers/specs/2026-03-16-gitim-v1-design.md` — v1 协议设计文档
- `legacy/` — 废案参考文档（design.md, message-format.md, directory-config.md）

## 架构

- 消息是 `.thread` 文件中的行，前缀格式：`[L<行号>][P<父行号>][@<handler>][<时间戳>] <正文>`
- 通过 `P` 字段实现线程链 — 无需 thread_id
- 续行：下一行没有 `[L...]` 开头即为当前消息的续行
- 身份：`identities/<handler>.meta.json`，handler = GitHub handle（小写）
- 技术栈：Rust daemon（核心引擎）+ TypeScript CLI（薄客户端）
- 通信：Unix socket（默认）+ HTTP（调试模式）
- Git 负责持久化、同步和审计追踪

## v1 范围

- 三个模块：身份（identities）、频道（channels）、私信（dm）
- 消息格式：普通消息 + 回复，无特殊消息类型
- 行号：最少 6 位零填充，可无限增长
- 并发：乐观锁 + 冲突时 git pull --rebase
- 延后：特殊消息类型、MCP Server、归档、GUI、桥接

## 约定

- 所有文档使用中文
- Handler：小写 a-z 0-9 连字符，1-39 字符，`system` 为保留字
- DM 文件名：两个 handler 按字母序排列，`--` 连接
