# GitIM 前端方案整合报告

> 作者：ux-architect | 日期：2026-03-17
> 状态：Task #6 最终整合

---

## 0. 整合方法论

本报告基于 5 份预研成果和 5 份交叉 Review 的发现，整合为 3 套可行方案。

评估维度（吸收 Review #5 建议，开发成本权重从 10% 提升到 20%）：

| 维度 | 权重 | 说明 |
|------|------|------|
| AI 对话可读性 | 25% | 高频长文本消息的阅读体验 |
| 线程结构表达 | 15% | point_to DAG 关系的可视化 |
| 场景覆盖度 | 15% | 观察/指挥/协作/审计/调试 五大场景 |
| 开发成本 | 20% | v1 可实现性，人力和时间 |
| 学习成本 | 10% | 新用户上手难度 |
| 性能 | 10% | 大量消息渲染性能 |
| 扩展性 | 5% | 未来功能兼容性 |

底线标准（硬性要求，不达标则方案不可行）：
1. point_to 关系不能丢失——用户必须能从任意消息追溯回复目标
2. 人类消息必须视觉突出——不能淹没在 AI 消息中
3. 代码块必须有语法高亮
4. Git 同步状态必须可见
5. 虚拟滚动或等效性能方案（Review #2 阻塞性问题）

---

## 1. 预研成果与 Review 发现总结

### 实时桥接层（realtime-bridge）
- **选型**：WebSocket Bridge（独立 Node.js 进程），零 daemon 侵入
- **原型**：bridge.ts 已实现 subscribe/unsubscribe/send + 500ms 轮询
- **问题**：channelCursors 全局共享，多客户端时后连接者丢历史；subscribe 无 since 参数
- **修复方向**：subscribe 支持 `since` 参数；区分 history（一次性拉取）和 live（增量推送）

### Web 前端（web-frontend）
- **选型**：Svelte 5 + Vite，分栏布局（时间线 + 线程面板）
- **原型**：完整可运行，含频道列表、消息气泡（Agent 色带）、线程面板、搜索过滤
- **亮点**：线程数据层（buildThreadTree/getThreadChain/getThreadReplies）设计扎实；Agent 色带 + 折叠合理
- **阻塞问题**：虚拟滚动完全缺失；`@html renderBody()` 有 XSS 风险
- **修复方向**：引入虚拟滚动库；用 marked + DOMPurify 替换手写 Markdown 渲染

### TUI（tui）
- **选型**：Ink（React for CLI），TypeScript 生态复用
- **原型**：三栏布局（频道列表/消息区/线程面板），Vim 风格键盘操作
- **问题**：人类消息无视觉区分（底线标准未达标）；代码块无语法高亮（底线标准未达标）；AI 消息信息过载无应对
- **修复方向**：增加人类/AI 样式区分；终端语法高亮（如 cli-highlight）；连续 AI 消息折叠

### 桌面客户端（desktop）
- **选型**：Tauri 2.0，Rust 后端直连 daemon + Web 前端
- **原型**：Tauri 脚手架 + React 前端，daemon.rs/watcher.rs/commands.rs 已实现
- **亮点**：Rust 生态协同（直接引用 gitim-core crate）、Unix Socket 直连、文件系统监听原生支持
- **问题**：前端框架用了 React 而非 Svelte（与 Web 方案不一致）；多仓库设计过早；与 Web 组件无复用
- **修复方向**：前端对齐 Svelte 5；先做单仓库；抽取共享组件包

### UX 设计
- **核心产出**：5 种线程可视化方案对比、通知分级（P0-P4）、Agent 状态模型
- **Review 反馈**：双视图（Mission Control + Thread Reader）对 v1 过度设计；Agent 状态缺 daemon API 支撑
- **采纳**：v1 聚焦 Thread Reader 单视图，Mission Control 作为 v2 方向

---

## 2. 三套可行方案

### 方案一："Web First"（推荐，最快落地）

**一句话**：Svelte 5 Web 应用 + WebSocket Bridge，v1 最快路径。

#### 技术栈

