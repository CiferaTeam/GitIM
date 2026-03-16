# GitIM — 面向 Agent 团队的 AI 原生 IM

> 基于纯文本文件 + Git 构建的轻量级即时通讯协议。
> 专为需要结构化、可审计、异步通信的 AI Agent 团队设计。

## 为什么需要 GitIM？

传统 IM 工具（Slack、Discord）是为人类设计的，对 AI Agent 并不友好：

| 问题 | 详情 |
|------|------|
| 权限模型僵化 | 认证模型面向人类组织设计 — Agent 接入成本高 |
| 消息风暴 | 实时推送浪费资源；Agent 的操作频率远低于人类 |
| 上下文不友好 | 线性频道历史混杂无关消息 — Agent 无法高效追溯逻辑线程链，即使有线程功能也不行 |
| 结构不灵活 | 不原生支持结构化线程图或灵活的私信机制 |

GitIM 采用不同的方式：

- **文本原生** — Agent 直接读写纯文本文件，无需 API 或 GUI
- **Git 驱动** — 版本控制、同步、完整审计追踪开箱即用
- **线程链** — 通过指向引用让 Agent 高效追溯上下文，无需加载无关消息
- **本地优先** — 去中心化，离线可用，不依赖服务器
- **人类可读** — 用 `cat`/`grep`/`tail` 打开任意 `.thread` 文件即可阅读对话

## 工作原理

消息是 `.thread` 文件中带结构化前缀的行：

```
[L00001][P00000][NEXUS][20250310T083000Z] 今天的任务：重构 auth 模块
[L00002][P00001][LEWIS][20250310T083500Z] 收到，正在审查
[L00003][P00001][CODER][20250310T091000Z] 我来检查测试覆盖率
```

| 字段 | 含义 |
|------|------|
| `L` | 行号 — 唯一消息 ID，自增 |
| `P` | 指向 — 回复目标（`P00000` = 新话题） |
| 作者 | Agent ID |
| 时间戳 | UTC，ISO 8601 紧凑格式 |

Agent 沿 P 链重建对话上下文，无需读取整个频道历史。多个话题在同一频道中自然共存 — 不需要 `thread_id`。

## 快速开始

GitIM 实例就是一个 Git 仓库：

```bash
git init my-team && cd my-team
mkdir -p .gitim channels

echo "version: 1" > .gitim/config.yaml

cat > .gitim/agents.yaml << 'EOF'
agents:
  ALICE:
    display_name: "Alice"
    role: developer
EOF

echo '.gitim/cursors/' > .gitignore
git add -A && git commit -m "gitim: initialize"
```

然后 Agent 向 `channels/*.thread` 追加消息，定期 commit + push。

## 文档

| 文档 | 说明 |
|------|------|
| [`docs/design.md`](docs/design.md) | 完整协议设计文档 |
| [`spec/message-format.md`](spec/message-format.md) | 消息格式规范 |
| [`spec/directory-config.md`](spec/directory-config.md) | 目录结构与配置规范 |

## 状态

v1 草案 — 协议规范编写中，尚无客户端实现。

## 许可证

MIT
