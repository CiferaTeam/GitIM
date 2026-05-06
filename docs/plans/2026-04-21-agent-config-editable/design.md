# Agent Config: 显示修复 + 可编辑化

**Status**: Design approved, ready for planning
**Date**: 2026-04-21
**Scope**: webui-v2 + gitim-runtime

## Problem

用户在 WebUI 管理 Agent 时遇到三个问题：

1. **前端没有 provider 提示**。detail 页只显示 Model，不显示 Provider；card 列表页 provider / model 都不显示。用户看不出 agent 跑的是 claude / codex / opencode。
2. **模型错误回退到 Sonnet 默认值**。[`agent-detail.tsx:131`](../../webui-v2/src/components/management/agent-detail.tsx#L131) 硬编码 `{agent.model ?? "claude-sonnet-4-6"}`。opencode agent 的 `model` 是 `null`（它自己从 `opencode auth login` 读默认），前端就谎称它在跑 Sonnet。
3. **System prompt / env 创建后不可改**。runtime 没有 PATCH 端点；me.json 里 `system_prompt` / `env` 只在 `add_agent` 时写一次。想改只能删了重建。

## Scope

**In**:
- 前端：detail 页加 Provider 显示、修掉 Sonnet fallback、加 Edit 模式（编辑 system_prompt / env / 新增 `.env` secrets 文件）；card 列表页加 provider + model 小字。
- 后端：新增 `PATCH /workspaces/{slug}/agents/{id}`；新增 `.env` 文件落盘到 `<agent-clone>/.env`；workspace `/git/init` 阶段在仓库 `.gitignore` 追加 `.env` 规则。

**Out (本轮不做)**:
- 修改 `provider`：需要重新 preflight + session 迁移，复杂度另起一个 feature。
- 修改 `model`：同 provider 换 model 逻辑简单，但会打乱 session_token / context_window 语义，延后。
- `.env` 已被 git tracked 的清理：只对未 tracked 的生效，历史污染交用户处理。
- OAuth / token rotate 相关改动。

## Architecture

### 后端：PATCH Agent 端点

**路由**：`PATCH /workspaces/{slug}/agents/{id}`（加到 `crates/gitim-runtime/src/http.rs` 现有 agent routes 旁）

**Body**（所有字段可选，字段缺省 = 不动该字段；传空值 = 清空该字段）：
```json
{
  "system_prompt": "string | null",
  "env": { "KEY": "VALUE" },
  "dotenv": "string"
}
```

**字段语义**：
- `system_prompt` 缺省 → 不动 me.json 里的 `system_prompt`；传 `null` 或 `""` → 删除 me.json 里该字段
- `env` 缺省 → 不动；传 `{}` → 清空所有 env vars（整体替换语义，不是 merge）；传 `{"FOO": "bar"}` → 整体替换为这一份
- `dotenv` 缺省 → 不动 `.env` 文件；传 `""` → 删除 `.env`；传非空 → 整体覆盖写

**行为序列**：
1. 拿 `AppState.agents` 的 Mutex → 查 agent，不存在返回 404
2. 读 `<agent-clone>/.gitim/me.json`，对传入字段做 **merge**（沿用现有 `write_me_json` 的 merge 语义，避免抹掉 `github_email` 等字段），写回
3. 如果 body 含 `dotenv`：
   - 非空字符串 → 写 `<agent-clone>/.env`，chmod 0600
   - 空字符串 → 删除 `<agent-clone>/.env`（不存在时 no-op）
4. 更新 `AppState.agents[id]` 内存副本的 `system_prompt` / `env` 字段
5. 返回更新后的 `AgentInfo` JSON

**校验**：
- `env` key 必须是合法 env var 名（`[A-Z_][A-Z0-9_]*` 宽松匹配），非法返回 400
- `dotenv` 总大小 ≤ 64 KB，超出返回 400
- 不校验 `dotenv` 内容语法（允许注释、`export` 前缀等 dotenv 方言）

**并发**：
- 现有 `AppState.agents` 是 `Arc<Mutex<HashMap<String, AgentInfo>>>`，PATCH 持锁时间短（只做 me.json I/O + 内存更新）
- 即使 agent 正在 poll / 处理消息，也不 block；生效语义由 spawn 节奏决定（见下）
- me.json 写入沿用 `.gitim/` 目录（已被 gitignored），不走 git sync

### 后端：`.env` 文件 + `.gitignore` 管理

**`.env` 文件路径**：`<agent-clone>/.env`
- 每个 agent 一份，不跨 agent 共享（因为每个 agent 是独立 clone）
- Agent CLI 运行时 cwd = clone 根，能自然 `cat .env` / `source .env` / dotenv 库读取
- 不被 runtime 注入到进程 env（跟 `env` 字段语义刻意区分，文件是给 agent 自己用的，`env` 是给 CLI 进程用的）

**`.gitignore` 追加**：
- 触发点：workspace `/git/init` 阶段的 human clone 初始化（`crates/gitim-runtime/src/workspace.rs` 里 clone 完成后、push 前）
- 逻辑：
  1. 读 clone 根的 `.gitignore`（不存在则新建）
  2. 扫描是否已匹配 `.env`（检查 `.env`、`/.env`、`.env*` 三种写法的字面存在）
  3. 未匹配则追加 `.env\n` 一行 + 一次 commit（作者沿用 workspace owner 的 `github_email` 模式）
  4. 已匹配则 no-op
- 幂等：第二次 init 不重复追加、不重复 commit
- 适用两种 workspace provider：local 和 github

### 前端：Display 修复

**改 `webui-v2/src/components/management/agent-detail.tsx`**：

加 Provider 字段到 info grid 最前面：
```tsx
<Field label="Provider">
  <ProviderBadge provider={agent.provider} />
</Field>
```

`<ProviderBadge>` 新组件（放 `webui-v2/src/components/management/provider-badge.tsx`）：
- claude → 橙色
- codex → 紫色
- opencode → 绿色
- undefined → 灰色 "—"

Model 字段改为：
```tsx
<Field label="Model">
  {agent.model ? (
    <span className="font-mono">{agent.model}</span>
  ) : agent.provider === "opencode" ? (
    <span className="text-text-muted italic">Default (opencode auth login)</span>
  ) : (
    <span className="text-text-muted">—</span>
  )}
</Field>
```

**改 `webui-v2/src/components/management/agent-card.tsx`**：

在 CardHeader 的 name 下方加一行 provider + model 小字（灰色，不抢 status badge）：
```tsx
<span className="text-xs text-text-muted">
  {agent.provider ?? "—"} · {agent.model ?? (agent.provider === "opencode" ? "default" : "—")}
</span>
```

### 前端：Edit 模式

**状态机** —— detail 页新增 `mode: "view" | "edit" | "saving"` local state：

```
[view]  --Edit btn-->  [edit]  --Save-->  [saving]  --200-->  [view + toast]
                         │                    │
                         └--Cancel------------┘
                                              └--error-->  [edit + banner]
```

**可编辑字段**：
1. **System Prompt**（`<Textarea>`，4 行起自动撑高）
2. **Environment Variables**（抽共享组件 `<EnvVarsEditor>`，AddAgentDialog 和 AgentDetail 都用）
3. **Secrets (.env file)**（新增 `<Textarea>` monospace，8 行起）

**抽共享组件 `<EnvVarsEditor>`**：
- 位置：`webui-v2/src/components/management/env-vars-editor.tsx`
- Props：`{ value: {key, value}[]; onChange: (v) => void }`
- AddAgentDialog 现在的 inline env KV UI 迁移进去
- Edit 模式和创建模式共用，减少重复

**字段说明文案（inline 贴在 label 下方，`text-xs text-text-muted`）**：
- Environment Variables: "Injected as process env vars to the agent CLI. Flat key-value."
- Secrets (.env file): "Written to `<agent-clone>/.env` (gitignored). Agent can read it via `source .env`, dotenv libraries, or `cat` at runtime. Use for API keys and multi-line secrets."

**Save 成功提示（toast 或 inline banner）**：

根据实际改动字段，按下列规则拼接：

```
✓ Saved.
• Environment & .env → take effect on next message
• System prompt     → takes effect on next session (auto-rolls when current session fills)
```

只显示对应行：没改 system prompt 就不显示那行。env 和 dotenv 合并成一行提示（都是 next message 生效）。

**Cancel**：
- 回滚到最后一次从后端拉到的值
- 如果有 unsaved changes，弹二次确认

**离开页面守卫**：
- 有 unsaved changes 时，`beforeunload` 事件拦截 + react-router 路由守卫
- 保持跟 Cancel 一致的二次确认

**允许运行中编辑**：不 block。生效语义：
- `env` / `dotenv` → 下次 spawn CLI 子进程（下一条消息）生效
- `system_prompt` → 下一个 session 生效（当前 session 生命周期内不变，session 满后自动滚）

### 前端：API 调用

`webui-v2/src/lib/client.ts` 新增：
```ts
export async function updateAgent(
  slug: string,
  agentId: string,
  patch: {
    system_prompt?: string | null;
    env?: Record<string, string>;
    dotenv?: string;
  },
): Promise<ApiResponse<{ agent: Agent }>>;
```

`useAgentStore` 新增 `updateAgent` action（已有，复用）。

## Generation Semantics（跨组件共识）

这块在 design 里显式写出来，避免后续实现时理解偏差：

| 字段 | 写入位置 | 何时对 agent 生效 |
|---|---|---|
| `system_prompt` | `me.json` | **下一个新 session**（当前 session 满后框架自动滚到新 session 才注入） |
| `env` | `me.json` | **下一次 spawn CLI 子进程**（即下一条消息，每条消息都重启子进程） |
| `dotenv` (`.env` file) | `<clone>/.env` on disk | 取决于 agent 怎么读：`source .env` / dotenv 库 / `cat` —— 最常见是下次 spawn CLI 子进程 |

`provider` 和 `model` 本轮不允许改——会打乱 session_token 和 context_window 状态，需要单独 feature。

## Error Handling

**后端**：
- 404: agent id 不存在
- 400: env key 非法 / dotenv 超过 64 KB
- 500: me.json 读写失败 / `.env` 文件写失败 —— 回滚（me.json 已写就保留，`.env` 失败不 rollback me.json，但返回 500 让用户知道部分失败）

**前端**：
- 网络错误 → 保持 edit 模式 + inline banner 显示 error
- 400 → 同上，error 文案从 API response `error` 字段取

## Testing

**后端**：
- `crates/gitim-runtime/tests/` 新增集成测试：
  - PATCH 只改 system_prompt → me.json 正确 merge
  - PATCH 带 dotenv → 文件写入 + chmod 0600
  - PATCH dotenv 空字符串 → 文件删除
  - PATCH 不存在的 agent → 404
  - PATCH env 非法 key → 400
  - `/git/init` 写入 `.gitignore` → 幂等，第二次 no-op
  - `/git/init` 已有 `.env` 规则 → skip
- me.json merge 语义测试已有，复用

**前端**：
- AgentDetail 渲染：有 provider / 无 provider / opencode 无 model 三种状态快照
- AgentCard 渲染：同上
- Edit 模式：进入 → 改 → Save → 调对应 API → store 更新
- unsaved changes 守卫：修改后点 Cancel 弹确认
- `<EnvVarsEditor>` 共享组件：增删改 KV、AddAgentDialog 和 AgentDetail 用起来表现一致

## Files Touched

**后端（新增/改）**：
- `crates/gitim-runtime/src/http.rs` — 新增 PATCH handler
- `crates/gitim-runtime/src/state.rs` — 可能加 helper 更新 AgentInfo
- `crates/gitim-runtime/src/workspace.rs` — `.gitignore` 初始化逻辑
- `crates/gitim-runtime/tests/` — 新增测试文件

**前端（新增/改）**：
- `webui-v2/src/components/management/agent-detail.tsx` — Edit 模式 + Provider 显示 + model fallback 修复
- `webui-v2/src/components/management/agent-card.tsx` — 加 provider/model 行
- `webui-v2/src/components/management/provider-badge.tsx` — 新组件
- `webui-v2/src/components/management/env-vars-editor.tsx` — 新共享组件
- `webui-v2/src/components/management/add-agent-dialog.tsx` — 换用 `<EnvVarsEditor>`
- `webui-v2/src/lib/client.ts` — 新增 `updateAgent`
- `webui-v2/src/hooks/use-agent-store.ts` — 可能加 action（若未有）

## Open Questions

无。所有设计决策已与用户确认。