| 层 | 技术 | 来源 |
|----|------|------|
| 前端框架 | Svelte 5 + Vite | web-frontend 原型 |
| 实时通信 | WebSocket Bridge (Node.js) | realtime-bridge 原型 |
| Markdown | marked + DOMPurify | Review #2 建议 |
| 虚拟滚动 | @tanstack/virtual (svelte adapter) | Review #2 阻塞修复 |
| 代码高亮 | shiki（轻量，支持 .thread 自定义语法） | UX 设计建议 |

#### 架构

```
浏览器 (Svelte 5)
    |
    | WebSocket (ws://localhost:3100)
    |
WebSocket Bridge (Node.js)
    |
    | Unix Socket (行分隔 JSON)
    |
gitim-daemon (Rust)
    |
.thread 文件 + Git
```

#### 布局

采用 Thread Reader 单视图（吸收 Review #5 反馈，v1 不做双视图）：

```
+--------+----------------------------------+-------------------+
| 240px  |          MAIN AREA               |   THREAD PANEL    |
|        |                                  |   (380px, 可选)   |
| NAV    | +------------------------------+|                   |
|        | | #channel-name     [搜索][过滤]||  Thread: L1       |
| ====== | +------------------------------+|  @nexus           |
| 频道   | |                              ||                   |
| #general| | @nexus [12:00]      L000001  ||  +-- @lewis       |
| #dev   | | 讨论部署方案                  ||  |  收到           |
| #design| |                              ||  |                |
|        | |   +- 回复 @nexus             ||  |  +-- @nexus    |
| ====== | |   @lewis [12:05]    L000002  ||  |     看K8s      |
| 私信   | |   收到                        ||  |                |
| @nexus | |                              ||  +-- @coder      |
|        | | | @lewis [12:35]    HUMAN  | ||     我也看看      |
| ====== | | | 先别动 main.rs           | ||                   |
| 状态栏 | |                              || [回复 L1 v][发送] |
| sync ok| +------------------------------+|                   |
+--------+----------------------------------+-------------------+
```

关键交互：
- 主列：完整时间线，回复消息带 1 级缩进 + 引用标记（Discord 风格）
- 右栏：点击消息后展开线程树（嵌套展示，限 3 层）
- 人类消息：左侧彩色粗边框 + `HUMAN` 标签 + 微弱背景色
- AI 消息：左侧细色带（Agent 独有色）
- 长消息：默认显示 5 行，超出折叠
- 代码块：shiki 语法高亮 + 复制按钮

#### 从现有原型到 v1 的修复清单

| 优先级 | 修复项 | 来源 | 工作量 |
|--------|--------|------|--------|
| P0 | 虚拟滚动 | Review #2 | 2-3 天 |
| P0 | XSS 修复（marked + DOMPurify 替换 @html） | Review #2 | 1 天 |
| P0 | 人类消息视觉区分 | UX 底线标准 | 0.5 天 |
| P0 | 代码块语法高亮 | UX 底线标准 | 1 天 |
| P1 | Bridge subscribe 支持 since 参数 | Review #1 | 0.5 天 |
| P1 | Bridge history/live 分离 | Review #1 | 1 天 |
| P1 | Bridge per-client cursor 替代全局 cursor | Review #1 | 0.5 天 |
| P1 | Git 同步状态显示 | UX 底线标准 | 0.5 天 |
| P2 | 抽取 @gitim/thread-utils 共享包 | Review #2 | 1 天 |
| P2 | 通知分级（P0-P4） | UX 设计 | 2 天 |

总工期估算：**2-3 周**（1 人全职）可从原型到 v1 可用版本。

#### 评估

| 维度 | 分数 | 说明 |
|------|------|------|
| AI 对话可读性 | 4/5 | Agent 色带 + 折叠 + 语法高亮，覆盖核心需求 |
| 线程结构表达 | 4/5 | 时间线缩进 + 线程面板嵌套，双层保证 |
| 场景覆盖度 | 3/5 | 观察/协作/指挥覆盖好；审计/调试需要 v2 的 Git 历史浏览 |
| 开发成本 | 5/5 | 原型已有 80%，修复清单明确 |
| 学习成本 | 5/5 | 浏览器访问，零安装 |
| 性能 | 3/5 | 虚拟滚动修复后可达 4/5，WebSocket 延迟 500ms 可接受 |
| 扩展性 | 4/5 | Svelte 组件可复用于 Tauri；Bridge 可被 daemon 原生 SSE 替代 |

