# E2E Startup Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the Runtime HTTP shell mode, WebV2 connection/workspace setup flow, and the first Playwright E2E test covering the full startup sequence.

**Architecture:** Runtime gains a minimal Axum HTTP server (`GET /health`, `POST /workspace`) that starts without a workspace. WebV2 gains a connection gate: on load it reads a stored port from localStorage, tries `/health`, shows a port input form if unreachable, then a workspace path form. E2E test compiles the runtime binary, launches it, opens the browser, walks through both forms, and asserts the workspace marker file on disk.

**Tech Stack:** Rust/Axum (runtime HTTP), React/Zustand (frontend connection flow), Playwright (E2E)

---

## File Structure

### Rust (Runtime HTTP server)

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/gitim-runtime/src/http.rs` | Axum router: `GET /health`, `POST /workspace` |
| Modify | `crates/gitim-runtime/src/bin/runtime.rs` | Add `--port` flag, start HTTP server in shell mode |
| Modify | `crates/gitim-runtime/src/lib.rs` | Export `http` module |
| Modify | `crates/gitim-runtime/Cargo.toml` | Add axum, tower-http (cors) dependencies |

### Frontend (Connection flow)

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `webui-v2/src/hooks/use-connection-store.ts` | Zustand store: port, status, workspacePath |
| Create | `webui-v2/src/components/setup/connect-form.tsx` | Port input UI + health check |
| Create | `webui-v2/src/components/setup/workspace-form.tsx` | Workspace path input UI |
| Create | `webui-v2/src/components/setup/setup-gate.tsx` | Gate component: renders setup forms or children |
| Modify | `webui-v2/src/app.tsx` | Wrap routes in SetupGate |

### E2E

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `e2e/package.json` | Playwright + dependencies |
| Create | `e2e/playwright.config.ts` | Test configuration |
| Create | `e2e/tests/startup.spec.ts` | Startup flow E2E test |

---

## Task 1: Runtime HTTP server — health endpoint

**Files:**
- Modify: `crates/gitim-runtime/Cargo.toml`
- Create: `crates/gitim-runtime/src/http.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`

- [ ] **Step 1: Add dependencies to Cargo.toml**

Add axum, tower-http, and serde_json to `crates/gitim-runtime/Cargo.toml`:

```toml
# Add under [dependencies]:
axum = "0.8"
tower-http = { version = "0.6", features = ["cors"] }
```

- [ ] **Step 2: Create `http.rs` with health endpoint**

```rust
// crates/gitim-runtime/src/http.rs
use axum::{routing::get, Json, Router};
use serde::Serialize;
use tower_http::cors::CorsLayer;

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub fn create_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .layer(CorsLayer::permissive())
}
```

- [ ] **Step 3: Export http module from lib.rs**

Add to `crates/gitim-runtime/src/lib.rs`:

```rust
pub mod http;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p gitim-runtime`
Expected: success, no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/
git commit -m "feat(runtime): add minimal HTTP server with health endpoint"
```

---

## Task 2: Runtime binary — shell mode with `--port`

**Files:**
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`

- [ ] **Step 1: Rewrite runtime.rs to support `--port` shell mode**

The binary now has two modes:
- `gitim-runtime --port 7890` — shell mode: HTTP server only, no agent
- `gitim-runtime <remote_url> <handler> <display_name> [agents_dir]` — legacy agent mode (preserved)

```rust
// crates/gitim-runtime/src/bin/runtime.rs
use std::net::SocketAddr;
use std::path::PathBuf;

