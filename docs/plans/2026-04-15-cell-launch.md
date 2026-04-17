# Cell.gitim.io 产品上线 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 cell.gitim.io 邀请制产品上线——Cloudflare Worker 后端（邀请码验证 + 管理 API）+ webui-v2 前端（InviteGate + 安装引导）+ Runtime 改造（Claude 预检端点）。

**Architecture:** Cloudflare Worker + KV 作为轻量后端，处理邀请码验证、设备注册和活跃心跳。webui-v2 部署到 Cloudflare Pages 作为 cell.gitim.io，在现有 SetupGate 前增加 InviteGate。Runtime 新增 Claude CLI 预检端点供前端调用。

**Tech Stack:** Cloudflare Workers (Hono) + KV, React 19 + Vite + Zustand + Tailwind 4, Rust (Axum)

---

## File Structure

### New: Cloudflare Worker (`services/cell-api/`)

```
services/cell-api/
├── wrangler.toml           # Worker 配置 + KV 绑定
├── package.json            # Hono + wrangler 依赖
├── tsconfig.json           # TypeScript 配置
└── src/
    ├── index.ts            # Hono 应用入口，挂载所有路由
    ├── invite.ts           # POST /api/verify — 邀请码验证 + 设备注册
    ├── heartbeat.ts        # POST /api/heartbeat — 版本检查 + 活跃心跳
    ├── admin.ts            # /admin/codes CRUD + 设备管理
    └── types.ts            # InviteCode / Device 类型 + KV key 工具
```

### Modified: webui-v2 (`webui-v2/`)

```
webui-v2/
├── .env.example                        # NEW: VITE_CELL_API_URL 示例
├── src/
│   ├── lib/
│   │   ├── device.ts                   # NEW: localStorage UUID 管理
│   │   └── cell-api.ts                 # NEW: Cloudflare Worker API 客户端
│   ├── components/
│   │   └── invite/
│   │       ├── invite-gate.tsx         # NEW: 邀请码验证门
│   │       └── guide-page.tsx          # NEW: 安装引导页（含 Claude 预检）
│   └── app.tsx                         # MODIFY: 用 InviteGate 包裹 SetupGate
```

### Modified: Runtime (`crates/gitim-runtime/`)

```
crates/gitim-runtime/src/
├── http.rs                 # MODIFY: 添加 /preflight/claude 路由
└── preflight.rs            # MODIFY: 添加 check_claude() 函数
```

---

## Phase 1: Cloudflare Worker 后端

### Task 1: Worker 项目初始化

**Files:**
- Create: `services/cell-api/package.json`
- Create: `services/cell-api/tsconfig.json`
- Create: `services/cell-api/wrangler.toml`
- Create: `services/cell-api/src/types.ts`

- [ ] **Step 1: 创建项目目录**

```bash
mkdir -p services/cell-api/src
```

- [ ] **Step 2: 写 package.json**

Create `services/cell-api/package.json`:

```json
{
  "name": "cell-api",
  "private": true,
  "scripts": {
    "dev": "wrangler dev",
    "deploy": "wrangler deploy"
  },
  "dependencies": {
    "hono": "^4"
  },
  "devDependencies": {
    "@cloudflare/workers-types": "^4",
    "wrangler": "^4",
    "typescript": "^5"
  }
}
```

- [ ] **Step 3: 写 tsconfig.json**

Create `services/cell-api/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "lib": ["ESNext"],
    "types": ["@cloudflare/workers-types"],
    "outDir": "dist",
    "rootDir": "src"
  },
  "include": ["src"]
}
```

- [ ] **Step 4: 写 wrangler.toml**

Create `services/cell-api/wrangler.toml`:

```toml
name = "cell-api"
main = "src/index.ts"
compatibility_date = "2024-12-01"

[[kv_namespaces]]
binding = "CELL_KV"
id = "placeholder"
preview_id = "placeholder"

[vars]
ADMIN_SECRET = "changeme-in-dashboard"
```

> **Note:** KV namespace ID 需要在部署前通过 `wrangler kv namespace create CELL_KV` 获取后填入。

- [ ] **Step 5: 写 types.ts**