**加权总分：4.00/5**

---

### 方案二："Tauri Hybrid"（最佳体验，成本较高）

**一句话**：Tauri 2.0 桌面应用，复用 Web 前端的 Svelte 组件，Rust 后端直连 daemon。

#### 技术栈

| 层 | 技术 | 来源 |
|----|------|------|
| 桌面壳 | Tauri 2.0 | desktop 原型 |
| 前端框架 | Svelte 5（复用方案一组件） | web-frontend + 对齐修复 |
| 实时通信 | Rust 后端 notify crate + Tauri event | desktop 原型 watcher.rs |
| daemon 连接 | tokio::net::UnixStream（直连） | desktop 原型 daemon.rs |
| 消息解析 | gitim-core crate 直接引用 | desktop 关键验证 |

#### 架构

```
Tauri WebView (Svelte 5 -- 复用 Web 组件)
    |
    | Tauri IPC (invoke/event)
    |
Tauri Rust Backend
    +-- daemon.rs   -> UnixStream 直连 daemon
    +-- watcher.rs  -> notify 监听 .thread 文件变化
    +-- gitim-core  -> 本地消息解析（离线可用）
    |
gitim-daemon (Rust)
    |
.thread 文件 + Git
```

#### 相比方案一的差异化优势

1. **实时性**：文件系统 notify 事件 -> Tauri event -> 前端，延迟 < 50ms（方案一 500ms）
2. **离线体验**：Tauri Rust 后端可直接调用 gitim-core 解析本地 .thread 文件，daemon 不在线也能阅读历史
3. **系统集成**：系统托盘、全局快捷键、系统通知、开机启动
4. **安全性**：Tauri CSP + 进程隔离，无 XSS 问题（WebView 不加载远程资源）
5. **包体积**：约 5-8MB（Electron 150MB+）

#### 从现有原型到 v1 的修复清单

| 优先级 | 修复项 | 来源 | 工作量 |
|--------|--------|------|--------|
| P0 | 前端从 React 迁移到 Svelte 5 | Review #4 | 3-5 天 |
| P0 | 复用 web-frontend 的 Svelte 组件 | Review #4 | 2 天 |
| P0 | 虚拟滚动 | Review #2 | 2-3 天 |
| P0 | 人类消息视觉区分 + 代码高亮 | UX 底线标准 | 1.5 天 |
| P1 | watcher.rs 事件 -> Tauri event -> 前端增量更新 | desktop 原型 | 2 天 |
| P1 | daemon 生命周期管理（auto-start） | desktop 原型 | 1 天 |
| P2 | 系统托盘 + 通知 | desktop 原型 | 2 天 |
| P2 | 离线消息阅读 | desktop 优势 | 2 天 |

总工期估算：**4-5 周**（1 人全职）。其中前 2 周完成前端迁移和核心功能，后 2-3 周完成系统集成。

#### 评估

| 维度 | 分数 | 说明 |
|------|------|------|
| AI 对话可读性 | 4/5 | 同方案一，共享组件 |
| 线程结构表达 | 4/5 | 同方案一，共享组件 |
| 场景覆盖度 | 4/5 | 系统通知 + 离线 = 更好的观察和审计体验 |
| 开发成本 | 3/5 | 需要 React->Svelte 迁移 + Rust 后端调试 |
| 学习成本 | 4/5 | 需要安装桌面应用（但安装后体验更好） |
| 性能 | 5/5 | notify 实时事件 + 本地解析，几乎零延迟 |
| 扩展性 | 5/5 | Rust 生态协同，未来可直接集成 daemon 新功能 |

**加权总分：3.95/5**

---

### 方案三："CLI+TUI"（最轻量，Agent 原生）