use gitim_runtime::{provision_agent, AgentConfig, AgentLoop};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();

    // Shell mode: gitim-runtime --port <PORT>
    if args.len() >= 3 && args[1] == "--port" {
        let port: u16 = args[2].parse().expect("invalid port number");
        return run_shell(port).await;
    }

    // Legacy agent mode: gitim-runtime <remote_url> <handler> <display_name> [agents_dir]
    if args.len() < 4 {
        eprintln!("Usage:");
        eprintln!("  gitim-runtime --port <PORT>                              (shell mode)");
        eprintln!("  gitim-runtime <remote_url> <handler> <display_name> [agents_dir]  (agent mode)");
        std::process::exit(1);
    }

    let remote_url = &args[1];
    let handler = &args[2];
    let display_name = &args[3];
    let agents_dir = if args.len() > 4 {
        PathBuf::from(&args[4])
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitim-agents")
    };

    std::fs::create_dir_all(&agents_dir)?;

    eprintln!("provisioning agent @{handler} ...");
    let config = AgentConfig {
        handler: handler.clone(),
        display_name: display_name.clone(),
        remote_url: remote_url.clone(),
    };
    let handle = provision_agent(&agents_dir, &config).await?;
    eprintln!("agent ready at {}", handle.repo_root.display());

    eprintln!("starting agent loop (ctrl-c to stop) ...");
    let mut agent_loop = AgentLoop::with_defaults(&handle.repo_root)?;
    agent_loop.run().await?;

    Ok(())
}

async fn run_shell(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let router = gitim_runtime::http::create_router();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
```

- [ ] **Step 2: Verify shell mode starts and responds**

Run in one terminal:
```bash
cargo run -p gitim-runtime -- --port 7891
```
Expected: `runtime shell listening on http://127.0.0.1:7891`

Run in another terminal:
```bash
curl -s http://127.0.0.1:7891/health | python3 -m json.tool
```
Expected:
```json
{
    "service": "gitim-runtime",
    "version": "0.3.1"
}
```

Kill the runtime process after verification.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/src/bin/runtime.rs
git commit -m "feat(runtime): add --port shell mode for HTTP-only startup"
```

---

## Task 3: Runtime HTTP — workspace endpoint

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`

- [ ] **Step 1: Add shared state and workspace endpoint**

```rust
// crates/gitim-runtime/src/http.rs
use axum::{extract::State, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;

#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
}

#[derive(Deserialize)]
struct WorkspaceRequest {
    path: String,
}

#[derive(Serialize)]
struct ApiOk {
    ok: bool,
}

#[derive(Serialize)]
struct ApiError {
    ok: bool,
    error: String,
}

#[derive(Default)]
pub struct RuntimeState {
    pub workspace: Option<PathBuf>,
}

pub type SharedRuntimeState = Arc<Mutex<RuntimeState>>;

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn set_workspace(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<WorkspaceRequest>,
) -> Json<serde_json::Value> {
    let path = PathBuf::from(&req.path);

    if !path.is_dir() {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("directory does not exist: {}", req.path)
        }));
    }

    // Create marker directory and write config
    let marker_dir = path.join(".gitim-runtime");
    if let Err(e) = std::fs::create_dir_all(&marker_dir) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to create .gitim-runtime: {e}")
        }));
    }

    let config = serde_json::json!({
        "workspace": req.path,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    let config_path = marker_dir.join("config.json");
    if let Err(e) = std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()) {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to write config: {e}")
        }));
    }

    let mut s = state.lock().unwrap();
    s.workspace = Some(path);

    Json(serde_json::json!({ "ok": true }))
}

pub fn create_router() -> Router {
    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));

    Router::new()
        .route("/health", get(health))
        .route("/workspace", post(set_workspace))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
```

- [ ] **Step 2: Add chrono dependency to Cargo.toml**

Add to `crates/gitim-runtime/Cargo.toml` under `[dependencies]`:

```toml
chrono = { workspace = true }
```

- [ ] **Step 3: Verify workspace endpoint**

Start runtime:
```bash
cargo run -p gitim-runtime -- --port 7891
```

Create a temp directory and test:
```bash
mkdir -p /tmp/test-workspace
curl -s -X POST http://127.0.0.1:7891/workspace \
  -H 'Content-Type: application/json' \
  -d '{"path": "/tmp/test-workspace"}' | python3 -m json.tool
```
Expected: `{ "ok": true }`

Verify marker file:
```bash
cat /tmp/test-workspace/.gitim-runtime/config.json | python3 -m json.tool
```
Expected: JSON with `workspace` and `created_at` fields.

Test with nonexistent directory:
```bash
curl -s -X POST http://127.0.0.1:7891/workspace \
  -H 'Content-Type: application/json' \
  -d '{"path": "/tmp/nonexistent-dir-xyz"}' | python3 -m json.tool
```
Expected: `{ "ok": false, "error": "directory does not exist: ..." }`

Clean up and kill runtime.

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime/
git commit -m "feat(runtime): add POST /workspace endpoint with marker file creation"
```

---

## Task 4: Frontend — connection store

**Files:**
- Create: `webui-v2/src/hooks/use-connection-store.ts`

- [ ] **Step 1: Create the connection store**

```typescript
// webui-v2/src/hooks/use-connection-store.ts
import { create } from "zustand";

export type ConnectionStatus =
  | "checking"     // trying stored port
  | "disconnected" // no runtime found, show port form
  | "connected"    // health OK, need workspace
  | "ready";       // workspace set, app can proceed

const STORAGE_KEY = "gitim-runtime-port";

interface ConnectionState {
  status: ConnectionStatus;
  port: number | null;
  runtimeVersion: string | null;
  workspacePath: string | null;
  error: string | null;

  setStatus: (s: ConnectionStatus) => void;
  setPort: (p: number) => void;
  setRuntimeVersion: (v: string) => void;
  setWorkspacePath: (p: string) => void;
  setError: (e: string | null) => void;
  baseUrl: () => string;
}

function loadStoredPort(): number | null {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  status: "checking",
  port: loadStoredPort(),
  runtimeVersion: null,
  workspacePath: null,
  error: null,

  setStatus: (s) => set({ status: s, error: null }),
  setPort: (p) => {
    localStorage.setItem(STORAGE_KEY, String(p));
    set({ port: p });
  },
  setRuntimeVersion: (v) => set({ runtimeVersion: v }),
  setWorkspacePath: (p) => set({ workspacePath: p }),
  setError: (e) => set({ error: e }),
  baseUrl: () => `http://127.0.0.1:${get().port}`,
}));
```

- [ ] **Step 2: Verify it compiles**

Run: `cd webui-v2 && npx tsc --noEmit`
Expected: no type errors.

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/hooks/use-connection-store.ts
git commit -m "feat(webui-v2): add connection store for runtime port and status"
```

