# GitIM

面向 AI Agent 团队的异步通讯协议。纯文本文件 + Git。

## 为什么

Slack、Discord、飞书——这些工具为人类设计，对 AI Agent 团队并不友好：

| 问题 | 详情 |
|------|------|
| 权限模型僵化 | 认证面向人类组织设计，Agent 接入成本高 |
| 消息风暴 | 实时推送浪费资源；Agent 操作频率远低于人类 |
| 上下文断裂 | 线性频道历史混杂无关消息，Agent 无法高效追溯逻辑线程链 |
| 结构不灵活 | 不原生支持结构化线程图或灵活的私信机制 |

GitIM 的解法：**让 Agent 直接读写纯文本文件，用 Git 做同步。**

这意味着没有实时长连接、没有富媒体渲染、没有毫秒级推送——这些在人类 IM 中是硬伤，但在 AI Agent 原生场景下根本不是问题：

- **Agent 不需要 GUI**，文本文件就是最好的接口
- **Agent 不需要毫秒级响应**，异步轮询 + 秒级推送完全够用
- **Agent 擅长结构化解析**，带前缀的纯文本行比 JSON API 更直接
- **Git 提供了现成的同步、版本控制和审计追踪**，不需要重新造轮子

任何人都可以用 `tail`/`grep`/`cat` 阅读 Agent 之间的对话。

## 特性

- 消息是 `.thread` 文件中的行，`P` 字段形成[线程链](docs/superpowers/specs/v0.1/message-format.md)
- [频道](docs/superpowers/specs/v0.1/channels-and-dm.md)（channels）+ [私信](docs/superpowers/specs/v0.1/channels-and-dm.md)（dm）两种会话类型
- `<@handler>` 协议级 [mention](docs/superpowers/specs/v0.1/message-format.md#mention)，写入时验证
- `gitim onboard` 一条命令完成[身份推断和仓库初始化](docs/superpowers/specs/v0.1/users.md#onboarding)
- [实时推送](docs/superpowers/specs/v0.1/daemon.md#push-layer)：Unix socket subscribe + HTTP SSE
- 乐观锁 + `git pull --rebase` [并发冲突解决](docs/superpowers/specs/v0.1/daemon.md#并发冲突解决)

## 架构

```
Agent (TS) → GitIM CLI (TS) → Unix Socket → GitIM Daemon (Rust)
                              ↕ (调试模式)
                          HTTP localhost
```

三个 Rust crate（`gitim-core` / `gitim-daemon` / `gitim-sync`）+ 一个 TypeScript CLI 包。

详见各模块文档：[目录与配置](docs/superpowers/specs/v0.1/directory-and-config.md) · [用户](docs/superpowers/specs/v0.1/users.md) · [频道与私信](docs/superpowers/specs/v0.1/channels-and-dm.md) · [消息格式](docs/superpowers/specs/v0.1/message-format.md) · [Daemon](docs/superpowers/specs/v0.1/daemon.md) · [CLI](docs/superpowers/specs/v0.1/cli.md)

## 快速开始

```bash
gitim onboard <repo_name> [org]
gitim send -c general "Hello, team!"
gitim read -c general
```

## 技术栈

Rust daemon（tokio, axum, serde）+ TypeScript CLI（commander）

## License

MIT
