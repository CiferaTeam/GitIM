# Agent Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Agent 创建时支持模型选择、自定义环境变量和 System Prompt，并持久化到 me.json。

**Architecture:** 扩展 `AgentAddRequest` → 写入 `me.json` → `AgentLoop` 从 config 读取 model/env/system_prompt → recovery 从 me.json 恢复。前端增加模型下拉和环境变量编辑器。

**Tech Stack:** Rust (axum, serde), TypeScript (React, Tailwind)

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/gitim-runtime/src/http.rs` | Modify | API 层：AgentAddRequest、AgentInfo、agents_add、start_agent_loop、recover_from_config |
| `crates/gitim-runtime/src/agent_loop.rs` | Modify | AgentLoop：接受外部 model/env/system_prompt 配置 |
| `webui-v2/src/lib/types.ts` | Modify | Agent 类型增加 model、env 字段 |
| `webui-v2/src/lib/client.ts` | Modify | addAgent 发送新字段，mapBackendAgent 映射新字段 |
| `webui-v2/src/components/management/add-agent-dialog.tsx` | Modify | 增加模型选择器和环境变量编辑器 |
| `webui-v2/src/components/management/agent-detail.tsx` | Modify | 展示 model、env、system_prompt |

---

### Task 1: Backend — 扩展 AgentAddRequest 和 AgentInfo

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: 给 AgentAddRequest 添加 model、system_prompt、env 字段**

在 `http.rs` 中找到 `AgentAddRequest`，改为：

```rust
#[derive(Deserialize)]
struct AgentAddRequest {
    handler: String,
    display_name: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}
```

- [ ] **Step 2: 给 AgentInfo 添加 model、system_prompt、env 字段**

找到 `AgentInfo`，添加三个字段（在 `last_activity` 之后、`repo_root` 之前）：

```rust
#[derive(Clone, Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub handler: String,
    pub display_name: String,
    pub status: String,
    pub last_activity: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(skip)]
    pub repo_root: PathBuf,
    #[serde(skip)]
    pub loop_handle: Option<AbortHandle>,
}
```

- [ ] **Step 3: 更新 agents_add 中 AgentInfo 的构建**

在 `agents_add` handler 中，`provision_agent` 成功后构建 `AgentInfo` 时加上新字段：

```rust
let info = AgentInfo {
    id: req.handler.clone(),
    handler: req.handler.clone(),
    display_name: req.display_name.clone(),
    status: "idle".to_string(),
    last_activity: None,
    model: req.model.clone(),
    system_prompt: req.system_prompt.clone(),
    env: req.env.clone(),
    repo_root: handle.repo_root,
    loop_handle: None,
};
```

- [ ] **Step 4: 在 agents_add 中持久化 config 到 me.json**

在 `agents_add` 的 `provision_agent` 成功后、`start_agent_loop` 之前，将 model/system_prompt/env 写入 agent 的 `me.json`：

```rust
// Persist config to me.json
{
    let me_path = handle.repo_root.join(".gitim/me.json");
    if let Ok(content) = std::fs::read_to_string(&me_path) {
        if let Ok(mut me) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(model) = &req.model {
                me["model"] = serde_json::Value::String(model.clone());
            }
            if let Some(sp) = &req.system_prompt {
                me["system_prompt"] = serde_json::Value::String(sp.clone());
            }
            if !req.env.is_empty() {
                me["env"] = serde_json::to_value(&req.env).unwrap_or_default();
            }
            let _ = std::fs::write(&me_path, serde_json::to_string_pretty(&me).unwrap());
        }
    }
}
```

- [ ] **Step 5: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译通过（可能有 unused 警告，后续 task 会用到）

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): extend AgentAddRequest and AgentInfo with model/system_prompt/env"
```

---

### Task 2: Backend — AgentLoop 接受外部配置

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: 添加 AgentLoopConfig 结构体**

在 `AgentLoop` 定义之前添加：

```rust
#[derive(Debug, Clone, Default)]
pub struct AgentLoopConfig {
    pub provider_type: String,
    pub handler: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub env: HashMap<String, String>,
}
```

需要在文件顶部 `use std::path::{Path, PathBuf};` 后面加上：

```rust
use std::collections::HashMap;
```

- [ ] **Step 2: 给 AgentLoop 添加 custom_system_prompt 字段**

