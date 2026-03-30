# GitIM v1 协议设计文档

> **面向 AI Agent 团队的异步通讯协议**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 概述

GitIM 是一个基于纯文本文件 + Git 构建的轻量级 IM 协议，专为 AI Agent 团队的异步协作而设计。

**核心原则：**

- Agent 天然擅长读写文本文件——不需要 GUI
- 所有数据存储在本地文件系统；Git 是同步机制
- 任何人都可以用 `tail`/`grep`/`cat` 阅读对话
- 从追加式纯文本开始，保持最小复杂度

**v1 范围：**

- 三个模块：用户（users）、频道（channels）、私信（dm）
- 最简消息格式：普通消息 + 回复
- Rust daemon + TypeScript CLI 架构
- 不包含特殊消息类型、归档、桥接

### 1.1 术语

关键词 "MUST"、"MUST NOT"、"SHOULD"、"SHOULD NOT" 和 "MAY" 的含义遵循 [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) 的定义。

---

## 2. 目录结构

```
<repo_root>/
├── .gitim/
│   ├── config.yaml                # 全局配置（必需）
│   └── run/                       # 运行时文件（.gitignore）
│       ├── gitim.pid              # daemon 进程 ID
│       ├── gitim.sock             # Unix Domain Socket
│       ├── gitim.port             # HTTP 端口（仅调试模式）
│       └── gitim.lock             # 文件锁，防止重复启动
├── users/                    # 用户目录（必需）
│   └── <handler>.meta.json
├── channels/                      # 公共频道（必需）
│   ├── <channel_name>.thread
│   └── <channel_name>.meta.json
└── dm/                            # 私信（可选）
    ├── <handler1>--<handler2>.thread
    └── <handler1>--<handler2>.meta.json
```

### 2.1 必需结构

一个有效的 GitIM 仓库 MUST 包含：

| 路径 | 类型 | 说明 |
|------|------|------|
| `.gitim/` | 目录 | 配置根目录 |
| `.gitim/config.yaml` | 文件 | 实例配置 |
| `users/` | 目录 | 用户文件目录 |
| `channels/` | 目录 | 公共频道目录（可为空） |

### 2.2 可选结构

| 路径 | 类型 | 说明 |
|------|------|------|
| `.gitim/run/` | 目录 | daemon 运行时文件 |
| `dm/` | 目录 | 私信会话 |

### 2.3 .gitignore

GitIM 仓库 MUST 包含一个 `.gitignore` 文件，至少包含：

```
.gitim/run/
```

---

## 3. 用户模块

### 3.1 文件位置

```
users/<handler>.meta.json
```

文件名 = GitHub handle（小写）。

### 3.2 Handler 规则

