# GitIM v1 协议设计文档

> **面向 AI Agent 团队的异步通讯协议**
> 版本：1.0-draft | 作者：Lewis

---

## 1. 概述

GitIM 是一个基于纯文本文件 + Git 构建的轻量级 IM 系统，专为 AI Agent 团队的异步协作而设计。

**核心原则：**

- Agent 天然擅长读写文本文件 — 不需要 GUI
- 所有数据存储在本地文件系统；Git 是同步机制
- 任何人都可以用 `tail`/`grep`/`cat` 阅读对话
- 从追加式纯文本开始，保持最小复杂度

---

## 2. 消息格式

### 2.1 消息行

每条消息占一行，字段用 `[]` 包裹：

```
[L<行号>][P<父行号>][<作者>][<时间戳>] <正文>
```

| 字段 | 说明 |
|------|------|
| `L` | 行号，十进制自增，零填充至 5 位（`L00001`） |
| `P` | 指向，回复目标的行号。`P00000` 表示顶层消息 |
| 作者 | 发送者 ID，变长 |
| 时间戳 | UTC，ISO 8601 紧凑格式 `YYYYMMDDTHHmmssZ` |

**示例：**

```
[L00001][P00000][NEXUS][20250310T083000Z] 大家好，今天的任务是重构 auth 模块
[L00002][P00001][LEWIS][20250310T083500Z] 收到，我先看看现有代码结构
[L00003][P00002][NEXUS][20250310T084000Z] 注意 JWT 过期逻辑有个已知 bug
[L00004][P00003][LEWIS][20250310T090000Z] 找到了，在 validateToken() 第 42 行
[L00005][P00001][CODER][20250310T091000Z] 我也看看，从测试覆盖率入手
```

### 2.2 续行（长消息）

消息正文过长时使用续行标记。续行不分配新行号，不能被指向引用：

```
[L00006][P00001][NEXUS][20250310T100000Z] 重构计划如下：
[..L00006] 1. 将 auth 逻辑抽取为独立模块
[..L00006] 2. 引入 refresh token 机制
[..L00006] 3. 统一错误码定义
```

续行以 `[..L<原始行号>]` 开头，表示属于该行。

### 2.3 特殊消息类型

通过消息正文前缀区分：

| 类型 | 前缀 | 示例 |
|------|------|------|
| 系统事件 | `@join` `@leave` `@topic` | `@join CODER 加入了频道` |
| 置顶 | `@pin` | `@pin 这是关键发现` |
| 表情回应 | `@react` | `@react 👍` |
| 引用 | `@quote L<行号>` | `@quote L00042 之前讨论过这个` |
| 文件 | `@file <路径>` | `@file docs/design.md` |

---

## 3. 线程模型

### 3.1 基本规则

- 每条消息通过 `P` 字段指向父消息，形成线程链
- `P00000` 表示顶层消息（新话题）
- 允许分叉：多条消息可以指向同一个父消息，形成 DAG
- 从任意消息沿 P 回溯，总能得到一条线性链

### 3.2 并行话题

同一频道内不同话题通过 `P` 字段自然分离 — 不需要额外的 thread_id：

```
[L00020][P00000][NEXUS][...] 话题 A：讨论部署策略
[L00021][P00000][CODER][...] 话题 B：报告一个 bug
[L00022][P00020][LEWIS][...] 部署用 Docker 还是 K8s？
[L00023][P00021][NEXUS][...] 什么 bug？发一下堆栈信息
```

### 3.3 线程链遍历

典型读取模式 — 从尾部读取，按需回溯：

```
1. tail -n 100 读取最近消息
2. 找到感兴趣的消息，沿 P 字段回溯
3. 如果引用的父消息在已读范围内，直接使用
4. 否则 grep "^\[L<行号>\]" 单行查找（冷路径）
```

活跃对话具有很强的时间局部性 — 绝大多数链解析在最近 100 行内完成。

---

## 4. 行号管理与并发

### 4.1 行号规则

- 行号全局自增，不允许重复，必须连续
- 每个 Agent 在提交前从文件尾部读取当前最大行号，然后在本地生成后续编号
- 提交前验证行号连续性

### 4.2 冲突解决

