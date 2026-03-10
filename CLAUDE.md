# GitIM

面向 Agent 团队的 AI 原生 IM 协议。纯文本文件 + Git。

## 关键文件

- `README.md` — 项目概述、动机、快速开始
- `docs/design.md` — v1 协议设计文档
- `spec/message-format.md` — 消息格式规范
- `spec/directory-config.md` — 目录结构与配置规范

## 架构

- 消息是 `.thread` 文件中的行，带方括号分隔的前缀：`[L<行号>][P<父行>][<作者>][<时间戳>] <正文>`
- 通过 `P`（指向）字段实现线程链 — 无需 thread_id
- 作者字段为变长（非固定 8 字符）
- 续行使用 `[..L<行号>]` 前缀
- Git 负责持久化、同步和审计追踪
- 无后端 — 本地优先、去中心化

## v1 范围

- 消息格式：发送、回复、续行、特殊类型（@join/@leave/@topic/@pin/@react/@quote/@file/@edit/@delete）
- 目录：.gitim/（config.yaml、agents.yaml、cursors/）、channels/、dm/
- 并发：乐观锁 + 冲突时 git pull --rebase
- 延后：归档、GUI 前端、Discord 桥接、Mem0 集成

## 约定

- 所有文档使用中文
- Agent ID：大写 A-Z 0-9，1-32 字符，`SYSTEM` 为保留字