在 `AgentLoop` struct 中，把 `model` 字段后面加 `custom_system_prompt`：

```rust
pub struct AgentLoop {
    poller: Poller,
    provider: Box<dyn Provider>,
    session_token: Option<String>,
    poll_interval: Duration,
    repo_root: PathBuf,
    model: Option<String>,
    custom_system_prompt: Option<String>,
    handler: String,
    activity_tx: Option<broadcast::Sender<AgentActivityEvent>>,
}
```

- [ ] **Step 3: 新增 with_config 构造函数**

保留 `with_provider` 不变（给 recovery 和旧路径兜底），新增 `with_config`：

```rust
pub fn with_config(
    repo_root: &Path,
    config: &AgentLoopConfig,
) -> Result<Self, RuntimeError> {
    let state = AgentState::load(repo_root)?;

    let poller = match state.cursor {
        Some(cursor) => {
            info!(cursor = %cursor, "restored cursor from state");
            Poller::with_cursor(GitimClient::new(repo_root), cursor)
        }
        None => Poller::new(GitimClient::new(repo_root)),
    };

    let provider_config = ProviderConfig {
        executable_path: None,
        env: config.env.clone(),
    };
    let provider = create(&config.provider_type, provider_config)
        .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

    if state.session_token.is_some() {
        info!("restored session_token from state");
    }

    Ok(Self {
        poller,
        provider,
        session_token: state.session_token,
        poll_interval: Duration::from_secs(2),
        repo_root: repo_root.to_path_buf(),
        model: config.model.clone().or_else(|| Some("claude-sonnet-4-6".to_string())),
        custom_system_prompt: config.system_prompt.clone(),
        handler: config.handler.clone(),
        activity_tx: None,
    })
}
```

- [ ] **Step 4: 给 with_provider 补上 custom_system_prompt 字段**

在现有的 `with_provider` 构造函数的 `Ok(Self { ... })` 中加上：

```rust
custom_system_prompt: None,
```

- [ ] **Step 5: 修改 build_exec_options 合并自定义 system prompt**

```rust
fn build_exec_options(&self) -> ExecOptions {
    let system_prompt = if self.session_token.is_none() {
        let mut prompt = build_system_prompt(&self.handler);
        if let Some(custom) = &self.custom_system_prompt {
            if !custom.is_empty() {
                prompt.push_str("\n\n## 用户自定义指令\n\n");
                prompt.push_str(custom);
            }
        }
        Some(prompt)
    } else {
        None
    };

    ExecOptions {
        cwd: Some(self.repo_root.clone()),
        model: self.model.clone(),
        system_prompt,
        max_turns: Some(20),
        resume_token: self.session_token.clone(),
        ..Default::default()
    }
}
```