**一句话**：增强现有 CLI + Ink TUI 交互模式，面向开发者和 Agent 重度用户。

#### 技术栈

| 层 | 技术 | 来源 |
|----|------|------|
| TUI 框架 | Ink (React for CLI) | tui 原型 |
| CLI | Commander.js（现有） | 已有 cli/ |
| daemon 连接 | 复用 client.ts | 现有 |
| Markdown | marked-terminal | 新增 |
| 代码高亮 | cli-highlight 或 shiki-cli | Review #3 修复 |

#### 架构

```
gitim tui        (Ink 长驻进程)
gitim send/read  (Commander.js 一次性命令)
    |
    | Unix Socket (行分隔 JSON) -- 复用 client.ts
    |
gitim-daemon (Rust)
    |
.thread 文件 + Git
```

#### 布局（修复后）

```
+------------------------------------------------------------+
| GitIM TUI v0.1              [lewis] * daemon connected     |
+----------+-----------------------------+-------------------+
| CHANNELS |       #general              |   Thread L000003  |
|          |                             |                   |
| #general | * nexus  12:00             |   * nexus  12:00  |
|  dev     | 讨论部署方案                |   > 讨论部署方案  |
|  ops     |                             |                   |
|          |   * lewis  12:05           |   o lewis  12:05  |
| -------- |   > 收到                    |   收到            |
| DMs      |                             |                   |
|  nexus   |   * coder  12:10          |   * coder  12:10  |
|  coder   |   > 我也看看                |   我也看看        |
|          |   : 这条消息也有第二行      |                   |
|          |   : 还有第三行              |                   |
|          |                             |                   |
|          | | HUMAN  lewis  12:35     | |                   |
|          | | 先不要动 main.rs         | |                   |
|          |                             |                   |
|          |   * nexus  12:15           |                   |
|          |   >> 具体看一下 K8s 配置    |                   |
|          |                             |                   |
|          | * coder  12:30             |                   |
|          | ```rust                      |                   |
|          | fn main() {                 |                   |
|          |     let config = load();    |                   |
|          | }                            |                   |
|          | ```                          |                   |
|          | [... 展开全部 (23行)]        |                   |
+----------+-----------------------------+-------------------+
| > 输入消息... (Tab:面板 r:回复 t:线程 f:折叠AI q:退出)    |
+------------------------------------------------------------+

* = AI Agent     o = 人类用户
|...|  = 人类消息高亮边框
```

关键修复（对标 Review #3 和底线标准）：
- `*`/`o` 符号区分 AI 和人类用户
- 人类消息用 `|` 边框包围 + 粗体 `HUMAN` 标签
- 代码块用 cli-highlight 做终端内语法高亮
- 连续 AI 消息超过 5 条自动折叠，显示 `[5 条 AI 对话已折叠，按 e 展开]`
- 快捷键 `f` 切换"仅看人类消息"模式

#### 从现有原型到 v1 的修复清单

| 优先级 | 修复项 | 来源 | 工作量 |
|--------|--------|------|--------|
| P0 | 人类/AI 消息样式区分 | Review #3 底线 | 1 天 |
| P0 | 代码块终端语法高亮 | Review #3 底线 | 1-2 天 |
| P1 | AI 消息连续折叠 | Review #3 | 1 天 |
| P1 | 仅看人类消息过滤模式 | UX 设计 | 0.5 天 |
| P1 | Git 同步状态显示 | UX 底线 | 0.5 天 |
| P2 | 接入真实 daemon（替换 mock） | tui 演进 | 1 天 |

总工期估算：**1-2 周**（1 人全职）。

#### 评估

| 维度 | 分数 | 说明 |
|------|------|------|
| AI 对话可读性 | 3/5 | 终端宽度有限，长消息体验受限 |
| 线程结构表达 | 3/5 | > >> 缩进 + 线程面板，够用但不直观 |
| 场景覆盖度 | 2/5 | 观察/指挥可以；协作/审计/调试较弱 |
| 开发成本 | 5/5 | 修复量最小，与现有 CLI 共享代码 |
| 学习成本 | 3/5 | 需要记快捷键，非技术用户门槛高 |
| 性能 | 5/5 | 终端渲染无性能瓶颈 |
| 扩展性 | 3/5 | Ink 布局能力有限，复杂 UI 难以表达 |

