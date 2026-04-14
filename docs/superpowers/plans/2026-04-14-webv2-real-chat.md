# WebV2 Real Chat Integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace WebV2's mock IM client with real HTTP calls to the runtime API, making the chat fully functional against live daemon instances.

**Architecture:** The runtime HTTP server (`crates/gitim-runtime/src/http.rs`) already forwards most IM operations to the daemon via `GitimClient`. The frontend (`webui-v2/src/lib/client.ts`) re-exports mock functions. We add 2 missing backend endpoints, fix 1 existing endpoint, then swap all frontend IM calls from mock to real HTTP.

**Tech Stack:** Rust/axum (backend), TypeScript/React (frontend), Zustand (state)

---

### Task 1: Backend — Add `/im/users` and `/im/thread` endpoints

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:539-561` (add handlers + routes)

The runtime already has `/im/me`, `/im/channels`, `/im/send`, `/im/read`, `/im/poll`. Two endpoints are missing that the frontend needs.

- [ ] **Step 1: Add `im_users` handler**

Add after the `im_poll` function (around line 320), before the `// -- /agents/add --` comment:

```rust
// -- /im/users --

async fn im_users(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.list_users().await)
}
```

- [ ] **Step 2: Add `im_thread` handler**

Add right after `im_users`:

```rust
// -- /im/thread --

#[derive(Deserialize)]
struct ThreadRequest {
    channel: String,
    line: u64,
}

async fn im_thread(
    State(state): State<SharedRuntimeState>,
    Json(req): Json<ThreadRequest>,
) -> Json<serde_json::Value> {
    let client = match human_client(&state) {
        Ok(c) => c,
        Err(e) => return e,
    };
    api_response_to_json(client.get_thread(&req.channel, req.line).await)
}
```

- [ ] **Step 3: Register routes**

In `create_router()`, add these two lines after `.route("/im/poll", post(im_poll))`:

```rust
.route("/im/users", get(im_users))
.route("/im/thread", post(im_thread))
```

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "feat(runtime): add /im/users and /im/thread HTTP endpoints"
```

---

### Task 2: Backend — Fix `/im/me` to return handler info

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:228-234` (rewrite `im_me`)

The current `im_me` calls `client.status()` which returns `{ version, status, guest }`. The frontend needs `{ handler, display_name }`. We read this from the human repo's `.gitim/me.json`.

- [ ] **Step 1: Rewrite `im_me` handler**

Replace the existing `im_me` function with:

```rust
async fn im_me(State(state): State<SharedRuntimeState>) -> Json<serde_json::Value> {
    let human_repo = {
        let s = state.lock().unwrap();
        match &s.human_repo {
            Some(p) => p.clone(),
            None => {
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "human daemon not initialized"
                }));
            }
        }
    };

    let me_path = human_repo.join(".gitim/me.json");
    match std::fs::read_to_string(&me_path) {
        Ok(content) => {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(me) => Json(serde_json::json!({
                    "ok": true,
                    "data": {
                        "handler": me.get("handler").and_then(|v| v.as_str()).unwrap_or("unknown"),
                        "display_name": me.get("display_name").and_then(|v| v.as_str()).unwrap_or("Unknown"),
                        "guest": me.get("guest").and_then(|v| v.as_bool()).unwrap_or(false),
                    }
                })),
                Err(e) => Json(serde_json::json!({
                    "ok": false,
                    "error": format!("failed to parse me.json: {e}")
                })),
            }
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("failed to read me.json: {e}")
        })),
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-runtime/src/http.rs
git commit -m "fix(runtime): /im/me reads handler from me.json instead of daemon status"
```

---

### Task 3: Frontend — Replace mock IM methods in client.ts

**Files:**
- Modify: `webui-v2/src/lib/client.ts`

Replace the 7 mock IM re-exports with real HTTP calls to the runtime `/im/*` endpoints. Keep the mock import only as agent API fallback.

- [ ] **Step 1: Replace IM methods**

Replace lines 1-17 of `client.ts` (the mock import + re-exports) with real implementations:

```typescript
/**
 * Unified client — all methods hit the real runtime HTTP API.
 * Agent methods fall back to mock if runtime is unreachable.
 */
import type { Agent, ApiResponse } from "./types";
import * as mockClient from "./mock/client";
import { useConnectionStore } from "@/hooks/use-connection-store";
```

Then replace the mock re-exports (lines 9-17) with:

```typescript
// --- IM methods: real runtime HTTP ---

export async function me(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/me`);
  return await res.json();
}

export async function poll(since?: string): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/poll`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ since }),
  });
  return await res.json();
}

export async function channels(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/channels`);
  return await res.json();
}