- [ ] **Step 6: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "feat(runtime): AgentLoop accepts external model/env/system_prompt config"
```

---

### Task 3: Backend — start_agent_loop 和 recovery 使用新配置

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: 修改 start_agent_loop 从 AgentInfo 读取配置**

找到 `start_agent_loop` 函数，修改 destructure 和 AgentLoop 创建：

```rust
fn start_agent_loop(state: &SharedRuntimeState, agent_id: &str) -> Result<(), String> {
    let (repo_root, handler, model, system_prompt, env) = {
        let s = state.lock().unwrap();
        match s.agents.get(agent_id) {
            None => return Err(format!("agent not found: {agent_id}")),
            Some(info) if info.status == "running" => {
                return Err(format!("agent already running: {agent_id}"));
            }
            Some(info) => (
                info.repo_root.clone(),
                info.handler.clone(),
                info.model.clone(),
                info.system_prompt.clone(),
                info.env.clone(),
            ),
        }
    };

    let config = crate::agent_loop::AgentLoopConfig {
        provider_type: "claude".to_string(),
        handler,
        model,
        system_prompt,
        env,
    };
    let mut agent_loop = AgentLoop::with_config(&repo_root, &config)
        .map_err(|e| format!("failed to create agent loop: {e}"))?;

    // ... rest unchanged
```

需要在文件顶部加上 import：

```rust
use crate::agent_loop::AgentLoopConfig;
```

（注意：`AgentLoop` 已经在 import 中了，只需加 `AgentLoopConfig`）

- [ ] **Step 2: 修改 recover_from_config 从 me.json 读取扩展字段**

在 `recover_from_config` 函数中，读取 `me.json` 的代码块内（已有 `handler` 和 `display_name` 的解析之后），添加 model/system_prompt/env 的解析：

```rust
let model = me["model"].as_str().map(|s| s.to_string());
let system_prompt = me["system_prompt"].as_str().map(|s| s.to_string());
let env: HashMap<String, String> = me.get("env")
    .and_then(|v| serde_json::from_value(v.clone()).ok())
    .unwrap_or_default();
```

然后在构建 `AgentInfo` 时加上这三个字段：

```rust
s.agents.insert(handler.clone(), AgentInfo {
    id: handler.clone(),
    handler: handler.clone(),
    display_name,
    status: "idle".to_string(),
    last_activity: None,
    model,
    system_prompt,
    env,
    repo_root: dir,
    loop_handle: None,
});
```

- [ ] **Step 3: 编译验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译通过，无错误

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): wire start_agent_loop and recovery to use agent config"
```

---

### Task 4: Frontend — 扩展 Agent 类型和 client

**Files:**
- Modify: `webui-v2/src/lib/types.ts`
- Modify: `webui-v2/src/lib/client.ts`

- [ ] **Step 1: 给 Agent 接口添加 model 和 env 字段**

在 `types.ts` 的 `Agent` interface 中，`systemPrompt` 后面添加：

```typescript
model?: string;
env?: Record<string, string>;
```

- [ ] **Step 2: 更新 mapBackendAgent 映射新字段**

在 `client.ts` 的 `mapBackendAgent` 函数中，返回对象里添加：

```typescript
model: (raw.model as string) ?? undefined,
env: (raw.env as Record<string, string>) ?? undefined,
```

- [ ] **Step 3: 更新 addAgent 函数签名和请求体**

改 `addAgent` 的签名和实现：

```typescript
export async function addAgent(
  name: string,
  systemPrompt: string,
  model?: string,
  env?: Record<string, string>,
): Promise<ApiResponse> {
  try {
    const handler = toHandler(name);
    const res = await fetch(`${baseUrl()}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        handler,
        display_name: name,
        model: model || undefined,
        system_prompt: systemPrompt || undefined,
        env: env && Object.keys(env).length > 0 ? env : undefined,
      }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    const agent: Agent = {
      id: data.id ?? handler,
      name,
      status: "offline",
      systemPrompt,
      model,
      env,
      repoPath: "",
      messagesProcessed: 0,
    };
    return { ok: true, data: { agent } };
  } catch {
    return mockClient.addAgent(name, systemPrompt);
  }
}
```

- [ ] **Step 4: 编译验证**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | tail -10`
Expected: 无类型错误（addAgent 调用方会在 Task 5 更新）

- [ ] **Step 5: Commit**

```bash
git add webui-v2/src/lib/types.ts webui-v2/src/lib/client.ts
git commit -m "feat(webui): extend Agent type and client with model/env fields"
```

---

### Task 5: Frontend — AddAgentDialog 增加模型选择器和环境变量编辑器

**Files:**
- Modify: `webui-v2/src/components/management/add-agent-dialog.tsx`

- [ ] **Step 1: 添加 model 和 env 的 state**

在 `AddAgentDialog` 组件中，现有 state 后面添加：

```typescript
const [model, setModel] = useState("claude-sonnet-4-6");
const [envVars, setEnvVars] = useState<{ key: string; value: string }[]>([]);
```

- [ ] **Step 2: 更新 handleSubmit 传递新字段**

```typescript
async function handleSubmit(e: React.FormEvent) {
  e.preventDefault();
  if (!name.trim() || validationError) return;

  const envMap: Record<string, string> = {};
  for (const { key, value } of envVars) {
    if (key.trim()) envMap[key.trim()] = value;
  }

  const res = await client.addAgent(
    name.trim(),
    systemPrompt.trim(),
    model,
    envMap,
  );
  if (res.ok && res.data?.agent) {
    addAgent(res.data.agent as Agent);
    setName("");
    setSystemPrompt("");
    setModel("claude-sonnet-4-6");
    setEnvVars([]);
    setOpen(false);
  } else {
    toast.error(res.error ?? "Failed to add agent");
  }
}
```

