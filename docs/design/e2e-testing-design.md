# GitIM E2E Testing Framework — Design Document

> 2026-04-12. 基于 grill-me 讨论确定的 9 项设计决策。

## 目标

构建一套 E2E 自动化测试框架，在前端 UI 打磨和后端 Runtime API 构建过程中同步推进，确保迭代不破坏已有功能。

## 架构

```
                         Playwright Tests
                              │
                         WebV2 (React)
                              │
                      IMClient Interface
                       ╱              ╲
                 MockClient        RealClient (HTTP)
                 (Phase 1)            │ (Phase 2)
                                      │
                              Runtime HTTP 网关
                              ╱              ╲
                     Agent 管理 API       Daemon 代理
                     (CRUD+启停+状态)     (IM 能力)
                           │                  │
                  N × Agent Slot      1 × Human Slot
                  (mock provider)     (无 agent，供 WebV2 接入)
                           ╲                ╱
                        共享 remote git repo
```

## 设计决策

### 1. E2E 边界：全链路，只 mock LLM provider

测试覆盖从浏览器到 git repo 的完整链路。唯一被 mock 的是 LLM 的实际调用（Claude CLI 等）。Runtime 本身是重量级的管理层（HTTP 接口、agent 编排、工作空间协调），不可跳过。

### 2. 后端接入点：Runtime 作为统一 HTTP 网关

WebV2 只连一个端点。Runtime 同时暴露：
- **Agent 管理 API**：CRUD、启停、运行时状态
- **Daemon 代理**：IM 能力（消息收发、通道管理、用户列表）

IM 能力只暴露"人类频道"——一个不运行 agent 的特殊 slot，仅为人类提供接入工作流的入口。人类可以通过它查看所有 agent 间的交流并参与对话。

N 个 agent slot 各自有独立的 gitim clone + daemon + agent_loop，不暴露 HTTP。

### 3. 推进节奏：mock 先行，API 逐个替换

- Phase 1：用 Playwright 测 mock 前端，兜住 UI 回归
- Phase 2：每实现一个 Runtime HTTP API，就同步补一个 E2E 测试
- 同一套 Playwright 测试，通过环境切换，自动从 mock 变为真正的 E2E

### 4. 前端切换机制：IMClient interface + 环境变量

前端定义统一的 `IMClient` interface，mock 和 real client 都实现它：

```typescript
interface IMClient {
  me(): Promise<User>
  poll(since?: string): Promise<PollResult>
  channels(): Promise<Channel[]>
  send(channel: string, body: string, pointTo?: number): Promise<Message>
  read(channel: string, opts?: ReadOpts): Promise<Message[]>
  thread(channel: string, lineNumber: number): Promise<Message[]>
  users(): Promise<User[]>

  // Agent management
  listAgents(): Promise<Agent[]>
  getAgent(id: string): Promise<Agent>
  addAgent(config: AgentConfig): Promise<Agent>
  removeAgent(id: string): Promise<void>
  startAgent(id: string): Promise<void>
  stopAgent(id: string): Promise<void>
}
```

通过 `VITE_BACKEND=mock|real` 环境变量决定运行时使用哪个实现。Playwright 测试本身完全不变——它只操作浏览器 DOM，不关心数据来源。

### 5. 测试环境生命周期：共享环境，channel 隔离

Phase 2 的真实后端测试中：
- `globalSetup` 启动一次完整环境（git repo + daemon + runtime + webui dev server）
- 每个测试文件在 `beforeAll` 中创建独立 channel（`test-xxx-{timestamp}`）做数据隔离
- `globalTeardown` 统一销毁
- 少数需要干净环境的测试（如 onboard 流程）标记为独立运行

### 6. Mock Provider：固定回复，预留可编程扩展

Mock provider 实现 `Provider` trait，默认返回固定文本。内部持有可选的 response 队列：

```rust
pub struct MockProvider {
    default_response: String,
    queue: Arc<Mutex<VecDeque<MockResponse>>>,
}
```

- 队列为空时返回固定回复，足以验证全链路消息传递
- 测试需要特定 agent 行为时，预设 response 队列按序消费
- 不做关键词匹配，避免脆弱的匹配逻辑

### 7. 断言粒度：行为断言为主

测试断言用户操作后的结果状态，不断言 CSS、像素或 snapshot：

```typescript
// 好 — 行为断言
await page.fill('[data-testid="message-input"]', 'hello');
await page.click('[data-testid="send-button"]');
await expect(page.locator('.message-list')).toContainText('hello');

// 不做 — snapshot 断言（前端打磨期变化太频繁）
// await expect(page).toHaveScreenshot();
```

前端 UI 趋于稳定后，可以对个别关键状态（空状态、错误状态）补充少量 snapshot。

### 8. Agent API 范围

第一优先级：
- CRUD：`addAgent`, `removeAgent`, `getAgent`, `listAgents`
- 启停：`startAgent`, `stopAgent`
- 运行时状态：当前轮次、最近执行结果、错误信息

第二优先级（后续扩展）：
- 配置编辑：system prompt、model 选择、max_turns
- 跨 agent 可观测性

### 9. 目录结构：顶层独立 `/e2e/`

```
e2e/
├── package.json          # playwright + ts 依赖
├── playwright.config.ts  # 两套 project: mock / real
├── fixtures/
│   ├── setup-mock.ts     # 启动 webui-v2 dev server (mock mode)
│   └── setup-real.ts     # 启动 git repo + daemon + runtime + webui-v2 (real mode)
├── tests/
│   ├── messaging.spec.ts
│   ├── channels.spec.ts
│   ├── threads.spec.ts
│   └── agents.spec.ts
└── helpers/
    └── ...
```

E2E 测试跨前后端，不属于任何单一 crate 或 webui-v2，放顶层准确反映职责。独立 `package.json` 避免污染 webui-v2 的依赖。

## 实施顺序

### Phase 1 — Mock 基线

可以立刻开始，不依赖 Runtime HTTP API。

1. **前端提取 IMClient interface**：从当前 mock client 抽出 interface，mock client 实现它
2. **搭建 `/e2e/`**：Playwright 配置、mock 启动 fixture
3. **写核心行为测试**：
   - 消息收发（发送后出现在列表）
   - Channel 切换（切换后消息列表刷新）
   - Thread 交互（点击消息打开 thread panel）
   - Agent 列表与状态展示
   - Agent 启停操作
4. **持续保护**：前端打磨细节时，测试捕获回归

### Phase 2 — 逐步接真实后端

每实现一个 API 就补一个测试。

1. **Runtime HTTP server**：Axum，暴露 agent 管理 + daemon 代理
2. **RealClient 实现**：逐个方法对接 Runtime HTTP API
3. **Mock Provider 接入**：挂到 agent loop，固定回复验证全链路
4. **Real 环境 fixture**：编排 git repo + daemon + runtime + webui 的完整启动/销毁
5. **Playwright `real` project**：在 `playwright.config.ts` 中启用，跑同一套测试