---

## Task 5: Frontend — connect form component

**Files:**
- Create: `webui-v2/src/components/setup/connect-form.tsx`

- [ ] **Step 1: Create connect form**

```tsx
// webui-v2/src/components/setup/connect-form.tsx
import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

export function ConnectForm() {
  const port = useConnectionStore((s) => s.port);
  const error = useConnectionStore((s) => s.error);
  const setPort = useConnectionStore((s) => s.setPort);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);
  const setError = useConnectionStore((s) => s.setError);

  const [input, setInput] = useState(port?.toString() ?? "");
  const [checking, setChecking] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const p = parseInt(input, 10);
    if (!Number.isFinite(p) || p < 1 || p > 65535) {
      setError("Please enter a valid port (1-65535)");
      return;
    }

    setChecking(true);
    setError(null);

    try {
      const res = await fetch(`http://127.0.0.1:${p}/health`, {
        signal: AbortSignal.timeout(3000),
      });
      const data = await res.json();

      if (data.service !== "gitim-runtime") {
        setError("Connected, but service is not gitim-runtime");
        return;
      }

      setPort(p);
      setRuntimeVersion(data.version ?? null);
      setStatus("connected");
    } catch {
      setError(`Cannot reach runtime at port ${p}. Is it running?`);
    } finally {
      setChecking(false);
    }
  }

  return (
    <div className="flex flex-col items-center justify-center h-screen bg-background text-foreground">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-xl font-bold tracking-tight">GitIM</h1>
          <p className="text-sm text-muted-foreground">
            Connect to a running Runtime instance
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <label
              htmlFor="port-input"
              className="text-xs font-medium text-text-secondary"
            >
              Runtime Port
            </label>
            <input
              id="port-input"
              data-testid="port-input"
              type="text"
              inputMode="numeric"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="7890"
              className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm font-mono placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
              autoFocus
            />
          </div>

          {error && (
            <p data-testid="connect-error" className="text-xs text-error">
              {error}
            </p>
          )}

          <button
            data-testid="connect-button"
            type="submit"
            disabled={checking}
            className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
          >
            {checking ? "Connecting..." : "Connect"}
          </button>
        </form>

        <p className="text-xs text-text-muted text-center leading-relaxed">
          Start the runtime first:{" "}
          <code className="text-text-secondary">
            gitim-runtime --port 7890
          </code>
        </p>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd webui-v2 && npx tsc --noEmit`
Expected: no type errors.

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/components/setup/connect-form.tsx
git commit -m "feat(webui-v2): add connect form for runtime port input"
```

---

## Task 6: Frontend — workspace form component

**Files:**
- Create: `webui-v2/src/components/setup/workspace-form.tsx`

- [ ] **Step 1: Create workspace form**

```tsx
// webui-v2/src/components/setup/workspace-form.tsx
import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

export function WorkspaceForm() {
  const baseUrl = useConnectionStore((s) => s.baseUrl);
  const runtimeVersion = useConnectionStore((s) => s.runtimeVersion);
  const setWorkspacePath = useConnectionStore((s) => s.setWorkspacePath);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);

  const [input, setInput] = useState("");
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const path = input.trim();
    if (!path) {
      setError("Please enter a workspace path");
      return;
    }

    setSubmitting(true);
    setError(null);

    try {
      const res = await fetch(`${baseUrl()}/workspace`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path }),
      });
      const data = await res.json();

      if (!data.ok) {
        setError(data.error ?? "Failed to set workspace");
        return;
      }

      setWorkspacePath(path);
      setStatus("ready");
    } catch {
      setError("Failed to connect to runtime");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex flex-col items-center justify-center h-screen bg-background text-foreground">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-xl font-bold tracking-tight">GitIM</h1>
          <p className="text-sm text-muted-foreground">
            Set a workspace directory for this session
          </p>
          {runtimeVersion && (
            <p className="text-xs text-text-muted">
              Runtime v{runtimeVersion}
            </p>
          )}
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <label
              htmlFor="workspace-input"
              className="text-xs font-medium text-text-secondary"
            >
              Workspace Path
            </label>
            <input
              id="workspace-input"
              data-testid="workspace-input"
              type="text"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="/path/to/workspace"
              className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm font-mono placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
              autoFocus
            />
          </div>

          {error && (
            <p data-testid="workspace-error" className="text-xs text-error">
              {error}
            </p>
          )}

          <button
            data-testid="workspace-button"
            type="submit"
            disabled={submitting}
            className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
          >
            {submitting ? "Setting up..." : "Open Workspace"}
          </button>
        </form>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd webui-v2 && npx tsc --noEmit`
Expected: no type errors.

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/components/setup/workspace-form.tsx
git commit -m "feat(webui-v2): add workspace form for directory selection"
```

---

## Task 7: Frontend — setup gate + app integration

**Files:**
- Create: `webui-v2/src/components/setup/setup-gate.tsx`
- Modify: `webui-v2/src/app.tsx`

- [ ] **Step 1: Create setup gate component**

```tsx
// webui-v2/src/components/setup/setup-gate.tsx
import { useEffect, type ReactNode } from "react";
import {
  useConnectionStore,
  type ConnectionStatus,
} from "../../hooks/use-connection-store";
import { ConnectForm } from "./connect-form";
import { WorkspaceForm } from "./workspace-form";

interface SetupGateProps {
  children: ReactNode;
}

export function SetupGate({ children }: SetupGateProps) {
  const status = useConnectionStore((s) => s.status);
  const port = useConnectionStore((s) => s.port);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);

  // On mount: if we have a stored port, try to connect automatically
  useEffect(() => {
    if (status !== "checking") return;
    if (!port) {
      setStatus("disconnected");
      return;
    }

    let cancelled = false;

    async function tryConnect() {
      try {
        const res = await fetch(`http://127.0.0.1:${port}/health`, {
          signal: AbortSignal.timeout(3000),
        });
        const data = await res.json();
        if (cancelled) return;

        if (data.service === "gitim-runtime") {
          setRuntimeVersion(data.version ?? null);
          setStatus("connected");
        } else {
          setStatus("disconnected");
        }
      } catch {
        if (!cancelled) setStatus("disconnected");
      }
    }

    tryConnect();
    return () => { cancelled = true; };
  }, [status, port, setStatus, setRuntimeVersion]);

  const screens: Record<ConnectionStatus, ReactNode> = {
    checking: (
      <div className="flex items-center justify-center h-screen bg-background text-muted-foreground text-sm">
        Connecting...
      </div>
    ),
    disconnected: <ConnectForm />,
    connected: <WorkspaceForm />,
    ready: <>{children}</>,
  };

  return <>{screens[status]}</>;
}
```

- [ ] **Step 2: Wrap app routes in SetupGate**

Modify `webui-v2/src/app.tsx`. Replace the import section and the `App` component's return statement.

Add import at top:
```typescript
import { SetupGate } from "./components/setup/setup-gate";
```

Wrap the `<Routes>` block in the return statement:
```tsx
  return (
    <SetupGate>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/management" replace />} />
          <Route path="/management" element={<ManagementPage />} />
          <Route path="/management/:agentId" element={<AgentDetail />} />
          <Route path="/chat" element={<ChatPage />} />
        </Route>
      </Routes>
    </SetupGate>
  );