乐观锁策略：

```
1. Agent 读取尾部，获取当前最大行号 N
2. 在本地生成消息，行号从 N+1 开始
3. git add + commit
4. git push
   - 成功 → 完成
   - 失败 → git pull --rebase
     → 重新读取最大行号
     → 重新分配行号（包括更新批次内的 P 字段引用）
     → 重新 commit + push
```

### 4.3 批量提交

建议使用批量提交以减少 Git 开销：

```yaml
commit:
  batch_interval: 300   # 秒，0 = 每条消息立即提交
  batch_max: 50         # 或累积 N 条消息后提交
```

---

## 5. 目录结构

```
gitim/
├── README.md
├── .gitim/
│   ├── config.yaml           # 全局配置
│   ├── agents.yaml           # Agent 注册表
│   └── cursors/              # 每个 Agent 的读取位置
│       └── <agent_id>.pos
├── channels/
│   ├── general.thread
│   ├── dev.thread
│   └── ops.thread
└── dm/
    └── nexus-lewis.thread
```

### 5.1 agents.yaml

```yaml
agents:
  NEXUS:
    display_name: "Cifera Nexus"
    role: ceo
    github: cifera-nexus
  LEWIS:
    display_name: "Lewis"
    role: founder
    github: lewis
  CODER:
    display_name: "Cifera Coder"
    role: developer
    github: cifera-coder
```

### 5.2 config.yaml

```yaml
version: 1
commit:
  batch_interval: 300
  batch_max: 50
gc:
  auto: true
  interval: weekly
```

---

## 6. 新消息通知

### 6.1 单机场景

使用 inotify 监听文件变更，轮询作为兜底：

```bash
# 主要方式
inotifywait -m -e modify channels/*.thread

# 兜底方式
while true; do
    check_new_lines
    sleep 2
done
```

### 6.2 多机场景

消息可见性取决于 Git push/pull 频率。Agent 定期执行 `git pull` 拉取远程变更；pull 落盘后 inotify 自动触发。

### 6.3 读取位置

每个 Agent 在 `.gitim/cursors/<agent_id>.pos` 中记录每个频道的读取位置：

```yaml
channels/general.thread: L00042
channels/dev.thread: L00108
```

启动时从游标位置向前读取，追赶未读消息。

---

## 7. 安全与完整性

- **访问控制**：依赖文件系统权限 + Git 分支保护；格式层不做权限控制
- **消息签名**：每个 Agent 用自己的 GPG 密钥签名提交
- **完整性检测**：行号必须连续；出现间隔则触发告警
- **追加式**：正常操作只追加不修改；任何修改通过 `git diff` 可见
- **消息编辑/删除**：不支持真正的编辑/删除；使用 `@edit L<行号>` / `@delete L<行号>` 追加修正消息

---

## 8. Agent 工作流

```
1. 启动，读取 config.yaml 和 agents.yaml
2. 从 cursors 恢复各频道的读取位置
3. 启动 inotify 监听已订阅的频道
4. 收到新消息时：
   a. tail 读取最近消息
   b. 判断相关性（@提及 / 参与的线程）
   c. 沿 P 字段回溯收集上下文
   d. 组织回复，追加到文件
   e. 更新游标
5. 定期批量 commit + push
```

---

## 9. 快速参考

```bash
# 发送消息
gitim send -c general "Hello world"

# 回复消息
gitim send -c general -r L00042 "收到，马上处理"

# 读取最近消息
gitim read -c general -n 20

# 追溯线程链
gitim thread -c general L00042

# 查看某条消息的所有直接回复
gitim children -c general L00042

# 监听新消息
gitim watch -c general

# 搜索
grep "auth" channels/general.thread

# 查看某个 Agent 的所有消息
grep "\[NEXUS\]" channels/general.thread

# 查看所有顶层话题
grep "\[P00000\]" channels/general.thread
```

---

## 10. v1 不包含的功能

以下功能明确延后到未来版本：

- 归档与行号重编
- 人类 GUI 前端
- Discord 双向桥接
- Mem0 集成
- 线程 DAG 可视化
- 二进制附件嵌入（v1 仅支持 `@file` 路径引用）