export async function send(
  channel: string,
  body: string,
  author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/send`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, body, reply_to: replyTo }),
  });
  return await res.json();
}

export async function read(
  channel: string,
  limit?: number,
): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/read`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, limit }),
  });
  return await res.json();
}

export async function thread(
  channel: string,
  line: number,
): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/thread`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, line }),
  });
  return await res.json();
}

export async function users(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/users`);
  return await res.json();
}
```

Note: `author` param in `send()` is intentionally not sent to the backend — the daemon resolves the author from its own identity (`current_user`). The mock needed it because it had no identity concept.

- [ ] **Step 2: Verify TypeScript compiles**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | head -20`
Expected: no errors (or only pre-existing ones unrelated to our changes).

- [ ] **Step 3: Commit**

```bash
git add webui-v2/src/lib/client.ts
git commit -m "feat(webui): replace mock IM client with real runtime HTTP calls"
```

---

### Task 4: Frontend — Update app.tsx to use real client

**Files:**
- Modify: `webui-v2/src/app.tsx`

Replace all `mockClient.*` calls with `client.*` calls. Remove mock timer import and usage.

- [ ] **Step 1: Remove mock imports**

Remove these two lines:

```typescript
import * as mockClient from "./lib/mock/client";
import { startMockTimer, stopMockTimer } from "./lib/mock/timer";
```

- [ ] **Step 2: Replace mockClient calls in `runPoll`**

In `runPoll` callback (line 57-106), replace:
- `mockClient.poll(sinceRef.current)` → `client.poll(sinceRef.current)`
- `mockClient.read(apiCh)` → `client.read(apiCh)`
- `mockClient.channels()` → `client.channels()`

These are on lines 58, 83, 93. All become `client.*` calls — `client` is already imported on line 10.

- [ ] **Step 3: Replace mockClient calls in `init`**

In the `init` function (line 111-127), replace:
- `mockClient.me()` → `client.me()`
- `mockClient.channels()` → `client.channels()`
- `mockClient.users()` → `client.users()`

Remove the `startMockTimer()` call (line 128).

- [ ] **Step 4: Remove mock timer cleanup**

In the cleanup function (line 134-137), remove the `stopMockTimer()` call.

- [ ] **Step 5: Verify TypeScript compiles**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | head -20`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add webui-v2/src/app.tsx
git commit -m "feat(webui): app.tsx uses real client for poll and init"
```

---

### Task 5: Frontend — Update chat-layout.tsx to use real client

**Files:**
- Modify: `webui-v2/src/components/chat/chat-layout.tsx`

Replace all `mockClient.*` calls with `client.*`. Remove mock import.

- [ ] **Step 1: Replace import**

Replace:
```typescript
import * as mockClient from "../../lib/mock/client";
```

With:
```typescript
import * as client from "../../lib/client";
```

- [ ] **Step 2: Replace all mockClient calls**

There are 5 call sites to replace:

1. **`handleChannelSelect`** (line 52): `mockClient.read(apiChannel)` → `client.read(apiChannel)`

2. **`handleStartDm`** (line 69): Remove the `mockClient.addChannel(newChannel)` call entirely. The store update on line 71 (`setChannels([...channels, newChannel])`) is sufficient — the DM will be created on the backend when the first message is sent.

3. **`handleSend`** (line 97-101): `mockClient.send(apiChannel, body, currentUser, pointTo)` → `client.send(apiChannel, body, currentUser, pointTo)`

4. **`handleShowThread`** (line 124): `mockClient.thread(apiChannel, msg.line_number)` → `client.thread(apiChannel, msg.line_number)`

5. **`handleNavBack`** (line 190): `mockClient.read(apiChannel)` → `client.read(apiChannel)`

- [ ] **Step 3: Verify TypeScript compiles**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | head -20`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add webui-v2/src/components/chat/chat-layout.tsx
git commit -m "feat(webui): chat-layout uses real client instead of mock"
```

---

### Task 6: Build verification

**Files:** None (verification only)

- [ ] **Step 1: Full Rust build**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 2: Full TypeScript check**

Run: `cd webui-v2 && npx tsc --noEmit 2>&1 | head -20`
Expected: no errors.

- [ ] **Step 3: Verify no remaining mock references in production code**

Run: `grep -rn "mockClient\|mock/client\|mockTimer\|mock/timer" webui-v2/src/ --include="*.ts" --include="*.tsx" | grep -v "mock/"` 

Expected: Only `client.ts` importing mock as agent API fallback. No references in `app.tsx` or `chat-layout.tsx`.

- [ ] **Step 4: Final commit (if any cleanup needed)**