Create `services/cell-api/src/types.ts`:

```typescript
export interface Device {
  id: string;
  registered_at: string;
  last_seen: string;
}

export interface InviteCode {
  code: string;
  created_at: string;
  max_devices: number;
  note: string;
  devices: Device[];
}

export type Bindings = {
  CELL_KV: KVNamespace;
  ADMIN_SECRET: string;
};

export function kvKey(code: string): string {
  return `invite:${code}`;
}
```

- [ ] **Step 6: 安装依赖**

```bash
cd services/cell-api && npm install
```

- [ ] **Step 7: Commit**

```bash
git add services/cell-api/
git commit -m "feat(cell-api): scaffold Cloudflare Worker project with Hono + KV"
```

---

### Task 2: 邀请码验证端点

**Files:**
- Create: `services/cell-api/src/invite.ts`

- [ ] **Step 1: 写 invite.ts**

Create `services/cell-api/src/invite.ts`:

```typescript
import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/verify", async (c) => {
  const body = await c.req.json<{ code?: string; device_id?: string }>();
  const code = body.code?.trim();
  const deviceId = body.device_id?.trim();

  if (!code || !deviceId) {
    return c.json({ ok: false, error: "missing code or device_id" }, 400);
  }
  if (code.length > 64) {
    return c.json({ ok: false, error: "code too long" }, 400);
  }

  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) {
    return c.json({ ok: false, error: "invalid code" }, 403);
  }

  const invite: InviteCode = JSON.parse(raw);

  // Already registered device — update last_seen
  const existing = invite.devices.find((d) => d.id === deviceId);
  if (existing) {
    existing.last_seen = new Date().toISOString();
    await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));
    return c.json({ ok: true });
  }

  // New device — check limit
  if (invite.devices.length >= invite.max_devices) {
    return c.json({ ok: false, error: "device limit reached" }, 403);
  }

  const now = new Date().toISOString();
  invite.devices.push({ id: deviceId, registered_at: now, last_seen: now });
  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true });
});

export { app as inviteRoutes };
```

- [ ] **Step 2: Commit**

```bash
git add services/cell-api/src/invite.ts
git commit -m "feat(cell-api): add invite code verification endpoint"
```

---

### Task 3: 心跳端点

**Files:**
- Create: `services/cell-api/src/heartbeat.ts`

- [ ] **Step 1: 写 heartbeat.ts**

Create `services/cell-api/src/heartbeat.ts`:

```typescript
import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/heartbeat", async (c) => {
  const body = await c.req.json<{
    code?: string;
    device_id?: string;
    version?: string;
  }>();
  const code = body.code?.trim();
  const deviceId = body.device_id?.trim();

  if (!code || !deviceId) {
    return c.json({ ok: false, error: "missing fields" }, 400);
  }

  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) {
    return c.json({ ok: false, error: "invalid code" }, 403);
  }

  const invite: InviteCode = JSON.parse(raw);
  const device = invite.devices.find((d) => d.id === deviceId);
  if (!device) {
    return c.json({ ok: false, error: "device not registered" }, 403);
  }

  device.last_seen = new Date().toISOString();
  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));

  return c.json({ ok: true, latest_version: body.version });
});

export { app as heartbeatRoutes };
```

- [ ] **Step 2: Commit**

```bash
git add services/cell-api/src/heartbeat.ts
git commit -m "feat(cell-api): add heartbeat endpoint for version check + activity tracking"
```

---

### Task 4: 管理 API + 路由入口

**Files:**
- Create: `services/cell-api/src/admin.ts`
- Create: `services/cell-api/src/index.ts`

- [ ] **Step 1: 写 admin.ts**

Create `services/cell-api/src/admin.ts`:

