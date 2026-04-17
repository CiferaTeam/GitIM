# Implementation Plan: Frontend Incremental Data Flow

Spec: `docs/specs/2026-04-14-frontend-incremental-data-flow.md`

## Step 1: Backend — health endpoint returns workspace

**File:** `crates/gitim-runtime/src/http.rs`

Add `workspace` field to `HealthResponse`:

```rust
#[derive(Serialize)]
struct HealthResponse {
    service: &'static str,
    version: &'static str,
    initialized: bool,
    workspace: Option<String>,  // NEW
}
```

In `health()` handler, populate from state:

```rust
async fn health(State(state): State<SharedRuntimeState>) -> Json<HealthResponse> {
    let s = state.lock().unwrap();
    let initialized = s.workspace.is_some() && s.human_repo.is_some();
    let workspace = s.workspace.as_ref().map(|p| p.to_string_lossy().into_owned());
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
        initialized,
        workspace,
    })
}
```

**Verify:** `cargo build -p gitim-runtime` compiles.

## Step 2: Frontend — update PollChange type to include entries

**File:** `webui-v2/src/lib/types.ts`

```typescript
export interface PollChange {
  channel: string;
  kind: string;
  entries?: Message[];  // NEW — poll returns full entries
}
```

## Step 3: Frontend — add health() client method

**File:** `webui-v2/src/lib/client.ts`

Add:

```typescript
export async function health(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/health`);
  return await res.json();
}
```

## Step 4: Frontend — cursor persistence helpers

**File:** `webui-v2/src/lib/cursor.ts` (new)

```typescript
function workspaceToKey(workspace: string): string {
  return "gitim:cursor:" + workspace.replace(/\//g, "-");
}

export function loadCursor(workspace: string): string | undefined {
  const key = workspaceToKey(workspace);
  return localStorage.getItem(key) ?? undefined;
}

export function saveCursor(workspace: string, commitId: string): void {
  const key = workspaceToKey(workspace);
  localStorage.setItem(key, commitId);
}

export function clearCursor(workspace: string): void {
  const key = workspaceToKey(workspace);
  localStorage.removeItem(key);
}
```

## Step 5: Frontend — overhaul App.tsx poll loop

**File:** `webui-v2/src/app.tsx`

Changes:
1. Add `workspaceRef` to store workspace path
2. In `init()`: call `health()` → extract workspace → `loadCursor()` → set `sinceRef`
3. In `runPoll()`:
   - After getting new commit_id: `saveCursor(workspace, commitId)`
   - Replace `client.read()` with direct `addMessages(change.entries)` for active channel
   - On poll error: `clearCursor()` → `poll(since=undefined)` to reset

Key diff in `runPoll`:

```typescript
// BEFORE:
if (displayName === currentChannelRef.current) {
  const readRes = await client.read(apiCh);
  if (readRes.ok && readRes.data) {
    setMessages(readRes.data.entries as Message[]);
  }
}

// AFTER:
if (displayName === currentChannelRef.current) {
  if (change.entries?.length) {
    addMessages(change.entries as Message[]);
  }
}
```

Key diff in `init`:

```typescript
// BEFORE:
// sinceRef starts as undefined, no persistence

// AFTER:
const healthRes = await client.health();
if (healthRes.ok && healthRes.data?.workspace) {
  workspaceRef.current = healthRes.data.workspace as string;
  sinceRef.current = loadCursor(workspaceRef.current);
}
```

## Step 6: Frontend — read with limit=50

**File:** `webui-v2/src/components/chat/chat-layout.tsx`

Two locations call `client.read()` without limit:

1. `handleChannelSelect` (line 52): `client.read(apiChannel)` → `client.read(apiChannel, 50)`
2. `handleNavBack` (line 189): `client.read(apiChannel)` → `client.read(apiChannel, 50)`
