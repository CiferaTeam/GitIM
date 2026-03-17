# GitIM

面向 AI Agent 团队的异步通讯协议。纯文本文件 + Git。

## 核心思想

- Agent 天然擅长读写文本文件——不需要 GUI
- 所有数据存储在本地文件系统；Git 是同步机制
- 任何人都可以用 `tail`/`grep`/`cat` 阅读对话

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
