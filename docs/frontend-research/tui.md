# GitIM TUI 选型报告与原型说明

## 1. 框架对比

### 1.1 ratatui (Rust)

| 维度 | 评价 |
|------|------|
| 语言 | Rust — 与 daemon 同语言 |
| 优势 | 可直接调用 daemon 内部库，无需 socket 通信；性能极佳；单二进制分发 |
| 劣势 | 即时模式渲染（immediate mode），UI 开发心智负担重；Rust 编译慢；团队需要 Rust UI 经验 |
| 终端兼容 | crossterm 后端支持 256 色 / true color / 鼠标，SSH 和 tmux 兼容好 |
| 启动速度 | < 10ms |
| 适合场景 | daemon 深度集成、高性能需求、长期方案 |

### 1.2 Ink (React for CLI) — **推荐**

| 维度 | 评价 |
|------|------|
| 语言 | TypeScript — 与现有 CLI 同生态 |
| 优势 | React 组件模型，声明式 UI；npm 生态丰富；与现有 cli/ 代码可直接复用（client.ts）；开发速度快 |
| 劣势 | 布局能力不如 ratatui 灵活（Flexbox only）；不支持鼠标；依赖 Node.js 运行时 |
| 终端兼容 | 基于 yoga 布局引擎 + 自定义渲染器，256 色 / true color / Unicode 支持好 |
| 启动速度 | ~200ms（Node.js 冷启动） |
| 适合场景 | 快速原型、与现有 TS CLI 共存、前端团队友好 |

### 1.3 Textual (Python)

| 维度 | 评价 |
|------|------|
| 语言 | Python |
| 优势 | 内置丰富组件（DataTable, Tree, Markdown）；CSS-like 样式；开发最快 |
| 劣势 | 引入新语言栈；Python 运行时依赖；与现有代码无复用可能 |
| 终端兼容 | 优秀，支持鼠标、true color |
| 启动速度 | ~300ms |
| 适合场景 | 独立工具、快速验证 UI 设计 |

### 1.4 blessed / blessed-contrib (Node.js)

| 维度 | 评价 |
|------|------|
| 语言 | JavaScript |
| 优势 | 经典方案，功能全面（分屏、滚动、表单、图表） |
| 劣势 | 项目已停止维护（最后更新 2020）；API 老旧，命令式风格；类型定义不完整 |
| 终端兼容 | 较好，但 Unicode 宽字符偶尔有问题 |
| 启动速度 | ~150ms |
| 适合场景 | 不推荐用于新项目 |

### 1.5 bubbletea (Go)

| 维度 | 评价 |
|------|------|
| 语言 | Go |
| 优势 | Elm 架构，状态管理清晰；单二进制；lipgloss 样式库美观；社区活跃 |
| 劣势 | 引入新语言栈；与 Rust daemon / TS CLI 无代码复用 |
| 终端兼容 | 优秀 |
| 启动速度 | < 20ms |
| 适合场景 | 独立 TUI 工具 |

## 2. 选型结论：Ink (React for CLI)

**理由：**

1. **生态复用**：与现有 `cli/` 同为 TypeScript，可直接复用 `client.ts`（socket 通信）、Commander.js 命令体系
2. **学习成本低**：React 组件模型，声明式 UI，前端开发者零学习曲线
3. **与 CLI 共存方案清晰**：作为 `gitim tui` 子命令，复用同一 npm 包
4. **原型开发快**：JSX 写 UI，hooks 管理状态，minutes-to-prototype
5. **长期路径**：如果性能不够，可以后期迁移到 ratatui，但 UI 设计和交互模式可以先用 Ink 验证

**权衡：**
- 不支持鼠标是 Ink 的主要短板，但 GitIM 的用户是开发者/Agent，键盘操作更自然
- Node.js 启动开销（~200ms）可接受——TUI 是长驻进程，启动只有一次

## 3. TUI 布局设计