```

- [ ] **Step 3: Verify it compiles and renders**

Run: `cd webui-v2 && npx tsc --noEmit`
Expected: no type errors.

Run: `cd webui-v2 && npx vite --port 5174`
Open browser at `http://localhost:5174` — should see the ConnectForm (since no runtime is running).

- [ ] **Step 4: Commit**

```bash
git add webui-v2/src/components/setup/setup-gate.tsx webui-v2/src/app.tsx
git commit -m "feat(webui-v2): add setup gate — connect then set workspace before app loads"
```

---

## Task 8: E2E scaffold — Playwright setup

**Files:**
- Create: `e2e/package.json`
- Create: `e2e/playwright.config.ts`
- Create: `e2e/tsconfig.json`

- [ ] **Step 1: Create e2e/package.json**

```json
{
  "name": "gitim-e2e",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "playwright test",
    "test:headed": "playwright test --headed"
  },
  "devDependencies": {
    "@playwright/test": "^1.52.0"
  }
}
```

- [ ] **Step 2: Create e2e/tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["**/*.ts"]
}
```

- [ ] **Step 3: Create e2e/playwright.config.ts**

```typescript
// e2e/playwright.config.ts
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  retries: 0,
  use: {
    baseURL: "http://localhost:5173",
    headless: true,
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});
```

- [ ] **Step 4: Install dependencies and Playwright browsers**

```bash
cd e2e && npm install && npx playwright install chromium
```

- [ ] **Step 5: Commit**

```bash
git add e2e/
git commit -m "chore(e2e): scaffold Playwright test infrastructure"
```

---

## Task 9: E2E test — startup flow

**Files:**
- Create: `e2e/tests/startup.spec.ts`

- [ ] **Step 1: Write the E2E test**

This test:
1. Builds the runtime binary (or uses a pre-built one)
2. Starts runtime on a random port
3. Starts webui-v2 dev server
4. Opens browser — sees connect form
5. Enters port — connects — sees workspace form
6. Enters workspace path — submits — runtime creates marker file
7. Asserts marker file exists on disk
8. Tears down everything

```typescript
// e2e/tests/startup.spec.ts
import { test, expect } from "@playwright/test";
import { execSync, spawn, type ChildProcess } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as net from "node:net";