- [ ] **Step 3: 在表单中添加模型选择器（System Prompt 之前）**

在 System Prompt 的 `<div>` 之前插入：

```tsx
<div className="space-y-1.5">
  <label className="text-sm font-medium" htmlFor="agent-model">
    Model
  </label>
  <select
    id="agent-model"
    value={model}
    onChange={(e) => setModel(e.target.value)}
    className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
  >
    <option value="claude-sonnet-4-6">Claude Sonnet 4.6</option>
    <option value="claude-opus-4-6">Claude Opus 4.6</option>
    <option value="claude-haiku-4-5">Claude Haiku 4.5</option>
  </select>
</div>
```

- [ ] **Step 4: 在 System Prompt 之后添加环境变量编辑器**

在 System Prompt 的 `</div>` 后面插入：

```tsx
<div className="space-y-1.5">
  <label className="text-sm font-medium">Environment Variables</label>
  <div className="space-y-2">
    {envVars.map((pair, i) => (
      <div key={i} className="flex gap-2">
        <Input
          placeholder="KEY"
          value={pair.key}
          onChange={(e) => {
            const updated = [...envVars];
            updated[i] = { ...updated[i], key: e.target.value };
            setEnvVars(updated);
          }}
          className="flex-1 font-mono text-xs"
        />
        <Input
          placeholder="value"
          value={pair.value}
          onChange={(e) => {
            const updated = [...envVars];
            updated[i] = { ...updated[i], value: e.target.value };
            setEnvVars(updated);
          }}
          className="flex-1 font-mono text-xs"
        />
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => setEnvVars(envVars.filter((_, j) => j !== i))}
          className="px-2 text-muted-foreground hover:text-destructive"
        >
          ×
        </Button>
      </div>
    ))}
    <Button
      type="button"
      variant="outline"
      size="sm"
      onClick={() => setEnvVars([...envVars, { key: "", value: "" }])}
    >
      + Add Variable
    </Button>
  </div>
</div>
```

- [ ] **Step 5: 编译验证**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | tail -10`
Expected: 无类型错误

- [ ] **Step 6: Commit**

```bash
git add webui-v2/src/components/management/add-agent-dialog.tsx
git commit -m "feat(webui): add model selector and env editor to AddAgentDialog"
```

---

### Task 6: Frontend — agent-detail 展示新字段

**Files:**
- Modify: `webui-v2/src/components/management/agent-detail.tsx`

- [ ] **Step 1: 在 Fields grid 中添加 Model 字段**

在 `agent-detail.tsx` 的 Fields grid 中（`Session ID` 后面），添加：

```tsx
<Field label="Model">
  <code className="text-sm font-mono text-muted-foreground">
    {agent.model ?? "claude-sonnet-4-6"}
  </code>
</Field>
```

- [ ] **Step 2: 在 System Prompt 之后添加 Environment Variables 展示**

在 System Prompt 的 `</div>` 后面、Activity Log 之前，添加：

```tsx
{agent.env && Object.keys(agent.env).length > 0 && (
  <div className="mb-6">
    <Field label="Environment Variables">
      <div className="mt-1 border rounded-md p-3 bg-muted/30 space-y-1">
        {Object.entries(agent.env).map(([key, value]) => (
          <div key={key} className="text-sm font-mono">
            <span className="text-muted-foreground">{key}</span>
            <span className="text-muted-foreground mx-1">=</span>
            <span>{value}</span>
          </div>
        ))}
      </div>
    </Field>
  </div>
)}
```

- [ ] **Step 3: 编译验证**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | tail -10`
Expected: 无类型错误

- [ ] **Step 4: Commit**

```bash
git add webui-v2/src/components/management/agent-detail.tsx
git commit -m "feat(webui): display model and env vars in agent detail view"
```

---

### Task 7: 全栈验证

- [ ] **Step 1: Rust 构建验证**

Run: `cargo build 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 2: 前端构建验证**

Run: `cd webui-v2 && npx tsc --noEmit && npm run build 2>&1 | tail -10`
Expected: 无类型错误，构建成功

- [ ] **Step 3: Commit plan 文件**

```bash
git add plans/2026-04-14-agent-config.md
git commit -m "docs: add agent-config implementation plan"
```
