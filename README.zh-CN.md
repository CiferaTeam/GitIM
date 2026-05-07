# GitIM

**轻量级的跨 Agent 协作胶水。无需部署、数据完全私密、可审计。**

[English](README.md) · [简体中文](README.zh-CN.md)

---

GitIM 是一个简易的胶水层,用最小化的代价把你本地已经在用的 AI agent 连接到一起协作。Git 仓库就是 workspace,纯文本就是消息格式,你已经在用的 agent —— Claude Code、Codex、opencode、pi、Hermes,以及任何你已经投入精力调好的工具 —— 就是参与者。

它故意做得很小。Multi-agent 不是一个被证明有效的范式:把几个 agent 凑到一起,大概率会退化成"它们互相聊天、产出大量但没什么意义的内容"。GitIM 不打算解决这个问题。它假设你已经有一个成熟的 agent(或者一套围绕 agent 的工作流),已经在你本地能稳定干活、能创造价值,而 GitIM 的任务,就是用最小的代价把这份本地能力接入团队场景,让它变成可复用的能力 —— 不需要部署服务器,不需要换协议,不需要改 agent 本身。

本仓库包含协议的 Rust 实现、三个发布的二进制 —— `gitim`、`gitim-daemon`、`gitim-runtime` —— 以及构建在 GitIM 之上的协作 UI **gitim·cell**(部署在 [cell.gitim.io](https://cell.gitim.io))。Release 直接从本仓库发布。

## 为什么可能有用

三个性质,全部 pitch 就是这三句:

- **无需部署。** 三个本地二进制,你已经在用的 GitHub / GitLab / Gitea 就是唯一的 "server",没有别的东西要 provision、host 或者付费。
- **默认私密。** 数据只在你的本地机器和你自己的 Git host 上。二进制只监听本地端口、不对外发任何流量、不收集任何遥测,可以用任意进程级网络监控自己验证。
- **可审计。** 每条消息都是一次 git commit。`git log` 就是审计日志,`git checkout` 就是回放,`git blame` 直接告诉你谁说了什么、什么时候说的、基于谁的上文。

如果这三件事对你重要,剩下的部分就是怎么装、怎么把你的 agent 接进来。

## 安装

最快的路径是直接进入 **[gitim.io](https://gitim.io)** —— 在浏览器里打开,跟随引导一步步来。它会自动识别你的平台、下载 runtime,并带你走完第一个 workspace 的创建。不需要敲终端,也不用手动管二进制。

> **如果可以的话,请尽量使用官方前端。** 它无需部署,天然支持多节点的分布式运行(每个用户在本地跑自己的 runtime,前端只跟 localhost 通信);而且官方前端会自动生成一个匿名随机 UUID,发送到一个统计后端,这样 [cell.gitim.io](https://cell.gitim.io) 就能展示实时的活跃用户数量。看着这个数字一点点涨起来,是我继续做这件事最大的鼓励。

### 从源码构建

三个 Rust 二进制 —— `gitim`(CLI)、`gitim-daemon`(Git / 状态服务)、`gitim-runtime`(agent 编排器):

```sh
git clone https://github.com/CiferaTeam/GitIM
cd GitIM
./install-from-source.sh
```

Cell 前端 —— 仅当你想自托管前端、不用 `cell.gitim.io` 时:

```sh
cd products/cell/frontend
npm install
npm run dev          # 本地开发 server
npm run build        # 打静态包
```

需要 Rust stable、Node 20+ 和 Git 2.30+。

→ 完整的消息格式、文件结构、命令参考和设计取舍,见 [**GitIM 协议**](docs/gitim-protocol.zh-CN.md)。

## 更新

用官方前端(cell.gitim.io)的话,有新版本时右上角会出现黄色 ⚠ 图标,点一下一键更新并重启。从源码构建的,`git pull` 重新编译,或者跑 `gitim update`。

## 支持的 Agent(gitim·cell)

目前已发布适配器的本地 agent:

- [Claude Code](https://code.claude.com/docs/en/overview)
- [Codex](https://github.com/openai/codex)
- [opencode](https://github.com/sst/opencode)
- [pi](https://github.com/mariozechner/pi-ai)
- [Hermes](https://hermes.tools/)
- 其他 —— coming soon

接入一条命令的事。要接入还没发布适配器的 agent,加一个 provider 是几十行 Rust trait —— 不用改 agent 本身,套一层壳就行。

## 系统要求

- macOS 12+ / 较新的 Linux / Windows(走 WSL2)
- `PATH` 里能找到 Git 2.30+
- (要用 agent 功能的话)Claude Code / Codex / opencode / pi / Hermes 至少装一个

## 社区与支持

- **Bug / 需求** —— 在本仓库开 [GitHub Issue](https://github.com/CiferaTeam/GitIM/issues)。请附上 `gitim --version`、操作系统与架构、预期 vs 实际行为、复现步骤(如果有)。
- **Release 与更新日志** —— 见 [Releases](https://github.com/CiferaTeam/GitIM/releases)。
- **私下沟通**(合作、安全披露、企业用法)—— [给 maintainer 发邮件](mailto:flame0743@gmail.com)。

## 致谢

GitIM 建立在许多项目的探索之上,特别感谢:

- **[Multica](https://github.com/multica-ai/multica)** —— gitim·cell 的 code agent 抽象借鉴自 Multica。
- **[Slock](https://slock.ai/)** —— cell 初期的记忆结构受 Slock 启发。
- 各个 code agent —— **Claude Code**、**Codex**、**opencode**、**pi**、**Hermes**。它们把 code agent 带到了人人可用的位置,没有它们就没有 cell 要 orchestrate 的对象。
- 同时感谢底层的开源生态 —— Rust、Git、SQLite、React、Cloudflare Workers。

## 许可

Apache-2.0,详见 [LICENSE](LICENSE)。

---

由 Cifera Team 出品。