const ROOT = path.resolve(__dirname, "../..");
const WEBUI_DIR = path.join(ROOT, "webui-v2");

/** Find a free port on localhost. */
async function freePort(): Promise<number> {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address() as net.AddressInfo;
      srv.close(() => resolve(addr.port));
    });
  });
}

/** Wait until an HTTP endpoint responds with expected JSON field. */
async function waitForHealth(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      const data = await res.json();
      if (data.service === "gitim-runtime") return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Runtime did not become healthy at ${url}`);
}

/** Wait until an HTTP endpoint responds (any 200). */
async function waitForHttp(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (res.ok) return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Server did not become available at ${url}`);
}

test.describe("startup flow", () => {
  let runtimeProc: ChildProcess;
  let viteProc: ChildProcess;
  let runtimePort: number;
  let vitePort: number;
  let workspaceDir: string;

  test.beforeAll(async () => {
    // 1. Build runtime binary
    execSync("cargo build -p gitim-runtime", {
      cwd: ROOT,
      stdio: "inherit",
    });

    // 2. Create temp workspace directory
    workspaceDir = fs.mkdtempSync(path.join(os.tmpdir(), "gitim-e2e-"));

    // 3. Start runtime on a free port
    runtimePort = await freePort();
    const runtimeBin = path.join(ROOT, "target/debug/gitim-runtime");
    runtimeProc = spawn(runtimeBin, ["--port", String(runtimePort)], {
      stdio: "pipe",
    });

    // 4. Start webui-v2 dev server on a free port
    vitePort = await freePort();
    viteProc = spawn("npx", ["vite", "--port", String(vitePort), "--strictPort"], {
      cwd: WEBUI_DIR,
      stdio: "pipe",
      env: { ...process.env, BROWSER: "none" },
    });

    // 5. Wait for both to be ready
    await Promise.all([
      waitForHealth(`http://127.0.0.1:${runtimePort}/health`),
      waitForHttp(`http://127.0.0.1:${vitePort}`),
    ]);
  });

  test.afterAll(() => {
    runtimeProc?.kill();
    viteProc?.kill();

    // Clean up workspace
    if (workspaceDir && fs.existsSync(workspaceDir)) {
      fs.rmSync(workspaceDir, { recursive: true, force: true });
    }
  });

  test("connect to runtime and set workspace", async ({ page }) => {
    // Clear any stored port from previous runs
    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Should see connect form
    await expect(page.getByTestId("port-input")).toBeVisible();

    // Enter runtime port
    await page.getByTestId("port-input").fill(String(runtimePort));
    await page.getByTestId("connect-button").click();

    // Should transition to workspace form
    await expect(page.getByTestId("workspace-input")).toBeVisible({
      timeout: 5000,
    });

    // Enter workspace path
    await page.getByTestId("workspace-input").fill(workspaceDir);
    await page.getByTestId("workspace-button").click();

    // Should transition to the main app (any element from AppShell)
    // The AppShell has a "GitIM" header text — wait for it
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 5000,
    });

    // Verify marker file was created on disk
    const configPath = path.join(workspaceDir, ".gitim-runtime", "config.json");
    expect(fs.existsSync(configPath)).toBe(true);

    const config = JSON.parse(fs.readFileSync(configPath, "utf-8"));
    expect(config.workspace).toBe(workspaceDir);
    expect(config.created_at).toBeDefined();
  });

  test("shows error for invalid port", async ({ page }) => {
    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    await expect(page.getByTestId("port-input")).toBeVisible();

    // Enter a port where nothing is running
    const deadPort = await freePort();
    await page.getByTestId("port-input").fill(String(deadPort));
    await page.getByTestId("connect-button").click();

    // Should show error
    await expect(page.getByTestId("connect-error")).toBeVisible({
      timeout: 5000,
    });
  });

  test("shows error for nonexistent workspace path", async ({ page }) => {
    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Connect first
    await page.getByTestId("port-input").fill(String(runtimePort));
    await page.getByTestId("connect-button").click();
    await expect(page.getByTestId("workspace-input")).toBeVisible({
      timeout: 5000,
    });

    // Enter nonexistent path
    await page.getByTestId("workspace-input").fill("/tmp/does-not-exist-xyz-123");
    await page.getByTestId("workspace-button").click();

    // Should show error
    await expect(page.getByTestId("workspace-error")).toBeVisible({
      timeout: 5000,
    });
  });
});
```

- [ ] **Step 2: Run the E2E test**

```bash
cd e2e && npx playwright test --headed
```

Expected: all 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add e2e/tests/startup.spec.ts
git commit -m "test(e2e): add startup flow tests — connect + workspace setup"
```