```
┌────────────────────────────────────────────────────────────┐
│  GitIM TUI v0.1              [lewis] ● daemon connected    │
├──────────┬─────────────────────────────┬───────────────────┤
│ CHANNELS │       #general              │   Thread L000003  │
│          │                             │                   │
│ #general │ nexus  12:00                │   nexus  12:00    │
│  dev     │ 大家好，今天讨论部署方案    │   > 讨论部署方案  │
│  ops     │                             │                   │
│          │ lewis  12:05                │   lewis  12:05    │
│ ──────── │ > 收到                      │   收到            │
│ DMs      │                             │                   │
│  nexus   │ coder  12:10                │   coder  12:10    │
│  coder   │ > 我也看看                  │   我也看看        │
│          │ 这条消息也有第二行          │                   │
│          │ 还有第三行                  │                   │
│          │                             │                   │
│          │ nexus  12:15                │                   │
│          │ >> 具体看一下 K8s 配置      │                   │
│          │                             │                   │
├──────────┴─────────────────────────────┴───────────────────┤
│ > 输入消息... (Tab:切换面板 r:回复 t:线程 q:退出)         │
└────────────────────────────────────────────────────────────┘
```

### 3.1 线程展示策略

- **主消息区**：`P000000` 的消息正常显示；回复消息前加 `>` 前缀和缩进
- **嵌套回复**：`>>` 表示回复的回复（最多显示 3 层缩进，之后用 `>>>` 折叠）
- **线程面板**：按 `t` 键打开右侧面板，展示选中消息的完整线程链
- **续行**：多行消息直接连续显示，不加额外前缀

### 3.2 键盘快捷键

| 快捷键 | 功能 |
|--------|------|
| `Tab` | 在频道列表 / 消息区 / 线程面板之间切换焦点 |
| `j` / `k` | 上下滚动 |
| `Enter` | 进入频道 / 开始输入 |
| `r` | 回复选中消息 |
| `t` | 查看选中消息的线程 |
| `Esc` | 取消 / 关闭面板 |
| `q` / `Ctrl+C` | 退出 |
| `/` | 搜索（预留） |

### 3.3 长消息与代码块

- 终端宽度自动换行
- 代码块（被 ``` 包裹）用不同背景色高亮显示
- 超长消息截断显示，按 Enter 展开

## 4. 与现有 CLI 的关系

**推荐方案：子命令共存**

```bash
gitim send ...       # 现有命令式操作（保留）
gitim read ...       # 现有命令式操作（保留）
gitim tui            # 启动交互式 TUI（新增）
gitim tui --channel general  # 直接进入某频道
```

- CLI 命令适合脚本化、Agent 调用、CI/CD 管道
- TUI 适合人类开发者日常使用、实时监控
- 两者共享同一 daemon 连接逻辑（`client.ts`）

## 5. 原型说明

### 5.1 文件结构

```
frontend-research/tui/
├── CLAUDE.md          # 本文件
├── package.json
├── tsconfig.json
├── src/
│   ├── app.tsx        # 主应用组件
│   ├── index.tsx      # 入口
│   ├── mock.ts        # Mock daemon 数据
│   ├── components/
│   │   ├── channel-list.tsx   # 频道列表面板
│   │   ├── message-view.tsx   # 消息展示区
│   │   ├── thread-panel.tsx   # 线程详情面板
│   │   ├── input-bar.tsx      # 输入栏
│   │   └── status-bar.tsx     # 状态栏
│   └── hooks/
│       └── use-store.ts       # 全局状态管理
```

### 5.2 启动方式

```bash
cd frontend-research/tui
npm install
npm run dev
```

### 5.3 操作说明

- `Tab` 切换焦点面板（频道列表 → 消息区 → 输入栏）
- 在频道列表中用 `j/k` 上下选择，`Enter` 进入
- 在消息区用 `j/k` 滚动，`r` 回复，`t` 查看线程
- 在输入栏直接打字，`Enter` 发送
- `q` 退出（非输入模式下）

### 5.4 Mock 模式

原型使用内置 mock 数据模拟 daemon 响应，包含：
- 3 个频道（general, dev, ops）
- 4 个用户（nexus, lewis, coder, ops-bot）
- 多条消息，含线程回复和多行消息

接入真实 daemon 只需将 mock 替换为 `GitimClient` 调用。

## 6. 后续演进

| 阶段 | 内容 |
|------|------|
| v0.1（当前） | Ink 原型，mock 数据，基本布局 |
| v0.2 | 接入真实 daemon（Unix socket），实时消息更新 |
| v0.3 | DM 支持，用户列表，在线状态 |
| v1.0 | 如果 Ink 性能瓶颈明显，考虑迁移到 ratatui |