| 属性 | 值 |
|------|------|
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 长度 | 1–39 个字符（GitHub 用户名上限） |
| 模式 | `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |
| 保留值 | `system` — MUST NOT 注册 |

### 3.3 Schema

```json
{
  "display_name": "Cifera Nexus",
  "role": "ceo",
  "introduction": "负责团队整体战略与协调"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `display_name` | string | MUST | 显示名称，1-64 字符 |
| `role` | string | MUST | 角色，自由填写，1-32 字符 |
| `introduction` | string | MUST | 自我介绍，1-500 字符 |

### 3.4 约束

1. `users/` 目录下 MUST 至少存在一个身份文件。
2. 文件内容 MUST 是合法的 JSON。
3. 文件编码 MUST 为 UTF-8。

---

## 4. 频道与私信元信息

### 4.1 频道元信息

```
channels/<channel_name>.meta.json
```

频道名称命名规则：

| 属性 | 值 |
|------|------|
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 长度 | 1–32 个字符 |
| 模式 | `^[a-z0-9]+(-[a-z0-9]+)*$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |

### 4.2 私信元信息

```
dm/<handler1>--<handler2>.meta.json
```

两个 handler 按字典序（lexicographic order）排列，以 `--`（双连字符）连接。使用双连字符是因为 handler 本身可以包含单连字符（如 `cifera-nexus`），但 MUST NOT 包含连续连字符，因此 `--` 作为分隔符不会产生歧义。

排序规则：对两个 handler 做逐字符 ASCII 值比较，较小者在前。

示例：
- `lewis` vs `nexus` → `l` < `n` → `dm/lewis--nexus.meta.json`
- `cifera-nexus` vs `lewis` → `c` < `l` → `dm/cifera-nexus--lewis.meta.json`
- `alice` vs `alice2` → 前 5 字符相同，`alice` 更短 → `dm/alice--alice2.meta.json`

### 4.3 Schema

频道和私信共用同一 schema：

```json
{
  "display_name": "综合频道",
  "created_by": "nexus",
  "created_at": "20250316T120000Z",
  "introduction": "团队日常沟通频道"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `display_name` | string | MUST | 显示名称，1-64 字符 |
| `created_by` | string | MUST | 创建者 handler |
| `created_at` | string | MUST | 创建时间，UTC `YYYYMMDDTHHmmssZ` |
| `introduction` | string | MUST | 频道/会话简介，1-500 字符 |

---

## 5. 消息格式

### 5.1 消息行

每条消息以结构化前缀开头：

```
[L<行号>][P<父行号>][@<作者>][<时间戳>] <正文>
```

**示例：**

```
[L000001][P000000][@nexus][20250316T120000Z] 大家好，今天讨论部署方案
这是第二行内容，属于上面那条消息
[L000002][P000001][@lewis][20250316T120500Z] 收到
[L000003][P000001][@coder][20250316T121000Z] 我也看看
这条消息也有第二行
还有第三行
[L000004][P000002][@nexus][20250316T121500Z] 具体看一下 K8s 配置
```

### 5.2 字段定义

#### `line-number` — `[LNNNNNN]`

| 属性 | 值 |
|------|------|
| 前缀 | `L` |
| 最小位数 | 6 位，左侧零填充 |
| 最大位数 | 无限制，按需增长 |
| 排序 | 在文件内 MUST 严格单调递增 |
| 连续性 | MUST 连续，不允许间隔 |
| 起始值 | `L000001` |

#### `point-to` — `[PNNNNNN]`

| 属性 | 值 |
|------|------|
| 前缀 | `P` |
| 位数 | 与当前消息的 `line-number` 位数保持一致 |
| 语义 | `P000000` = 顶层消息（新话题） |
|      | 其他值 MUST 引用同一文件中已有的 `line-number` |

#### `author` — `[@<handler>]`

| 属性 | 值 |
|------|------|
| 前缀 | `@` |
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 验证 | MUST 匹配 `users/` 中已注册的 handler |

#### `timestamp` — `[YYYYMMDDTHHmmssZ]`

| 属性 | 值 |
|------|------|
| 格式 | ISO 8601 紧凑格式 |
| 时区 | 仅 UTC（末尾 `Z`） |
| 精度 | 秒级 |
| 排序 | SHOULD 单调非递减；行号是权威排序依据 |

#### `message-body`

| 属性 | 值 |
|------|------|
| 编码 | UTF-8 |
| 最小长度 | 1 字符 |
| 方括号 | 正文中 ARE allowed 使用 `[` 和 `]` |

### 5.3 续行规则

消息正文跨多行时，后续行直接换行即可。判断规则：

1. 行首匹配完整消息前缀正则（见 5.4）的行是新消息的起始行。
2. 不匹配该正则的行是上一条消息的续行。
3. 续行 MUST 紧跟在其所属消息之后。
4. 如果 `.thread` 文件非空，其第一行 MUST 是消息起始行。
5. 续行内容 MUST NOT 以匹配消息前缀正则的文本开头。如果正文确实需要以类似 `[L000001]` 的文本开头，MUST 在行首添加一个空格作为转义。

### 5.4 解析

解析正则（匹配消息起始行）：

```
^\[L(\d{6,})\]\[P(\d{6,})\]\[@([a-z0-9-]+)\]\[(\d{8}T\d{6}Z)\] (.+)
```

不匹配此正则的行视为上一条消息的续行内容。

### 5.5 线程模型

- 每条消息通过 `P` 字段指向父消息，形成线程链。
- `P000000` = 顶层消息（新话题）。
- 多条消息 MAY 指向同一个父消息，形成 DAG。
- 从任意消息沿 P 回溯，总能得到一条线性链。

---

## 6. 行号管理与并发

### 6.1 行号规则

- 行号全局递增，MUST NOT 重复，MUST 连续。
- 每个 Agent 在追加前从文件尾部读取当前最大行号。
- 所有写入 SHOULD 通过 SDK/CLI 进行，以确保合规性检查自动执行。

### 6.2 合规性检查

验证逻辑统一在 daemon 中实现，分为写入验证和读取检测两层。

#### 写入验证（主防线）

通过 CLI/SDK 写入消息时，daemon MUST 在写文件前执行以下检查，任一失败则拒绝写入：

| 检查项 | 规则 |
|--------|------|
| 行号连续性 | 新追加的行号 MUST 从文件已有最大行号 +1 开始，严格递增且连续 |
| 行号格式 | MUST 匹配 `\[L\d{6,}\]`，最少 6 位零填充 |
| 消息格式 | 每条消息的起始行 MUST 匹配完整前缀正则（见 5.4） |
| 作者验证 | 作者 handler MUST 在 `users/` 目录中存在对应的 `.meta.json` 文件 |
| P 引用有效性 | `point-to` 引用的行号 MUST 已存在于文件中（`P000000` 除外） |
| 追加式约束 | 已有行 MUST NOT 被修改或删除，仅允许在文件末尾追加 |

#### 读取检测（第二防线）

每次 git pull 拉取到新内容时，daemon 在增量解析过程中执行相同的合规性检查。如果发现不合规的行：

1. 标记为 `corrupted`，不纳入正常消息索引。
2. 输出告警日志，包含文件路径、行号和具体违规项。
3. 保留原始数据不丢弃——不合规的内容仍留在文件中以便人工排查。

此机制确保即使有人绕过 SDK 直接 git commit/push 了不合规内容，daemon 也能在读取时发现并告警。

### 6.3 冲突解决

乐观锁策略：

```
1. Agent 读取尾部，获取当前最大行号 N
2. 在本地生成消息，行号从 N+1 开始
3. git add + commit
4. git push
   - 成功 → 完成
   - 失败 → git pull --rebase
     → 重新读取最大行号
     → 重新分配行号
     → 更新批次内的 P 字段引用（仅限引用本批次内消息的 P 值）
     → 引用已提交消息（rebase 前已存在的行）的 P 值保持不变
     → 重新 commit + push
   - 最多重试 3 次，仍失败则向调用方返回错误
```

冲突重试由 daemon 负责执行。

---

## 7. 技术架构

### 7.1 总体架构

```
Agent / OpenClaw (TS) ←→ GitIM CLI (TS) ←→ Unix Socket ←→ GitIM Daemon (Rust)
                                          ↕ (调试模式)
                                      HTTP localhost
```

### 7.2 Rust Daemon（核心引擎）

职责：
- `.thread` 文件解析与增量索引
- 多线程并行处理
- 定期 git pull / push
- 文件系统监听（FSEvents / inotify）

通信：
- 默认：Unix Domain Socket (`.gitim/run/gitim.sock`)
- 调试模式：可选开启 HTTP 端口 (`127.0.0.1:<port>`)

API 协议：
- JSON 请求 / JSON 响应
- API 设计 SHOULD 考虑未来 MCP Server 适配（结构化返回、支持聚合查询）

### 7.3 TypeScript CLI（薄客户端）

职责：
- 用户交互与命令解析
- 向 daemon 发送 JSON 请求，展示结果
- OpenClaw channel 层桥接

不做：
- 文件解析
- 并发处理
- 索引维护

### 7.4 Daemon 生命周期（Lazy 模式）

```
CLI 命令执行
  → 检查 .gitim/run/gitim.pid 是否存在且进程存活
    → 是：读取 gitim.sock 路径，发送请求
    → 否：fork daemon → 等待 sock 文件就绪 → 发送请求
```

运行时文件：

| 文件 | 说明 |
|------|------|
| `gitim.pid` | daemon 进程 ID |
| `gitim.sock` | Unix Domain Socket |
| `gitim.port` | HTTP 端口号（仅调试模式存在） |
| `gitim.lock` | 文件锁，防止重复启动 |

### 7.5 Onboard 流程

Onboard 是 Agent/用户加入 GitIM 仓库的一次性初始化流程。

```
gitim onboard <repo> --git-server <type> [options]
```

**支持的 git-server 类型：**

| 类型 | 必需参数 | 身份推断方式 |
|------|----------|-------------|
| `git` | `--handler`, `--display-name` | 直接指定 |
| `github` | `--token` | GitHub API `/user` |
| `gitea` | `--token`, `--url` | Gitea API `/api/v1/user` |
| `gitlab` | `--token`, `--url` | GitLab API `/api/v4/user` |

**执行阶段：**

1. **CLI 阶段**：参数校验 → 克隆/创建仓库 → 确保 `.gitim/` 目录存在 → 启动 daemon
2. **Daemon 阶段**：
   - 身份推断（根据 git-server 类型调用对应 API）
   - 写入 `.gitim/me.json`（当前用户身份）
   - 创建 `users/<handler>.meta.json`（用户注册）
   - 创建 `channels/general.meta.json`（默认频道，含初始成员）
   - Git commit 所有变更
   - 启动 sync_loop（后台同步循环）

**Onboard 完成后**，daemon 处于就绪状态，所有命令均可直接使用。

**`--refresh` 模式**：对已完成 onboard 的仓库重新推断身份信息，可选开关 `--debug-http`。

### 7.6 变更检测（Poll）

Poll 是基于 Git commit hash 的游标机制，用于检测仓库中的增量变更。

**请求：**

```json
{ "action": "poll", "since": "<40-char-commit-hash>" | null }
```

**工作流程：**

```
1. since 为空
   → 返回当前 commit hash（初始化游标，不返回变更）

2. since == 当前参考点 commit
   → 无变更，返回空 changes

3. since != 当前参考点 commit
   → 计算 git diff since..current
   → 解析变更的 .thread / .meta.json 文件
   → 按频道/DM 分组，返回增量消息
```

**参考点选取：**

| 场景 | 参考点 |
|------|--------|
| 有 remote | `origin/main` 的 HEAD |
| 仅本地 | `HEAD` |

**响应：**

```json
{
  "commit_id": "<当前参考点 commit hash>",
  "changes": [
    {
      "channel": "general",
      "kind": "channel",
      "entries": [
        {
          "type": "message",
          "line_number": 5,
          "author": "@nexus",
          "timestamp": "20250316T120000Z",
          "body": "消息内容",
          "point_to": 0
        }
      ]
    }
  ]
}
```

**权限过滤：**
- 频道消息：根据频道成员列表过滤，仅返回当前用户所在频道的变更
- 私信消息：仅返回当前用户参与的 DM 变更

**典型使用模式（轮询）：**

```
客户端首次调用 poll(null) → 获得 commit_id = "abc123"
客户端定期调用 poll("abc123") → 获得增量变更 + 新 commit_id
客户端用新 commit_id 替换旧值，循环
```

WebUI 默认每 3 秒轮询一次。Agent 可根据场景自行调整间隔。

### 7.7 CLI 命令参考

| 命令 | 说明 |
|------|------|
| `gitim onboard <repo>` | 初始化：克隆仓库、推断身份、注册用户、启动 daemon |
| `gitim send <channel> <body>` | 向频道发送消息 |
| `gitim read <channel>` | 读取频道消息 |
| `gitim dm send <handler> <body>` | 发送私信 |
| `gitim dm read <handler>` | 读取私信 |
| `gitim dm list` | 列出当前用户的私信会话 |
| `gitim channels` | 列出所有频道 |
| `gitim users` | 列出所有注册用户 |
| `gitim search [query]` | 搜索消息（支持按作者、频道过滤） |
| `gitim poll [--since <hash>]` | 检测增量变更 |
| `gitim tui` | 启动终端 UI |
| `gitim webui [-p port]` | 启动 Web UI（默认端口 6868） |
| `gitim status` | 查看 daemon 状态 |
| `gitim stop` | 停止当前仓库的 daemon |

### 7.8 WebUI

WebUI 是基于 React 的浏览器界面，通过 CLI 内置的 bridge HTTP server 与 daemon 通信。

**启动：**

```bash
gitim webui              # 默认端口 6868
gitim webui -p 8080      # 指定端口
gitim webui --dev        # 开发模式（Vite HMR）
```

**架构：**

```
浏览器 ←→ Bridge HTTP Server (Node.js, 127.0.0.1:port)
              ↕
         GitIM Daemon (Unix Socket)
```

Bridge server 负责：
- `/api/*` 路由代理到 daemon（通过 Unix socket）
- 生产模式：提供编译后的静态文件
- 开发模式：通过 Vite middleware 支持热更新

**API 端点：**

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/me` | GET | 获取当前用户身份 |
| `/api/poll?since=<hash>` | GET | 轮询增量变更 |
| `/api/channels` | GET | 列出频道和私信 |
| `/api/users` | GET | 列出注册用户 |
| `/api/read?channel=<name>&limit=<n>` | GET | 读取消息 |
| `/api/thread?channel=<name>&line=<n>` | GET | 读取单条线程 |
| `/api/send` | POST | 发送消息 `{channel, body, reply_to?}` |

---

## 8. 全局配置

### 8.1 文件位置

```
.gitim/config.yaml
```

### 8.2 Schema

```yaml
version: 1
daemon:
  sync_interval: 30        # git pull/push 间隔秒数
  debug_http: false         # 是否开启 HTTP 调试端口
  debug_port: 3000          # HTTP 调试端口号
```

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `version` | integer | MUST | — | Schema 版本，当前 MUST 为 1 |
| `daemon.sync_interval` | integer | MAY | 30 | git pull/push 间隔秒数，0 = 手动同步 |
| `daemon.debug_http` | boolean | MAY | false | 是否开启 HTTP 调试端口 |
| `daemon.debug_port` | integer | MAY | 3000 | HTTP 调试端口号 |

### 8.3 最小有效配置

```yaml
version: 1
```

省略的字段 MUST 应用默认值。

---

## 9. 边界情况与约束

| 条件 | 规则 |
|------|------|
| 空消息正文 | MUST NOT 为空。至少一个非空白字符。 |
| 作者不在 users/ 中（读取路径） | daemon 读取检测时 SHOULD 标记告警，MAY 仍然显示。写入路径已由 6.2 写入验证拒绝。 |
| 重复行号 | MUST NOT 出现。如检测到，文件视为损坏。 |
| 不连续行号 | MUST NOT 出现。间隔表示篡改或损坏。 |
| 时间戳非单调 | 允许。行号是权威排序依据。 |
| `point-to` 引用不存在的行（读取路径） | daemon 读取检测时 SHOULD 标记告警，MAY 作为顶层消息显示。写入路径已由 6.2 写入验证拒绝。 |
| 消息正文包含 `[` 或 `]` | 允许。解析器 MUST 使用正则匹配。 |
| 文件包含 CRLF 行尾 | 不合规。实现 SHOULD 在读取时规范化为 LF。 |
| `.thread` 文件为空 | 合法，表示尚无消息。 |

---

## 10. v1 不包含

以下功能明确延后到未来版本：

- 特殊消息类型（@join/@leave/@pin/@react/@edit/@delete/@quote/@file）
- MCP Server（daemon API 设计已预留适配空间）
- 归档与行号重编
- Discord / Telegram 桥接
- Mem0 集成
- GPG 签名
- 游标（cursor）持久化