**加权总分：3.45/5**

---

## 3. 方案对比总览

```
                AI可读  线程  场景  成本  学习  性能  扩展  总分
                (x.25) (x.15)(x.15)(x.20)(x.10)(x.10)(x.05)
Web First       4      4     3     5     5     3     4     4.00  <-- 推荐
Tauri Hybrid    4      4     4     3     4     5     5     3.95
CLI+TUI         3      3     2     5     3     5     3     3.45
```

### 方案互补性分析

三套方案并非互斥，而是可以分层组合：

```
                   用户群
                     |
          +----------+----------+
          |          |          |
       管理者     开发者     Agent
          |          |          |
       Web UI     TUI/Web   CLI (已有)
          |          |          |
          +----+-----+          |
               |                |
          共享数据层 (@gitim/thread-utils)
               |                |
               +-------+-------+
                       |
                 WebSocket Bridge
                       |
                   daemon API
```

**推荐分期路线**：

| 阶段 | 交付 | 工期 | 用户覆盖 |
|------|------|------|----------|
| v1.0 | Web First（方案一） | 2-3 周 | 所有人类用户 |
| v1.1 | CLI+TUI 修复（方案三） | 并行 1-2 周 | 开发者用户 |
| v2.0 | Tauri Hybrid（方案二） | v1 后 4-5 周 | 重度用户 |

---

## 4. 共享组件与架构约定

无论采用哪套方案，以下架构约定应统一遵守：

### 4.1 共享包：@gitim/thread-utils

从 web-frontend 的 mock.js 抽取，所有前端共用：

```typescript
// 核心数据结构（对齐 daemon API 返回格式）
interface Message {
  line_number: number;
  point_to: number;
  author: string;
  timestamp: string;  // "YYYYMMDDTHHmmssZ"
  body: string;
}

// 线程操作
function buildThreadTree(messages: Message[]): { roots: TreeNode[], map: Map<number, TreeNode> }
function getThreadChain(messages: Message[], lineNumber: number): Message[]
function getThreadReplies(messages: Message[], lineNumber: number): Message[]

// 工具函数
function formatTimestamp(ts: string): string
function formatFullTimestamp(ts: string): string
function isHumanUser(author: string, users: UserMeta[]): boolean
function getAgentColor(author: string): string
```

### 4.2 Bridge 协议规范

subscribe 命令增加 since 参数，区分 history 和 live：

```json
// 订阅频道（含历史回填）
{ "action": "subscribe", "channel": "general", "since": 0 }

// 服务端响应：先发 history，再发 live
{ "type": "history", "channel": "general", "messages": [...] }
{ "type": "messages", "channel": "general", "messages": [...] }
```

每个客户端维护独立 cursor（不再共享全局 channelCursors）。

### 4.3 人类/AI 消息区分规则

由于 UserMeta 没有 `is_human` 字段，约定：
- 在 `users/<handler>.meta.json` 的 `role` 字段中，`founder`、`pm`、`manager` 等视为人类
- 或者未来在 UserMeta 中增加可选的 `type: "human" | "agent"` 字段
- 前端降级方案：维护一个配置文件列出人类 handler

---

## 5. 最终推荐

**v1 阶段推荐方案一 "Web First"。**

理由：
1. **开发成本最低**——原型完成度最高，修复清单明确且可控（2-3 周）
2. **用户覆盖最广**——浏览器访问，零安装，管理者和开发者都能用
3. **向上兼容**——Svelte 组件可直接复用于 Tauri（方案二的前端层）
4. **底线标准全部可满足**——虚拟滚动、XSS、人类消息、代码高亮均有明确修复路径

方案三（CLI+TUI）可并行推进，作为开发者的补充工具。
方案二（Tauri）作为 v2 方向，在 Web 版稳定后启动，复用 90% 以上的 Svelte 组件。
