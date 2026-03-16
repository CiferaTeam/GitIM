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

- 三个模块：身份（identities）、频道（channels）、私信（dm）
- 最简消息格式：普通消息 + 回复
- Rust daemon + TypeScript CLI 架构
- 不包含特殊消息类型、归档、GUI、桥接

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
├── identities/                    # 身份目录（必需）
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
| `identities/` | 目录 | 身份文件目录 |
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

## 3. 身份模块

### 3.1 文件位置

```
identities/<handler>.meta.json
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

1. `identities/` 目录下 MUST 至少存在一个身份文件。
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
| 模式 | `^[a-z0-9]([a-z0-9](-[a-z0-9]+)*)?$` |
| 限制 | MUST NOT 以连字符开头或结尾；MUST NOT 包含连续连字符 |
| 长度 | 1–32 个字符 |

### 4.2 私信元信息

```
dm/<handler1>--<handler2>.meta.json
```

两个 handler 按字母序排列，以 `--`（双连字符）连接。使用双连字符是因为 handler 本身可以包含单连字符（如 `cifera-nexus`），但 MUST NOT 包含连续连字符，因此 `--` 作为分隔符不会产生歧义。

示例：Agent `nexus` 和 `lewis` → `dm/lewis--nexus.meta.json`
示例：Agent `cifera-nexus` 和 `lewis` → `dm/cifera-nexus--lewis.meta.json`

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
| 位数 | 与同文件中 `line-number` 保持一致 |
| 语义 | `P000000` = 顶层消息（新话题） |
|      | 其他值 MUST 引用同一文件中已有的 `line-number` |

#### `author` — `[@<handler>]`

| 属性 | 值 |
|------|------|
| 前缀 | `@` |
| 字符集 | 小写字母 `a-z`、数字 `0-9`、连字符 `-` |
| 验证 | MUST 匹配 `identities/` 中已注册的 handler |

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
- Agent 在提交前 MUST 验证行号连续性。

### 6.2 冲突解决

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
```

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
| 作者不在 identities/ 中 | 消息格式错误。实现 SHOULD 警告，MAY 仍然显示。 |
| 重复行号 | MUST NOT 出现。如检测到，文件视为损坏。 |
| 不连续行号 | MUST NOT 出现。间隔表示篡改或损坏。 |
| 时间戳非单调 | 允许。行号是权威排序依据。 |
| `point-to` 引用不存在的行 | 格式错误。实现 SHOULD 警告，MAY 作为顶层消息显示。 |
| 消息正文包含 `[` 或 `]` | 允许。解析器 MUST 使用正则匹配。 |
| 文件包含 CRLF 行尾 | 不合规。实现 SHOULD 在读取时规范化为 LF。 |
| `.thread` 文件为空 | 合法，表示尚无消息。 |

---

## 10. v1 不包含

以下功能明确延后到未来版本：

- 特殊消息类型（@join/@leave/@pin/@react/@edit/@delete/@quote/@file）
- MCP Server（daemon API 设计已预留适配空间）
- 归档与行号重编
- GUI 前端
- Discord / Telegram 桥接
- Mem0 集成
- GPG 签名
- 游标（cursor）持久化