```typescript
import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

// Auth middleware — all /admin/* routes require X-Admin-Secret header
app.use("*", async (c, next) => {
  if (c.req.header("x-admin-secret") !== c.env.ADMIN_SECRET) {
    return c.json({ error: "unauthorized" }, 401);
  }
  await next();
});

// List all invite codes
app.get("/admin/codes", async (c) => {
  const list = await c.env.CELL_KV.list({ prefix: "invite:" });
  const codes: InviteCode[] = [];
  for (const key of list.keys) {
    const raw = await c.env.CELL_KV.get(key.name);
    if (raw) codes.push(JSON.parse(raw));
  }
  return c.json({ codes });
});

// Create invite code
app.post("/admin/codes", async (c) => {
  const body = await c.req.json<{
    code?: string;
    note?: string;
    max_devices?: number;
  }>();
  const code = body.code?.trim();

  if (!code || code.length > 64) {
    return c.json({ error: "code required, max 64 chars" }, 400);
  }

  const existing = await c.env.CELL_KV.get(kvKey(code));
  if (existing) {
    return c.json({ error: "code already exists" }, 409);
  }

  const invite: InviteCode = {
    code,
    created_at: new Date().toISOString(),
    max_devices: body.max_devices ?? 5,
    note: body.note ?? "",
    devices: [],
  };

  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true, invite }, 201);
});

// Get code detail
app.get("/admin/codes/:code", async (c) => {
  const code = c.req.param("code");
  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) return c.json({ error: "not found" }, 404);
  return c.json(JSON.parse(raw));
});

// Delete code
app.delete("/admin/codes/:code", async (c) => {
  const code = c.req.param("code");
  await c.env.CELL_KV.delete(kvKey(code));
  return c.json({ ok: true });
});

// Remove a device from a code (manual reset)
app.delete("/admin/codes/:code/devices/:deviceId", async (c) => {
  const code = c.req.param("code");
  const deviceId = c.req.param("deviceId");

  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) return c.json({ error: "not found" }, 404);

  const invite: InviteCode = JSON.parse(raw);
  invite.devices = invite.devices.filter((d) => d.id !== deviceId);
  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true });
});

export { app as adminRoutes };
```

- [ ] **Step 2: 写 index.ts — 应用入口**

Create `services/cell-api/src/index.ts`:

```typescript
import { Hono } from "hono";
import { cors } from "hono/cors";
import type { Bindings } from "./types";
import { inviteRoutes } from "./invite";
import { heartbeatRoutes } from "./heartbeat";
import { adminRoutes } from "./admin";

const app = new Hono<{ Bindings: Bindings }>();

app.use(
  "*",
  cors({
    origin: ["https://cell.gitim.io", "http://localhost:5173"],
    allowMethods: ["GET", "POST", "DELETE"],
    allowHeaders: ["Content-Type", "X-Admin-Secret"],
  })
);

app.route("/", inviteRoutes);
app.route("/", heartbeatRoutes);
app.route("/", adminRoutes);

app.get("/", (c) => c.json({ service: "cell-api", status: "ok" }));

export default app;
```

- [ ] **Step 3: 本地验证**

```bash
cd services/cell-api && npm run dev
```

在另一个终端测试：

```bash
# 创建邀请码
curl -X POST http://localhost:8787/admin/codes \
  -H "Content-Type: application/json" \
  -H "X-Admin-Secret: changeme-in-dashboard" \
  -d '{"code":"大漠孤烟直","note":"测试用"}'
# Expected: {"ok":true,"invite":{"code":"大漠孤烟直",...}}

# 验证邀请码
curl -X POST http://localhost:8787/api/verify \
  -H "Content-Type: application/json" \
  -d '{"code":"大漠孤烟直","device_id":"test-device-001"}'
# Expected: {"ok":true}

# 列出所有码
curl http://localhost:8787/admin/codes \
  -H "X-Admin-Secret: changeme-in-dashboard"
# Expected: {"codes":[{"code":"大漠孤烟直","devices":[{"id":"test-device-001",...}],...}]}
```

- [ ] **Step 4: Commit**

```bash
git add services/cell-api/src/admin.ts services/cell-api/src/index.ts
git commit -m "feat(cell-api): add admin CRUD + wire all routes in Hono entry"
```

---

## Phase 2: webui-v2 前端改造

### Task 5: 设备 UUID + Cell API 客户端

**Files:**
- Create: `webui-v2/src/lib/device.ts`
- Create: `webui-v2/src/lib/cell-api.ts`
- Create: `webui-v2/.env.example`

- [ ] **Step 1: 写 device.ts**

