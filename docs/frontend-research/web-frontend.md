# GitIM Web 前端选型报告

## 选型对比

### 1. React + IM 组件库（如 stream-chat-react 模式）

**优势：**
- 生态成熟，stream-chat-react、ChatScope 等提供现成的消息列表、线程面板、输入框组件
- 虚拟滚动方案成熟（react-virtuoso、react-window）
- 社区资源丰富，招聘容易

**劣势：**
- stream-chat-react 深度绑定 Stream 后端 SDK，不适合自定义协议
- 重量级：React 本身 ~40KB gzipped，加上状态管理（zustand/redux）和 UI 库
- 过度抽象：GitIM 的 `point_to` 线程模型与 stream-chat 的 thread 模型不兼容，需要大量适配
- 对 AI 长消息的 Markdown 渲染需额外引入 react-markdown + rehype 套件

**适用场景：** 团队已有 React 经验、需要快速接入成熟 IM UI、消息模型与主流 IM 兼容时。

### 2. Vue 3 + 轻量方案

**优势：**
- 渐进式框架，组合式 API（Composition API）灵活度高
- 模板语法直观，适合快速原型
- vue-virtual-scroller 支持虚拟滚动
- 单文件组件（SFC）开发体验好

**劣势：**
- IM 领域专用组件库较少，基本需要从零构建
- 包体积中等（~30KB gzipped）
- TypeScript 支持在模板中仍有不足

**适用场景：** 团队偏好 Vue 生态，或需要与现有 Vue 项目集成时。

### 3. Svelte/SvelteKit 极简方案（推荐）

**优势：**
- 编译时框架，运行时几乎零开销，打包体积极小
- 响应式语法天然简洁，无需 useState/ref 等样板代码
- 适合自定义 UI：没有框架级别的组件约束，完全掌控渲染逻辑
- 对 GitIM 的 `point_to` 树形结构，可以用简洁的递归组件或平铺+缩进实现
- SvelteKit 提供开箱即用的路由、SSR、开发服务器

**劣势：**
- 生态相对较小，虚拟滚动需要手写或用 svelte-virtual-list
- 社区组件库不如 React 丰富
- 团队可能需要学习曲线

**适用场景：** 追求极简、高性能、完全定制 UI，且团队愿意自建组件。

### 4. Vanilla JS + Web Components

**优势：**
- 零依赖，完全掌控
- 最小包体积
- 无框架锁定

**劣势：**
- 手动管理 DOM 更新，开发效率低
- 状态管理需从零实现
- 代码量大，维护困难

**适用场景：** 嵌入式场景、对体积有极端要求时。

## 推荐方案：Svelte 5 + Vite

### 决策理由

1. **GitIM 的核心 UX 挑战是 `point_to` 线程可视化**，这需要完全自定义的 UI，成熟 IM 组件库在这里反而是约束
2. **AI Agent 消息的特殊性**（长文本、高频、多 Agent）需要定制化的渲染策略，框架越轻越好
3. **原型阶段追求快速迭代**，Svelte 的简洁语法能最大化开发效率
4. **未来可无缝迁移到 SvelteKit** 获得 SSR、路由等能力

### 线程可视化方案

经过对比三种方案：

| 方案 | 描述 | 优势 | 劣势 |
|------|------|------|------|
| 平铺 + 缩进 | 所有消息按时间排列，回复消息缩进显示 | 保持时间线完整 | 深层线程难以追踪 |
| 嵌套树 | 类 Reddit 风格，递归嵌套 | 线程关系清晰 | 屏幕空间浪费严重 |
| **分栏（推荐）** | 主列显示顶层消息，点击后右侧展开线程面板 | 兼顾概览与详情 | 需要额外交互 |

**原型采用「分栏」方案**，主列展示时间线（带缩进提示回复关系），右侧可展开完整线程视图。

### AI 消息 UX 设计要点

1. **Agent 身份色带**：每个 Agent 分配独特颜色，消息左侧显示色带 + 头像区
2. **长消息折叠**：超过 5 行的消息默认折叠，显示前 3 行 + "展开" 按钮
3. **代码块高亮**：内联 Markdown 渲染，代码块带语法高亮
4. **消息密度控制**：紧凑模式 vs 舒适模式切换
5. **Agent 过滤器**：按 Agent 筛选消息

## 原型说明

- 技术栈：Svelte 5 + Vite + TypeScript
- 启动方式：`npm install && npm run dev`
- Mock 数据模拟多个 AI Agent 的对话场景
- 实现功能：频道列表、消息列表、发送消息、线程面板、消息折叠/展开

## 未来演进

1. 接入真实 daemon HTTP API（替换 mock）
2. WebSocket/SSE 实时推送（daemon 扩展）
3. 虚拟滚动（消息量 > 1000 时）
4. Markdown 渲染 + 代码高亮（marked + highlight.js）
5. 响应式布局适配移动端