Create `webui-v2/src/lib/device.ts`:

```typescript
const DEVICE_ID_KEY = "gitim:device_id";

export function getDeviceId(): string {
  let id = localStorage.getItem(DEVICE_ID_KEY);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(DEVICE_ID_KEY, id);
  }
  return id;
}
```

- [ ] **Step 2: 写 cell-api.ts**

Create `webui-v2/src/lib/cell-api.ts`:

```typescript
const API_URL = import.meta.env.VITE_CELL_API_URL ?? "";

interface VerifyResult {
  ok: boolean;
  error?: string;
}

export async function verifyInviteCode(
  code: string,
  deviceId: string
): Promise<VerifyResult> {
  try {
    const res = await fetch(`${API_URL}/api/verify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, device_id: deviceId }),
    });
    return (await res.json()) as VerifyResult;
  } catch {
    return { ok: false, error: "无法连接验证服务" };
  }
}

export async function sendHeartbeat(
  code: string,
  deviceId: string,
  version?: string
): Promise<void> {
  try {
    await fetch(`${API_URL}/api/heartbeat`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, device_id: deviceId, version }),
    });
  } catch {
    // heartbeat failure is non-critical, silently ignore
  }
}
```

- [ ] **Step 3: 写 .env.example**

Create `webui-v2/.env.example`:

```
# Cell API URL — Cloudflare Worker 后端地址
# 开发环境：http://localhost:8787
# 生产环境：https://cell-api.gitim.io（或你的 Worker 自定义域名）
VITE_CELL_API_URL=http://localhost:8787
```

- [ ] **Step 4: Commit**

```bash
git add webui-v2/src/lib/device.ts webui-v2/src/lib/cell-api.ts webui-v2/.env.example
git commit -m "feat(webui): add device UUID management and Cell API client"
```

---

### Task 6: InviteGate 组件

**Files:**
- Create: `webui-v2/src/components/invite/invite-gate.tsx`

**Design context:** 遵循 `webui-v2/src/index.css` 的 warm dark palette。背景 `bg-background` (#24242a)，卡片 `bg-card` (#2c2c32)，主按钮 `bg-primary` (#60a5fa)，错误 `text-error` (#f87171)，字体 Plus Jakarta Sans。

- [ ] **Step 1: 写 invite-gate.tsx**

Create `webui-v2/src/components/invite/invite-gate.tsx`:

```tsx
import { useState, useEffect, type ReactNode } from "react";
import { verifyInviteCode } from "../../lib/cell-api";
import { getDeviceId } from "../../lib/device";
import { GuidePage } from "./guide-page";

const INVITE_CODE_KEY = "gitim:invite_code";
const INVITE_VERIFIED_KEY = "gitim:invite_verified";
const SETUP_COMPLETED_KEY = "gitim:setup_completed";

type GateStatus = "checking" | "need_code" | "need_setup" | "verified";

export function InviteGate({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<GateStatus>("checking");
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const verified = localStorage.getItem(INVITE_VERIFIED_KEY);
    if (!verified) {
      setStatus("need_code");
      return;
    }
    const setupDone = localStorage.getItem(SETUP_COMPLETED_KEY);
    setStatus(setupDone ? "verified" : "need_setup");
  }, []);

  if (status === "checking") {
    return (
      <div className="flex items-center justify-center h-screen bg-background text-muted-foreground text-sm">
        Loading...
      </div>
    );
  }

  if (status === "verified") {
    return <>{children}</>;
  }

  if (status === "need_setup") {
    return (
      <GuidePage
        onComplete={() => {
          localStorage.setItem(SETUP_COMPLETED_KEY, "true");
          setStatus("verified");
        }}
      />
    );
  }

  // status === "need_code"
  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = code.trim();
    if (!trimmed) return;

    setError("");
    setLoading(true);

    const deviceId = getDeviceId();
    const result = await verifyInviteCode(trimmed, deviceId);

    setLoading(false);
    if (result.ok) {
      localStorage.setItem(INVITE_CODE_KEY, trimmed);
      localStorage.setItem(INVITE_VERIFIED_KEY, "true");
      setStatus("need_setup");
    } else {
      setError(result.error ?? "验证失败");
    }
  };

  return (
    <div className="flex items-center justify-center h-screen bg-background p-4">
      <div className="w-full max-w-sm space-y-6">
        <div className="text-center space-y-2">
          <h1 className="text-xl font-bold tracking-tight text-foreground">
            GitIM Cell
          </h1>
          <p className="text-sm text-muted-foreground">输入口诀以继续</p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <input
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="你的口诀"
            maxLength={64}
            className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm text-foreground placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
            autoFocus
          />

          {error && <p className="text-xs text-error">{error}</p>}

          <button
            type="submit"
            disabled={!code.trim() || loading}
            className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
          >
            {loading ? "验证中..." : "进入"}
          </button>
        </form>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add webui-v2/src/components/invite/invite-gate.tsx
git commit -m "feat(webui): add InviteGate component with code verification"
```

---

### Task 7: GuidePage 安装引导页

**Files:**
- Create: `webui-v2/src/components/invite/guide-page.tsx`

**Behavior:** 展示安装步骤，提供端口连接测试和 Claude CLI 预检。连接成功后调用 `onComplete` 通知 InviteGate 完成引导。连接同时设置 connection store 的 port，这样 SetupGate 会跳过 ConnectForm 直接进入后续步骤。

- [ ] **Step 1: 写 guide-page.tsx**

Create `webui-v2/src/components/invite/guide-page.tsx`:

```tsx
import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

interface GuidePageProps {
  onComplete: () => void;
}

export function GuidePage({ onComplete }: GuidePageProps) {
  const setPort = useConnectionStore((s) => s.setPort);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);
  const setStatus = useConnectionStore((s) => s.setStatus);

  const [portInput, setPortInput] = useState("16868");
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState("");

  const [claudeStatus, setClaudeStatus] = useState<
    "idle" | "checking" | "ok" | "error"
  >("idle");
  const [claudeInfo, setClaudeInfo] = useState("");

  const handleConnect = async () => {
    const p = parseInt(portInput, 10);
    if (!Number.isFinite(p) || p < 1 || p > 65535) {
      setConnectError("请输入有效端口号 (1-65535)");
      return;
    }

    setConnecting(true);
    setConnectError("");

    try {
      const res = await fetch(`http://127.0.0.1:${p}/health`, {
        signal: AbortSignal.timeout(3000),
      });
      const data = await res.json();

      if (data.service !== "gitim-runtime") {
        setConnectError("连接成功，但服务不是 gitim-runtime");
        return;
      }

      // Persist port + version in connection store so SetupGate picks it up
      setPort(p);
      if (data.version) setRuntimeVersion(data.version as string);
      setStatus(data.initialized ? "ready" : "connected");

      onComplete();
    } catch {
      setConnectError(`无法连接 127.0.0.1:${p}，请确认 Runtime 已启动`);
    } finally {
      setConnecting(false);
    }
  };

  const handleClaudeCheck = async () => {
    const p = parseInt(portInput, 10);
    if (!Number.isFinite(p) || p < 1 || p > 65535) {
      setClaudeInfo("请先输入有效端口号");
      setClaudeStatus("error");
      return;
    }

    setClaudeStatus("checking");
    setClaudeInfo("");

    try {
      const res = await fetch(`http://127.0.0.1:${p}/preflight/claude`, {
        signal: AbortSignal.timeout(10000),
      });
      const data = await res.json();

      if (data.available) {
        setClaudeStatus("ok");
        setClaudeInfo(data.version ? `Claude ${data.version}` : "可用");
      } else {
        setClaudeStatus("error");
        setClaudeInfo(data.error ?? "Claude CLI 不可用");
      }
    } catch {
      setClaudeStatus("error");
      setClaudeInfo("无法连接 Runtime，请先启动");
    }
  };

  return (
    <div className="flex items-center justify-center min-h-screen bg-background p-4">
      <div className="w-full max-w-lg space-y-8">
        <div className="text-center space-y-2">
          <h1 className="text-xl font-bold tracking-tight text-foreground">
            设置 GitIM
          </h1>
          <p className="text-sm text-muted-foreground">
            完成以下步骤开始使用
          </p>
        </div>

        {/* Step 1: Install */}
        <section className="space-y-2">
          <h2 className="text-sm font-medium text-foreground">
            1. 安装 GitIM
          </h2>
          <div className="rounded-md bg-card p-3 font-mono text-xs text-foreground leading-relaxed select-all">
            curl -sSf
            https://raw.githubusercontent.com/CiferaTeam/gitim-releases/main/install.sh
            | sh
          </div>
        </section>

        {/* Step 2: Start Runtime */}
        <section className="space-y-2">
          <h2 className="text-sm font-medium text-foreground">
            2. 启动 Runtime
          </h2>
          <div className="rounded-md bg-card p-3 font-mono text-xs text-foreground select-all">
            gitim-runtime --port 16868
          </div>
          <p className="text-xs text-text-muted leading-relaxed">
            Runtime 在 24 小时无活动后会自动退出，无需手动关闭。
          </p>
        </section>

        {/* Step 3: Connect */}
        <section className="space-y-3">
          <h2 className="text-sm font-medium text-foreground">3. 连接</h2>
          <div className="flex gap-2">
            <input
              type="text"
              inputMode="numeric"
              value={portInput}
              onChange={(e) => setPortInput(e.target.value)}
              placeholder="16868"
              className="flex-1 h-9 px-3 rounded-md border border-input bg-background text-sm font-mono text-foreground placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
            />
            <button
              onClick={handleConnect}
              disabled={connecting}
              className="h-9 px-4 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
            >
              {connecting ? "连接中..." : "连接"}
            </button>
          </div>
          {connectError && (
            <p className="text-xs text-error">{connectError}</p>
          )}
        </section>

        {/* Step 4: Claude Check (optional) */}
        <section className="space-y-2">
          <h2 className="text-sm font-medium text-foreground">
            4. 检测 Claude CLI
            <span className="text-text-muted font-normal ml-1">（可选）</span>
          </h2>
          <div className="flex items-center gap-3">
            <button
              onClick={handleClaudeCheck}
              disabled={claudeStatus === "checking"}
              className="h-9 px-4 rounded-md border border-input text-sm text-foreground hover:bg-surface-hover disabled:opacity-50 transition-colors"
            >
              {claudeStatus === "checking" ? "检测中..." : "检测"}
            </button>
            {claudeStatus === "ok" && (
              <span className="text-xs text-success">{claudeInfo}</span>
            )}
            {claudeStatus === "error" && (
              <span className="text-xs text-error">{claudeInfo}</span>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add webui-v2/src/components/invite/guide-page.tsx
git commit -m "feat(webui): add GuidePage with install instructions and Claude preflight check"
```

---

### Task 8: 接入 InviteGate 到 App

**Files:**
- Modify: `webui-v2/src/app.tsx`

**Behavior:** InviteGate 包裹 SetupGate。邀请码验证 → 引导页 → 正常 setup 流程 → IM 界面。

- [ ] **Step 1: 修改 app.tsx**

Read `webui-v2/src/app.tsx` first.

In `webui-v2/src/app.tsx`, add the import at the top with other imports:

```typescript
import { InviteGate } from "./components/invite/invite-gate";
```

Then wrap the return JSX — change the return statement from:

```tsx
  return (
    <SetupGate>
      <Toaster position="top-right" richColors />
```

to:

```tsx
  return (
    <InviteGate>
      <SetupGate>
        <Toaster position="top-right" richColors />
```

And add the closing tag — change:

```tsx
      </Routes>
    </SetupGate>
  );
```

to:

```tsx
      </Routes>
      </SetupGate>
    </InviteGate>
  );
```

- [ ] **Step 2: 验证构建**

```bash
cd webui-v2 && npx tsc --noEmit
```

Expected: no type errors.

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/app.tsx
git commit -m "feat(webui): wire InviteGate to wrap SetupGate in app entry"
```

---

## Phase 3: Runtime 改造

### Task 9: Claude CLI 预检端点

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs`
- Modify: `crates/gitim-runtime/src/http.rs`

**Context:** `preflight.rs` 已有 `find_binary()` 和 `query_version()` 工具函数。`http.rs:923-949` 有 `create_router()` 函数注册所有路由。tokio 有 `full` features，包含 `process`。

- [ ] **Step 1: 在 preflight.rs 添加 check_claude()**

Read `crates/gitim-runtime/src/preflight.rs` first.

在文件末尾（`check_env()` 函数之后）添加：

```rust
/// Check if Claude CLI is available and return its version.
pub async fn check_claude() -> Result<String, String> {
    let output = tokio::process::Command::new("claude")
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("claude not found: {e}"))?;

    if !output.status.success() {
        return Err("claude --version exited with non-zero status".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout.trim().to_string();
    if version.is_empty() {
        return Err("claude --version returned empty output".to_string());
    }
    Ok(version)
}
```

- [ ] **Step 2: 在 http.rs 添加 handler 和路由**

Read `crates/gitim-runtime/src/http.rs` first.

在 `create_router()` 函数之前添加 handler 函数：

```rust
async fn preflight_claude() -> impl IntoResponse {
    match crate::preflight::check_claude().await {
        Ok(version) => Json(serde_json::json!({
            "available": true,
            "version": version,
        })),
        Err(error) => Json(serde_json::json!({
            "available": false,
            "error": error,
        })),
    }
}
```

在 `create_router()` 内的路由注册中，在 `.route("/agents/{id}", get(agents_get))` 之后添加：

```rust
        .route("/preflight/claude", get(preflight_claude))
```

> **Note:** 这个 handler 不需要 `SharedRuntimeState`，它是无状态的，直接执行 CLI 命令。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p gitim-runtime
```

Expected: compiles without errors.

- [ ] **Step 4: 手动测试**

启动 runtime 后测试：

```bash
# 如果 Claude CLI 已安装：
curl http://127.0.0.1:16868/preflight/claude
# Expected: {"available":true,"version":"..."}

# 如果未安装：
# Expected: {"available":false,"error":"claude not found: ..."}
```

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/preflight.rs crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): add /preflight/claude endpoint for Claude CLI detection"
```

---

## Phase 4: 部署配置

### Task 10: 部署准备

**Files:**
- Modify: `services/cell-api/wrangler.toml`

- [ ] **Step 1: 创建 KV namespace**

```bash
cd services/cell-api
npx wrangler kv namespace create CELL_KV
```

记录输出的 namespace ID。

```bash
npx wrangler kv namespace create CELL_KV --preview
```

记录 preview namespace ID。

- [ ] **Step 2: 更新 wrangler.toml 中的 KV ID**

将 `wrangler.toml` 中的 `id` 和 `preview_id` 替换为实际值：

```toml
[[kv_namespaces]]
binding = "CELL_KV"
id = "<production-id-from-step-1>"
preview_id = "<preview-id-from-step-1>"
```

- [ ] **Step 3: 设置生产环境 ADMIN_SECRET**

```bash
npx wrangler secret put ADMIN_SECRET
```

输入你的管理密钥（替代 wrangler.toml 中的 `changeme-in-dashboard`）。

- [ ] **Step 4: 部署 Worker**

```bash
cd services/cell-api && npm run deploy
```

记录输出的 Worker URL（类似 `https://cell-api.<your-account>.workers.dev`）。
如需自定义域名（如 `cell-api.gitim.io`），在 Cloudflare Dashboard 中配置 Custom Domain。

- [ ] **Step 5: 创建第一个邀请码验证部署**

```bash
curl -X POST https://<your-worker-url>/admin/codes \
  -H "Content-Type: application/json" \
  -H "X-Admin-Secret: <your-secret>" \
  -d '{"code":"测试口诀","note":"部署验证"}'
# Expected: {"ok":true,"invite":{...}}
```

- [ ] **Step 6: 部署 webui-v2 到 Cloudflare Pages**

在 Cloudflare Dashboard 中：

1. Pages → Create a project → Connect to Git
2. Repository: 选择 GitIM 仓库
3. Build settings:
   - Root directory: `webui-v2`
   - Build command: `npm run build`
   - Build output: `dist`
4. Environment variables:
   - `VITE_CELL_API_URL` = `https://<your-worker-url>`
5. Custom domain: `cell.gitim.io`

或者使用 CLI：

```bash
cd webui-v2 && npm run build
npx wrangler pages deploy dist --project-name cell-gitim
```

- [ ] **Step 7: Commit 部署配置**

```bash
git add services/cell-api/wrangler.toml
git commit -m "chore(cell-api): update KV namespace IDs for production"
```

---

## Phase 5: gitim.io 官网更新（可独立进行）

### Task 11: 移除 AccessForm + 添加致谢页

**Files:**
- Modify: `products/site/frontend/src/App.tsx`
- Delete: `products/site/frontend/src/components/sections/AccessForm.tsx`
- Create: `products/site/frontend/src/components/sections/Credits.tsx`

- [ ] **Step 1: 删除 AccessForm**

Read `products/site/frontend/src/App.tsx` first.

从 App.tsx 中移除 `<AccessForm />` 的引用和 import。

删除 `products/site/frontend/src/components/sections/AccessForm.tsx` 文件。

- [ ] **Step 2: 创建 Credits 组件**

Create `products/site/frontend/src/components/sections/Credits.tsx`:

```tsx
const projects = [
  { name: "Multica", url: "https://github.com/nickthecook/multica" },
  { name: "Slock", url: "https://github.com/CiferaTeam/slock" },
  { name: "Claude CLI", url: "https://docs.anthropic.com/en/docs/claude-code" },
  { name: "Codex CLI", url: "https://github.com/openai/codex" },
];

export function Credits() {
  return (
    <section className="py-20 px-6">
      <div className="max-w-3xl mx-auto text-center space-y-8">
        <h2 className="text-2xl font-bold text-foreground">致谢</h2>
        <p className="text-muted-foreground">
          GitIM 的诞生离不开以下项目
        </p>
        <div className="flex flex-wrap justify-center gap-4">
          {projects.map((p) => (
            <a
              key={p.name}
              href={p.url}
              target="_blank"
              rel="noopener noreferrer"
              className="px-4 py-2 rounded-md border border-border text-sm text-foreground hover:bg-card transition-colors"
            >
              {p.name}
            </a>
          ))}
        </div>
      </div>
    </section>
  );
}
```

> **Note:** 致谢列表中的项目名和 URL 需要你确认和补充，以上为占位示例。

- [ ] **Step 3: 在 App.tsx 中加入 Credits**

在原 `<AccessForm />` 的位置替换为 `<Credits />`，并添加对应 import。

- [ ] **Step 4: 验证构建**

```bash
cd products/site/frontend && npm run build
```

Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add products/site/frontend/
git commit -m "feat(site): remove AccessForm, add Credits section"
```

---

## API Reference

### Public Endpoints

| Method | Path | Body | Response |
|--------|------|------|----------|
| `POST` | `/api/verify` | `{ code, device_id }` | `{ ok: true }` or `{ ok: false, error }` |
| `POST` | `/api/heartbeat` | `{ code, device_id, version? }` | `{ ok: true, latest_version }` |

### Admin Endpoints (require `X-Admin-Secret` header)

| Method | Path | Body | Response |
|--------|------|------|----------|
| `GET` | `/admin/codes` | — | `{ codes: InviteCode[] }` |
| `POST` | `/admin/codes` | `{ code, note?, max_devices? }` | `{ ok, invite }` |
| `GET` | `/admin/codes/:code` | — | `InviteCode` |
| `DELETE` | `/admin/codes/:code` | — | `{ ok }` |
| `DELETE` | `/admin/codes/:code/devices/:deviceId` | — | `{ ok }` |

### Runtime Endpoints (new)

| Method | Path | Response |
|--------|------|----------|
| `GET` | `/preflight/claude` | `{ available: true, version }` or `{ available: false, error }` |

### KV Data Schema

Key: `invite:{code_string}`

```json
{
  "code": "大漠孤烟直",
  "created_at": "2026-04-15T00:00:00Z",
  "max_devices": 5,
  "note": "给 Alice",
  "devices": [
    {
      "id": "uuid-v4",
      "registered_at": "2026-04-15T10:00:00Z",
      "last_seen": "2026-04-15T12:00:00Z"
    }
  ]
}
```
