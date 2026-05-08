# Mobile WASM Workspaces Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Browser mode supports multiple persistent mobile workspaces, refresh-safe session tokens, cache reset, and reconnect flows without forcing a fresh clone.

**Architecture:** Add a browser workspace registry keyed by immutable `workspaceId`, isolate each new workspace in its own LightningFS namespace, and keep the legacy `gitim:/repo` cache as a migrated workspace. Local mode uses one active worker session at a time; switching workspaces terminates the previous worker and starts a new session for the selected workspace.

**Tech Stack:** React 19, Zustand, TypeScript, Web Worker RPC, isomorphic-git, `@isomorphic-git/lightning-fs`, Vitest, Playwright.

---

## File Structure

- Create: `products/gitim/frontend/src/lib/workspace-key.ts`
  - Owns persistent UI storage keys for runtime and browser workspaces.
- Create: `products/gitim/frontend/src/lib/workspace-key.test.ts`
  - Verifies cursor/active-key stability and legacy slug compatibility.
- Create: `products/gitim/frontend/src/lib/browser-workspaces.ts`
  - Owns browser workspace registry v2, legacy migration, session token storage, and cache wipe helpers.
- Create: `products/gitim/frontend/src/lib/browser-workspaces.test.ts`
  - Covers registry CRUD, legacy migration, session token isolation, reset, forget, and start-over.
- Modify: `products/gitim/frontend/src/lib/types.ts`
  - Adds optional browser identity fields to `WorkspaceSummary`.
- Modify: `products/gitim/frontend/src/lib/cursor.ts`
  - Uses the new workspace storage key helper.
- Modify: `products/gitim/frontend/src/daemon-web/storage.ts`
  - Makes LightningFS configurable per worker session and supports namespace wipe.
- Modify: `products/gitim/frontend/src/daemon-web/state.ts`
  - Adds `workspaceId`, `remoteUrl`, `fsName`, and optional token state.
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
  - Carries `workspaceId` and `generation` in requests, responses, and sync events.
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
  - Initializes from configured storage, supports offline cached reads, and returns reconnect-required errors for sync/send without token.
- Modify: `products/gitim/frontend/src/daemon-web/sync.ts`
  - Treats missing token as sync disabled and leaves current cache intact.
- Modify: `products/gitim/frontend/src/daemon-web/git.ts`
  - Uses the configured FS from `storage.ts`; no API shape change is expected.
- Modify: `products/gitim/frontend/src/daemon-web/handlers.test.ts`
  - Adds cached offline and reconnect-required coverage.
- Modify: `products/gitim/frontend/src/lib/backend.ts`
  - Adds LocalBackend workspace session identity, stale response guards, token update, and explicit shutdown.
- Modify: `products/gitim/frontend/src/lib/client.ts`
  - Implements browser-mode workspace CRUD from registry and local workspace activation.
- Modify: `products/gitim/frontend/src/hooks/use-workspace-store.ts`
  - Persists the active workspace using mode-aware storage keys.
- Create: `products/gitim/frontend/src/components/setup/browser-workspace-form.tsx`
  - Reusable browser workspace create/reconnect form.
- Modify: `products/gitim/frontend/src/components/setup/local-setup.tsx`
  - Shows existing browser workspaces, auto-restores sessionStorage tokens, and supports cached offline open.
- Modify: `products/gitim/frontend/src/components/workspace/workspace-switcher.tsx`
  - Uses browser-mode create/reconnect/reset/forget/start-over actions.
- Modify: `products/gitim/frontend/src/app.tsx`
  - Activates the selected browser workspace and resets local backend on switch.
- Modify: `products/gitim/frontend/e2e/mobile-layout.spec.ts`
  - Adds refresh, multi-workspace, and cache-management mobile coverage.

---

### Task 1: Workspace Keys And Browser Registry

**Files:**
- Create: `products/gitim/frontend/src/lib/workspace-key.ts`
- Create: `products/gitim/frontend/src/lib/workspace-key.test.ts`
- Create: `products/gitim/frontend/src/lib/browser-workspaces.ts`
- Create: `products/gitim/frontend/src/lib/browser-workspaces.test.ts`
- Modify: `products/gitim/frontend/src/lib/types.ts`
- Modify: `products/gitim/frontend/src/lib/cursor.ts`

- [ ] **Step 1: Write failing tests for mode-aware workspace keys**

Create `products/gitim/frontend/src/lib/workspace-key.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import {
  activeWorkspaceStorageKey,
  cursorWorkspaceKey,
  workspaceIdentity,
} from "./workspace-key";
import type { WorkspaceSummary } from "./types";

describe("workspace keys", () => {
  it("uses browser workspace id for local browser identity", () => {
    const ws: WorkspaceSummary = {
      id: "ws_abc123",
      slug: "browser-ws-abc123",
      workspace_name: "Phone",
      path: "indexeddb://gitim-ws-ws_abc123/repo",
      provider: "github",
      initialized: true,
      browser: true,
    };

    expect(workspaceIdentity("local", ws)).toBe("browser:ws_abc123");
    expect(cursorWorkspaceKey("local", ws)).toBe("gitim:cursor:browser:ws_abc123");
  });

  it("keeps runtime slugs isolated from browser ids", () => {
    const ws: WorkspaceSummary = {
      slug: "mobile",
      workspace_name: "Mobile",
      path: "/tmp/mobile",
      provider: "local",
      initialized: true,
    };

    expect(workspaceIdentity("remote", ws)).toBe("runtime:mobile");
    expect(activeWorkspaceStorageKey("remote")).toBe("gitim-active-workspace");
    expect(activeWorkspaceStorageKey("local")).toBe("gitim-active-browser-workspace");
  });
});
```

- [ ] **Step 2: Run the key tests and verify they fail**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/workspace-key.test.ts
```

Expected: FAIL because `workspace-key.ts` does not exist.

- [ ] **Step 3: Implement workspace key helpers**

Create `products/gitim/frontend/src/lib/workspace-key.ts`:

```ts
import type { ConnectionMode } from "@/hooks/use-connection-store";
import type { WorkspaceSummary } from "./types";

const RUNTIME_ACTIVE_KEY = "gitim-active-workspace";
const BROWSER_ACTIVE_KEY = "gitim-active-browser-workspace";

function cleanKeyPart(value: string): string {
  return value.replace(/\//g, "-");
}

export function activeWorkspaceStorageKey(mode: ConnectionMode): string {
  return mode === "local" ? BROWSER_ACTIVE_KEY : RUNTIME_ACTIVE_KEY;
}

export function workspaceIdentity(
  mode: ConnectionMode,
  workspace: Pick<WorkspaceSummary, "slug" | "id">,
): string {
  if (mode === "local") {
    return `browser:${workspace.id ?? workspace.slug}`;
  }
  return `runtime:${workspace.slug}`;
}

export function cursorWorkspaceKey(
  mode: ConnectionMode,
  workspace: Pick<WorkspaceSummary, "slug" | "id">,
): string {
  return "gitim:cursor:" + cleanKeyPart(workspaceIdentity(mode, workspace));
}
```

Modify `products/gitim/frontend/src/lib/types.ts`:

```ts
export interface WorkspaceSummary {
  id?: string;
  slug: string;
  workspace_name: string;
  path: string;
  provider: WorkspaceProvider;
  initialized: boolean;
  agents_count?: number;
  browser?: boolean;
  remote_url?: string;
  needs_token?: boolean;
}
```

Modify `products/gitim/frontend/src/lib/cursor.ts`:

```ts
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

This keeps the cursor module stable; callers pass the identity from `workspaceIdentity()`.

- [ ] **Step 4: Verify key tests pass**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/workspace-key.test.ts
```

Expected: PASS.

- [ ] **Step 5: Write failing browser registry tests**

Create `products/gitim/frontend/src/lib/browser-workspaces.test.ts`:

```ts
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearAllBrowserWorkspaces,
  createBrowserWorkspace,
  forgetBrowserWorkspace,
  getBrowserWorkspace,
  listBrowserWorkspaceSummaries,
  loadBrowserWorkspaces,
  loadSessionToken,
  migrateLegacyBrowserWorkspace,
  saveSessionToken,
} from "./browser-workspaces";

describe("browser workspaces", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-05-08T12:00:00Z"));
  });

  it("creates isolated v2 workspaces", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });

    expect(ws.id).toMatch(/^ws_/);
    expect(ws.slug).toBe(`browser-${ws.id}`);
    expect(ws.storage.fsName).toBe(`gitim-ws-${ws.id}`);
    expect(ws.storage.repoDir).toBe("/repo");
    expect(listBrowserWorkspaceSummaries()).toEqual([
      expect.objectContaining({
        id: ws.id,
        slug: ws.slug,
        workspace_name: "Phone",
        path: `indexeddb://gitim-ws-${ws.id}/repo`,
        browser: true,
        needs_token: true,
      }),
    ]);
  });

  it("migrates the legacy single browser config without moving storage", () => {
    localStorage.setItem(
      "gitim-local-config",
      JSON.stringify({
        remoteUrl: "https://github.com/acme/legacy",
        corsProxy: "https://cors.isomorphic-git.org",
      }),
    );

    const legacy = migrateLegacyBrowserWorkspace();

    expect(legacy).toEqual(expect.objectContaining({
      id: "legacy",
      slug: "browser-legacy",
      workspace_name: "Browser",
      remoteUrl: "https://github.com/acme/legacy",
      storage: {
        fsName: "gitim",
        repoDir: "/repo",
        legacy: true,
      },
    }));
    expect(loadBrowserWorkspaces()).toHaveLength(1);
  });

  it("keeps session tokens outside the registry", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });

    saveSessionToken(ws.id, "github_pat_secret");

    expect(loadSessionToken(ws.id)).toBe("github_pat_secret");
    expect(localStorage.getItem("gitim-browser-workspaces-v2")).not.toContain("github_pat_secret");
  });

  it("forgets a workspace and removes its session token", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    forgetBrowserWorkspace(ws.id);

    expect(getBrowserWorkspace(ws.id)).toBeUndefined();
    expect(loadSessionToken(ws.id)).toBeUndefined();
  });

  it("clears all registry and token state", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    clearAllBrowserWorkspaces();

    expect(loadBrowserWorkspaces()).toEqual([]);
    expect(loadSessionToken(ws.id)).toBeUndefined();
  });
});
```

- [ ] **Step 6: Run registry tests and verify they fail**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/browser-workspaces.test.ts
```

Expected: FAIL because `browser-workspaces.ts` does not exist.

- [ ] **Step 7: Implement browser registry**

Create `products/gitim/frontend/src/lib/browser-workspaces.ts`:

```ts
import type { WorkspaceSummary } from "./types";

export const BROWSER_REGISTRY_KEY = "gitim-browser-workspaces-v2";
export const LEGACY_LOCAL_CONFIG_KEY = "gitim-local-config";
export const LEGACY_FS_NAME = "gitim";
export const REPO_DIR = "/repo";

interface BrowserStorageConfig {
  fsName: string;
  repoDir: typeof REPO_DIR;
  legacy?: boolean;
}

export interface BrowserWorkspaceRecord {
  id: string;
  slug: string;
  workspace_name: string;
  remoteUrl: string;
  corsProxy: string;
  handler: string | null;
  storage: BrowserStorageConfig;
  initialized: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface CreateBrowserWorkspaceInput {
  remoteUrl: string;
  corsProxy: string;
  handler: string;
  workspaceName?: string;
}

function now(): string {
  return new Date().toISOString();
}

function randomId(): string {
  const bytes = new Uint8Array(12);
  crypto.getRandomValues(bytes);
  return "ws_" + Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function registryFromRaw(raw: string | null): BrowserWorkspaceRecord[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as { version?: number; workspaces?: BrowserWorkspaceRecord[] };
    if (parsed.version !== 2 || !Array.isArray(parsed.workspaces)) return [];
    return parsed.workspaces;
  } catch {
    return [];
  }
}

function saveRegistry(workspaces: BrowserWorkspaceRecord[]): void {
  localStorage.setItem(BROWSER_REGISTRY_KEY, JSON.stringify({
    version: 2,
    workspaces,
  }));
}

function sessionTokenKey(workspaceId: string): string {
  return `gitim-browser-token:${workspaceId}`;
}

function toSummary(record: BrowserWorkspaceRecord): WorkspaceSummary {
  return {
    id: record.id,
    slug: record.slug,
    workspace_name: record.workspace_name,
    path: `indexeddb://${record.storage.fsName}${record.storage.repoDir}`,
    provider: "github",
    initialized: record.initialized,
    agents_count: 0,
    browser: true,
    remote_url: record.remoteUrl,
    needs_token: !loadSessionToken(record.id),
  };
}

export function loadBrowserWorkspaces(): BrowserWorkspaceRecord[] {
  return registryFromRaw(localStorage.getItem(BROWSER_REGISTRY_KEY));
}

export function saveBrowserWorkspaces(workspaces: BrowserWorkspaceRecord[]): void {
  saveRegistry(workspaces);
}

export function migrateLegacyBrowserWorkspace(): BrowserWorkspaceRecord | undefined {
  const existing = loadBrowserWorkspaces();
  if (existing.length > 0) return existing[0];

  const raw = localStorage.getItem(LEGACY_LOCAL_CONFIG_KEY);
  if (!raw) return undefined;

  try {
    const legacy = JSON.parse(raw) as { remoteUrl?: string; corsProxy?: string };
    if (!legacy.remoteUrl) return undefined;

    const timestamp = now();
    const record: BrowserWorkspaceRecord = {
      id: "legacy",
      slug: "browser-legacy",
      workspace_name: "Browser",
      remoteUrl: legacy.remoteUrl,
      corsProxy: legacy.corsProxy ?? "https://cors.isomorphic-git.org",
      handler: null,
      storage: { fsName: LEGACY_FS_NAME, repoDir: REPO_DIR, legacy: true },
      initialized: true,
      createdAt: timestamp,
      updatedAt: timestamp,
    };
    saveRegistry([record]);
    return record;
  } catch {
    return undefined;
  }
}

export function listBrowserWorkspaces(): BrowserWorkspaceRecord[] {
  migrateLegacyBrowserWorkspace();
  return loadBrowserWorkspaces();
}

export function listBrowserWorkspaceSummaries(): WorkspaceSummary[] {
  return listBrowserWorkspaces().map(toSummary);
}

export function getBrowserWorkspace(idOrSlug: string): BrowserWorkspaceRecord | undefined {
  return listBrowserWorkspaces().find((w) => w.id === idOrSlug || w.slug === idOrSlug);
}

export function createBrowserWorkspace(input: CreateBrowserWorkspaceInput): BrowserWorkspaceRecord {
  const id = randomId();
  const timestamp = now();
  const record: BrowserWorkspaceRecord = {
    id,
    slug: `browser-${id}`,
    workspace_name: input.workspaceName?.trim() || input.remoteUrl.split("/").pop()?.replace(/\.git$/, "") || "Browser",
    remoteUrl: input.remoteUrl,
    corsProxy: input.corsProxy,
    handler: input.handler,
    storage: { fsName: `gitim-ws-${id}`, repoDir: REPO_DIR },
    initialized: true,
    createdAt: timestamp,
    updatedAt: timestamp,
  };
  saveRegistry([...loadBrowserWorkspaces(), record]);
  return record;
}

export function updateBrowserWorkspace(record: BrowserWorkspaceRecord): BrowserWorkspaceRecord {
  const next = { ...record, updatedAt: now() };
  saveRegistry(loadBrowserWorkspaces().map((w) => w.id === next.id ? next : w));
  return next;
}

export function saveSessionToken(workspaceId: string, token: string): void {
  sessionStorage.setItem(sessionTokenKey(workspaceId), token);
}

export function loadSessionToken(workspaceId: string): string | undefined {
  return sessionStorage.getItem(sessionTokenKey(workspaceId)) ?? undefined;
}

export function clearSessionToken(workspaceId: string): void {
  sessionStorage.removeItem(sessionTokenKey(workspaceId));
}

export function forgetBrowserWorkspace(idOrSlug: string): void {
  const record = getBrowserWorkspace(idOrSlug);
  if (record) clearSessionToken(record.id);
  saveRegistry(loadBrowserWorkspaces().filter((w) => w.id !== idOrSlug && w.slug !== idOrSlug));
}

export function clearAllBrowserWorkspaces(): void {
  for (const record of loadBrowserWorkspaces()) {
    clearSessionToken(record.id);
  }
  localStorage.removeItem(BROWSER_REGISTRY_KEY);
  localStorage.removeItem(LEGACY_LOCAL_CONFIG_KEY);
}
```

- [ ] **Step 8: Verify registry tests pass**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/workspace-key.test.ts src/lib/browser-workspaces.test.ts
```

Expected: PASS.

- [ ] **Step 9: Commit Task 1**

Run:

```bash
git add products/gitim/frontend/src/lib/workspace-key.ts \
  products/gitim/frontend/src/lib/workspace-key.test.ts \
  products/gitim/frontend/src/lib/browser-workspaces.ts \
  products/gitim/frontend/src/lib/browser-workspaces.test.ts \
  products/gitim/frontend/src/lib/types.ts \
  products/gitim/frontend/src/lib/cursor.ts
git commit -m "feat(frontend): add browser workspace registry"
```

Expected: commit succeeds on `codex/mobile-wasm-workspaces`.

---

### Task 2: Configurable Daemon-Web Storage And Offline State

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/storage.ts`
- Modify: `products/gitim/frontend/src/daemon-web/state.ts`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
- Modify: `products/gitim/frontend/src/daemon-web/sync.ts`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.test.ts`

- [ ] **Step 1: Add failing daemon-web offline tests**

Modify `products/gitim/frontend/src/daemon-web/handlers.test.ts` import block:

```ts
import {
  archiveChannel,
  archiveCard,
  channels,
  createCard,
  init,
  listArchivedChannels,
  listArchivedCards,
  listCards,
  poll,
  read,
  readCard,
  send,
  sendCardMessage,
  thread,
  unarchiveChannel,
  updateCard,
  joinChannel,
  unarchiveCard,
} from "./handlers";
```

Add tests inside `describe("daemon-web handlers", () => { ... })`:

```ts
  it("initializes an existing cached repo without a token", async () => {
    dirs.set("/repo/.git", []);

    const res = await init({
      workspaceId: "ws_cached",
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      storage: { fsName: "gitim-ws-ws_cached", repoDir: "/repo" },
    });

    expect(res.ok).toBe(true);
    expect(res.data).toEqual(expect.objectContaining({
      handler: "lewis",
      display_name: "Lewis",
      sync_enabled: false,
      needs_token: true,
    }));
  });

  it("requires reconnect token before browser send when token is missing", async () => {
    initState({
      workspaceId: "ws_cached",
      repoDir: "/repo",
      remoteUrl: "https://github.com/acme/room",
      fsName: "gitim-ws-ws_cached",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      displayName: "Lewis",
    });
    setState({ defaultBranch: "main", headCommit: "base" });

    const res = await send("general", "from offline cache");

    expect(res).toEqual({
      ok: false,
      error: "Reconnect token to send from this browser workspace.",
      error_code: "reconnect_required",
    });
    expect(commits).toHaveLength(0);
  });

  it("returns cached poll state without network when token is missing", async () => {
    initState({
      workspaceId: "ws_cached",
      repoDir: "/repo",
      remoteUrl: "https://github.com/acme/room",
      fsName: "gitim-ws-ws_cached",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      displayName: "Lewis",
    });
    setState({ defaultBranch: "main", headCommit: "cached-head" });

    const res = await poll("cached-head");

    expect(res).toEqual({
      ok: true,
      data: {
        commit_id: "cached-head",
        changes: [],
        sync_enabled: false,
        needs_token: true,
      },
    });
  });
```

- [ ] **Step 2: Run daemon-web tests and verify they fail**

Run:

```bash
cd products/gitim/frontend
npm test -- src/daemon-web/handlers.test.ts
```

Expected: FAIL because `initState` and `init` do not accept the new fields.

- [ ] **Step 3: Make LightningFS configurable**

Modify `products/gitim/frontend/src/daemon-web/storage.ts`:

```ts
import LightningFS from "@isomorphic-git/lightning-fs";

let fs: LightningFS | null = null;
let activeFsName = "gitim";

export interface StorageConfig {
  fsName: string;
  repoDir: "/repo";
}

export function configureFs(fsName: string): void {
  if (activeFsName === fsName && fs) return;
  activeFsName = fsName;
  fs = null;
}

export function getActiveFsName(): string {
  return activeFsName;
}

export function getFs(): LightningFS {
  if (!fs) {
    fs = new LightningFS(activeFsName);
  }
  return fs;
}

export async function wipeFs(fsName: string): Promise<void> {
  if (activeFsName === fsName) fs = null;
  new LightningFS(fsName, { wipe: true });
}

export async function readFile(path: string): Promise<string> {
  const f = getFs();
  const data = await f.promises.readFile(path, { encoding: "utf8" });
  return data as string;
}

export async function writeFile(path: string, content: string): Promise<void> {
  const f = getFs();
  await f.promises.writeFile(path, content, "utf8");
}

export async function removeFile(path: string): Promise<void> {
  const f = getFs();
  await f.promises.unlink(path);
}

export async function removeDir(path: string): Promise<void> {
  const f = getFs();
  await f.promises.rmdir(path);
}

export async function readdir(path: string): Promise<string[]> {
  const f = getFs();
  return (await f.promises.readdir(path)) as string[];
}

export async function exists(path: string): Promise<boolean> {
  try {
    const f = getFs();
    await f.promises.stat(path);
    return true;
  } catch {
    return false;
  }
}

export async function mkdir(path: string): Promise<void> {
  const f = getFs();
  try {
    await f.promises.mkdir(path);
  } catch {
    // existing directories are acceptable
  }
}

export async function stat(path: string) {
  const f = getFs();
  return f.promises.stat(path);
}
```

- [ ] **Step 4: Expand daemon-web state**

Modify `products/gitim/frontend/src/daemon-web/state.ts`:

```ts
export interface ChannelMeta {
  display_name: string;
  created_by: string;
  created_at: string;
  introduction: string;
  members: string[];
}

export interface UserMeta {
  display_name: string;
  role: string;
  introduction: string;
}

export interface DaemonWebState {
  workspaceId: string;
  repoDir: string;
  remoteUrl: string;
  fsName: string;
  corsProxy: string;
  token: string | null;
  me: { handler: string; display_name: string };
  channels: Map<string, ChannelMeta>;
  users: Map<string, UserMeta>;
  headCommit: string;
  syncStatus: "idle" | "syncing" | "error" | "reconnect_required";
  defaultBranch: string;
}

let state: DaemonWebState | null = null;

export function getState(): DaemonWebState {
  if (!state) throw new Error("daemon-web not initialized");
  return state;
}

export function initState(config: {
  workspaceId: string;
  repoDir: string;
  remoteUrl: string;
  fsName: string;
  corsProxy: string;
  token: string | null;
  handler: string;
  displayName: string;
}): DaemonWebState {
  state = {
    workspaceId: config.workspaceId,
    repoDir: config.repoDir,
    remoteUrl: config.remoteUrl,
    fsName: config.fsName,
    corsProxy: config.corsProxy,
    token: config.token,
    me: { handler: config.handler, display_name: config.displayName },
    channels: new Map(),
    users: new Map(),
    headCommit: "",
    syncStatus: config.token ? "idle" : "reconnect_required",
    defaultBranch: "main",
  };
  return state;
}

export function setState(partial: Partial<DaemonWebState>): void {
  if (!state) throw new Error("daemon-web not initialized");
  Object.assign(state, partial);
}
```

- [ ] **Step 5: Update handler init and reconnect-required responses**

Modify the top of `products/gitim/frontend/src/daemon-web/handlers.ts` storage import:

```ts
import {
  configureFs,
  readFile,
  writeFile,
  readdir,
  exists,
  mkdir,
  stat,
  removeFile,
  removeDir,
  type StorageConfig,
} from "./storage";
```

Add helper near `err()`:

```ts
function errCode(error: string, error_code: string): ApiResponse & { error_code: string } {
  return { ok: false, error, error_code };
}

function reconnectRequired(): ApiResponse & { error_code: string } {
  return errCode(
    "Reconnect token to send from this browser workspace.",
    "reconnect_required",
  );
}
```

Replace `init` signature and body:

```ts
export async function init(config: {
  workspaceId: string;
  remoteUrl: string;
  corsProxy: string;
  token: string | null;
  handler: string;
  storage: StorageConfig;
}): Promise<ApiResponse> {
  const { initState } = await import("./state");
  const dir = config.storage.repoDir;
  configureFs(config.storage.fsName);

  try {
    const repoExists = await exists(`${dir}/.git`);
    if (!repoExists && !config.token) {
      return errCode(
        "Reconnect token to clone this browser workspace.",
        "reconnect_required",
      );
    }

    if (!repoExists && config.token) {
      await gitOps.cloneRepo(
        config.remoteUrl,
        dir,
        config.corsProxy,
        tokenAuth(config.token),
      );
    }

    const branch = await gitOps.getCurrentBranch(dir);

    let displayName = config.handler;
    const userMetaPath = `${dir}/users/${config.handler}.meta.yaml`;
    if (await exists(userMetaPath)) {
      const content = await readFile(userMetaPath);
      const meta = parseYaml(content);
      if (meta.display_name) displayName = meta.display_name as string;
    }

    const s = initState({
      workspaceId: config.workspaceId,
      repoDir: dir,
      remoteUrl: config.remoteUrl,
      fsName: config.storage.fsName,
      corsProxy: config.corsProxy,
      token: config.token,
      handler: config.handler,
      displayName,
    });
    s.defaultBranch = branch;

    const head = await gitOps.resolveHead(dir);
    setState({ headCommit: head });
    await refreshChannelsCache();
    await refreshUsersCache();

    return ok({
      handler: config.handler,
      display_name: displayName,
      sync_enabled: !!config.token,
      needs_token: !config.token,
    });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}
```

Modify `health()` response:

```ts
export async function health(): Promise<ApiResponse> {
  try {
    const s = getState();
    return ok({
      service: "daemon-web",
      initialized: true,
      workspace: s.workspaceId,
      sync_enabled: !!s.token,
      needs_token: !s.token,
    });
  } catch {
    return ok({ service: "daemon-web", initialized: false });
  }
}
```

At the top of `poll()` after `const s = getState();` add:

```ts
  if (!s.token) {
    return ok({
      commit_id: s.headCommit,
      changes: [],
      sync_enabled: false,
      needs_token: true,
    });
  }
```

At the top of all mutating handlers add `if (!getState().token) return reconnectRequired();`:

```ts
export async function send(
  channel: string,
  body: string,
  _author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  if (!getState().token) return reconnectRequired();
  // existing body follows
}
```

Apply the same guard to `joinChannel`, `archiveChannel`, `unarchiveChannel`, `createCard`, `sendCardMessage`, `updateCard`, `archiveCard`, and `unarchiveCard`.

- [ ] **Step 6: Disable sync loop when token is missing**

Modify `products/gitim/frontend/src/daemon-web/sync.ts` at the start of `runSyncOnce()`:

```ts
  if (!s.token) {
    setState({ syncStatus: "reconnect_required" });
    return;
  }
```

Keep the existing `const onAuth = tokenAuth(s.token);` after that guard.

- [ ] **Step 7: Verify daemon-web tests pass**

Run:

```bash
cd products/gitim/frontend
npm test -- src/daemon-web/handlers.test.ts
```

Expected: PASS.

- [ ] **Step 8: Commit Task 2**

Run:

```bash
git add products/gitim/frontend/src/daemon-web/storage.ts \
  products/gitim/frontend/src/daemon-web/state.ts \
  products/gitim/frontend/src/daemon-web/handlers.ts \
  products/gitim/frontend/src/daemon-web/sync.ts \
  products/gitim/frontend/src/daemon-web/handlers.test.ts
git commit -m "feat(frontend): support cached browser workspace sessions"
```

Expected: commit succeeds.

---

### Task 3: Worker Session Identity And LocalBackend Lifecycle

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
- Modify: `products/gitim/frontend/src/lib/backend.ts`

- [ ] **Step 1: Update worker RPC types**

Modify `products/gitim/frontend/src/daemon-web/worker.ts` request/response interfaces:

```ts
export interface WorkerRequest {
  id: number;
  method: string;
  args: unknown[];
  workspaceId: string;
  generation: number;
}

export interface WorkerResponse {
  id: number;
  workspaceId: string;
  generation: number;
  result?: unknown;
  error?: string;
}

export interface WorkerEvent {
  type: "sync_reset" | "sync_error";
  workspaceId: string;
  generation: number;
}
```

Update `self.onmessage` response construction:

```ts
self.onmessage = async (event: MessageEvent<WorkerRequest>) => {
  const { id, method, args, workspaceId, generation } = event.data;

  const fn = handler[method];
  if (!fn) {
    const response: WorkerResponse = {
      id,
      workspaceId,
      generation,
      error: `unknown method: ${method}`,
    };
    self.postMessage(response);
    return;
  }

  try {
    const result = await fn(...args);
    const response: WorkerResponse = { id, workspaceId, generation, result };
    self.postMessage(response);
  } catch (e) {
    const response: WorkerResponse = {
      id,
      workspaceId,
      generation,
      error: String((e as Error).message ?? e),
    };
    self.postMessage(response);
  }
};
```

- [ ] **Step 2: Update init worker config cast**

Modify the `init` handler cast in `products/gitim/frontend/src/daemon-web/worker.ts`:

```ts
  init: (config: unknown) =>
    handlers.init(
      config as {
        workspaceId: string;
        remoteUrl: string;
        corsProxy: string;
        token: string | null;
        handler: string;
        storage: { fsName: string; repoDir: "/repo" };
      },
    ),
```

- [ ] **Step 3: Update LocalBackend constructor and stale guards**

Modify `products/gitim/frontend/src/lib/backend.ts` LocalBackend fields and constructor:

```ts
export class LocalBackend implements Backend {
  private worker: Worker;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: ApiResponse) => void; reject: (e: Error) => void }
  >();
  private onSyncReset?: () => void;
  private workspaceId: string;
  private generation: number;

  constructor(config: {
    workspaceId: string;
    generation: number;
    onSyncReset?: () => void;
  }) {
    this.workspaceId = config.workspaceId;
    this.generation = config.generation;
    this.onSyncReset = config.onSyncReset;
    this.worker = new Worker(
      new URL("../daemon-web/worker.ts", import.meta.url),
      { type: "module" },
    );
    this.worker.onmessage = (event: MessageEvent) => {
      const data = event.data as WorkerResponse | WorkerEvent;
      if (
        data.workspaceId !== this.workspaceId ||
        data.generation !== this.generation
      ) {
        return;
      }

      if ("type" in data && data.type === "sync_reset") {
        this.onSyncReset?.();
        const reset = (globalThis as unknown as Record<string, unknown>)
          .__gitimSyncReset;
        if (typeof reset === "function") reset();
        return;
      }

      const resp = data as WorkerResponse;
      const handler = this.pending.get(resp.id);
      if (handler) {
        this.pending.delete(resp.id);
        if (resp.error) {
          handler.resolve({ ok: false, error: resp.error });
        } else {
          handler.resolve(resp.result as ApiResponse);
        }
      }
    };
    this.worker.onerror = (event) => {
      this.rejectPending(event.message || "browser worker failed");
    };
    this.worker.onmessageerror = () => {
      this.rejectPending("browser worker sent an unreadable response");
    };
  }
```

Modify `call()`:

```ts
  private call(method: string, ...args: unknown[]): Promise<ApiResponse> {
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });
      const request: WorkerRequest = {
        id,
        method,
        args,
        workspaceId: this.workspaceId,
        generation: this.generation,
      };
      this.worker.postMessage(request);
    });
  }
```

Modify `init()` signature:

```ts
  async init(config: {
    workspaceId: string;
    remoteUrl: string;
    corsProxy: string;
    token: string | null;
    handler: string;
    storage: { fsName: string; repoDir: "/repo" };
  }): Promise<ApiResponse> {
    return this.call("init", config);
  }
```

Keep `terminate()` and add pending rejection:

```ts
  terminate(): void {
    this.rejectPending("browser worker session closed");
    this.worker.terminate();
  }
```

- [ ] **Step 4: Typecheck worker/backend changes**

Run:

```bash
cd products/gitim/frontend
npm run build
```

Expected: PASS through `tsc -b` and Vite build.

- [ ] **Step 5: Commit Task 3**

Run:

```bash
git add products/gitim/frontend/src/daemon-web/worker.ts \
  products/gitim/frontend/src/lib/backend.ts
git commit -m "feat(frontend): scope browser worker sessions"
```

Expected: commit succeeds.

---

### Task 4: Local Client Workspace CRUD And Activation

**Files:**
- Modify: `products/gitim/frontend/src/lib/client.ts`
- Modify: `products/gitim/frontend/src/hooks/use-workspace-store.ts`
- Modify: `products/gitim/frontend/src/app.tsx`

- [ ] **Step 1: Add browser workspace CRUD to client**

Modify imports in `products/gitim/frontend/src/lib/client.ts`:

```ts
import { HttpBackend, LocalBackend } from "./backend";
import {
  clearAllBrowserWorkspaces,
  clearSessionToken,
  createBrowserWorkspace,
  forgetBrowserWorkspace,
  getBrowserWorkspace,
  listBrowserWorkspaceSummaries,
  loadSessionToken,
  saveSessionToken,
  updateBrowserWorkspace,
  type BrowserWorkspaceRecord,
} from "./browser-workspaces";
```

Add local session globals near `activeBackend`:

```ts
let activeBackend: Backend = new HttpBackend(() => baseUrl());
let activeLocalBackend: LocalBackend | null = null;
let localGeneration = 0;
```

Add local workspace helpers:

```ts
export async function activateBrowserWorkspace(
  idOrSlug: string,
  options: {
    token?: string | null;
    onSyncReset?: () => void;
  } = {},
): Promise<ApiResponse<{ workspace: BrowserWorkspaceRecord }>> {
  const record = getBrowserWorkspace(idOrSlug);
  if (!record) return { ok: false, error: "Browser workspace not found", error_code: "not_found" };

  activeLocalBackend?.terminate();
  localGeneration += 1;
  const backend = new LocalBackend({
    workspaceId: record.id,
    generation: localGeneration,
    onSyncReset: options.onSyncReset,
  });

  const token = options.token ?? loadSessionToken(record.id) ?? null;
  const result = await backend.init({
    workspaceId: record.id,
    remoteUrl: record.remoteUrl,
    corsProxy: record.corsProxy,
    token,
    handler: record.handler ?? "",
    storage: record.storage,
  });

  if (!result.ok) {
    backend.terminate();
    return result as ApiResponse<{ workspace: BrowserWorkspaceRecord }>;
  }

  if (token) {
    saveSessionToken(record.id, token);
    await backend.startSync();
  }

  activeLocalBackend = backend;
  setBackend(backend);

  const data = result.data as Record<string, unknown> | undefined;
  const handler = data?.handler as string | undefined;
  const displayName = data?.display_name as string | undefined;
  const next = handler && handler !== record.handler
    ? updateBrowserWorkspace({
        ...record,
        handler,
        workspace_name: record.workspace_name || displayName || handler,
      })
    : record;

  return { ok: true, data: { workspace: next } };
}

export function shutdownBrowserWorkspace(): void {
  activeLocalBackend?.terminate();
  activeLocalBackend = null;
}

export function rememberBrowserToken(workspaceId: string, token: string): void {
  saveSessionToken(workspaceId, token);
}

export function clearBrowserToken(workspaceId: string): void {
  clearSessionToken(workspaceId);
}

export function resetAllBrowserWorkspaces(): void {
  shutdownBrowserWorkspace();
  clearAllBrowserWorkspaces();
}
```

- [ ] **Step 2: Replace local `listWorkspaces` stub**

Modify `listWorkspaces()` local branch in `products/gitim/frontend/src/lib/client.ts`:

```ts
  if (isLocalMode()) {
    return {
      ok: true,
      data: {
        workspaces: listBrowserWorkspaceSummaries(),
      },
    };
  }
```

- [ ] **Step 3: Implement local create/delete workspace branches**

Modify `createWorkspace()` local branch:

```ts
  if (isLocalMode()) {
    if (req.git.provider !== "github") {
      return { ok: false, error: "Browser workspaces require a GitHub remote", error_code: "unsupported_provider" };
    }
    const record = createBrowserWorkspace({
      remoteUrl: req.git.remote_url,
      corsProxy: "https://cors.isomorphic-git.org",
      handler: "",
      workspaceName: req.workspace_name,
    });
    saveSessionToken(record.id, req.git.token);
    return {
      ok: true,
      data: {
        slug: record.slug,
        workspace_name: record.workspace_name,
        path: `indexeddb://${record.storage.fsName}${record.storage.repoDir}`,
        provider: "github",
      },
    };
  }
```

Modify `deleteWorkspace()` local branch:

```ts
  if (isLocalMode()) {
    forgetBrowserWorkspace(slug);
    return { ok: true, data: {} };
  }
```

This branch unregisters the workspace. Cache wipe is handled by the UI action in Task 6.

- [ ] **Step 4: Store active workspace with mode-aware key**

Modify `products/gitim/frontend/src/hooks/use-workspace-store.ts` imports:

```ts
import { useConnectionStore } from "@/hooks/use-connection-store";
import { activeWorkspaceStorageKey } from "@/lib/workspace-key";
```

Replace storage helpers:

```ts
function currentActiveKey(): string {
  return activeWorkspaceStorageKey(useConnectionStore.getState().mode);
}

function loadStoredSlug(): string | null {
  return localStorage.getItem(currentActiveKey());
}

function persistSlug(slug: string | null) {
  const key = currentActiveKey();
  if (slug) localStorage.setItem(key, slug);
  else localStorage.removeItem(key);
}
```

Modify `fetchAll()` to refresh active slug from the current mode key:

```ts
    const workspaces = res.data.workspaces ?? [];
    const stored = loadStoredSlug();
    const current = get().activeSlug;
    let nextActive = stored ?? current;
    if (!nextActive || !workspaces.some((w) => w.slug === nextActive)) {
      nextActive = workspaces[0]?.slug ?? null;
    }
```

- [ ] **Step 5: Activate local backend on active browser workspace switch**

Modify imports in `products/gitim/frontend/src/app.tsx`:

```ts
import { workspaceIdentity } from "./lib/workspace-key";
```

In the init effect, before bootstrapping `me/channels/users`, derive active identity:

```ts
    const activeWorkspace = workspaces.find((w) => w.slug === activeSlug);
    if (!activeWorkspace) return;
```

Before `resetChatForSwitch();`, add:

```ts
    if (mode === "local") {
      client.activateBrowserWorkspace(activeSlug, {
        onSyncReset: () => useConnectionStore.getState().setCloneProgress(null),
      }).then((res) => {
        if (!res.ok) {
          useConnectionStore.getState().setError(res.error ?? "Failed to open browser workspace");
        }
      });
    }
```

Replace cursor load/save key usage:

```ts
      workspaceRef.current = workspaceIdentity(mode, activeWorkspace);
      sinceRef.current = loadCursor(workspaceRef.current);
```

Use the same `workspaceRef.current` already present in `runPoll()` for `saveCursor()` and `clearCursor()`.

- [ ] **Step 6: Run typecheck**

Run:

```bash
cd products/gitim/frontend
npm run build
```

Expected: PASS.

- [ ] **Step 7: Commit Task 4**

Run:

```bash
git add products/gitim/frontend/src/lib/client.ts \
  products/gitim/frontend/src/hooks/use-workspace-store.ts \
  products/gitim/frontend/src/app.tsx
git commit -m "feat(frontend): activate browser workspaces from registry"
```

Expected: commit succeeds.

---

### Task 5: Browser Workspace Form And Local Setup

**Files:**
- Create: `products/gitim/frontend/src/components/setup/browser-workspace-form.tsx`
- Modify: `products/gitim/frontend/src/components/setup/local-setup.tsx`

- [ ] **Step 1: Create reusable browser workspace form**

Create `products/gitim/frontend/src/components/setup/browser-workspace-form.tsx`:

```tsx
import { useState } from "react";
import { inferBrowserIdentity } from "../../lib/browser-identity";
import {
  createBrowserWorkspace,
  saveSessionToken,
  updateBrowserWorkspace,
  type BrowserWorkspaceRecord,
} from "../../lib/browser-workspaces";
import { Button } from "@/components/ui/button";

interface BrowserWorkspaceFormProps {
  initial?: BrowserWorkspaceRecord;
  submitLabel?: string;
  onConnected: (record: BrowserWorkspaceRecord, token: string) => Promise<void> | void;
  onCancel?: () => void;
}

export function BrowserWorkspaceForm({
  initial,
  submitLabel,
  onConnected,
  onCancel,
}: BrowserWorkspaceFormProps) {
  const [workspaceName, setWorkspaceName] = useState(initial?.workspace_name ?? "");
  const [remoteUrl, setRemoteUrl] = useState(initial?.remoteUrl ?? "");
  const [corsProxy, setCorsProxy] = useState(initial?.corsProxy ?? "https://cors.isomorphic-git.org");
  const [token, setToken] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [handler, setHandler] = useState<string | null>(initial?.handler ?? null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!remoteUrl.trim() || !token.trim()) return;

    setLoading(true);
    setError(null);
    try {
      const identity = await inferBrowserIdentity({
        remoteUrl: remoteUrl.trim(),
        token: token.trim(),
      });
      setHandler(identity.handler);

      const record = initial
        ? updateBrowserWorkspace({
            ...initial,
            workspace_name: workspaceName.trim() || initial.workspace_name,
            remoteUrl: remoteUrl.trim(),
            corsProxy: corsProxy.trim(),
            handler: identity.handler,
          })
        : createBrowserWorkspace({
            remoteUrl: remoteUrl.trim(),
            corsProxy: corsProxy.trim(),
            handler: identity.handler,
            workspaceName: workspaceName.trim(),
          });

      saveSessionToken(record.id, token.trim());
      await onConnected(record, token.trim());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      {error && (
        <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="space-y-2">
        <label htmlFor="browser-workspace-name" className="text-sm font-medium text-text-secondary">
          Workspace name
        </label>
        <input
          id="browser-workspace-name"
          value={workspaceName}
          onChange={(e) => setWorkspaceName(e.target.value)}
          placeholder="Phone"
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
        />
      </div>

      <div className="space-y-2">
        <label htmlFor="browser-remote-url" className="text-sm font-medium text-text-secondary">
          Git remote URL
        </label>
        <input
          id="browser-remote-url"
          type="url"
          value={remoteUrl}
          onChange={(e) => setRemoteUrl(e.target.value)}
          placeholder="https://github.com/team/im-repo"
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
          required
        />
      </div>

      <div className="space-y-2">
        <label htmlFor="browser-token" className="text-sm font-medium text-text-secondary">
          Personal access token
        </label>
        <input
          id="browser-token"
          type="password"
          value={token}
          onChange={(e) => setToken(e.target.value)}
          placeholder="github_pat_..."
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
          required
        />
        <p className="text-xs text-text-muted">
          The token is kept in this tab session so refresh can reconnect. Closing the tab clears it.
        </p>
      </div>

      <div className="space-y-2">
        <label htmlFor="browser-cors-proxy" className="text-sm font-medium text-text-secondary">
          CORS proxy
        </label>
        <input
          id="browser-cors-proxy"
          type="url"
          value={corsProxy}
          onChange={(e) => setCorsProxy(e.target.value)}
          placeholder="https://cors.isomorphic-git.org"
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
        />
        <p className="text-xs text-text-muted">
          The proxy can see git traffic and authorization headers. Use a trusted proxy for private repositories.
        </p>
      </div>

      {handler && (
        <p className="text-sm text-text-muted">Signed in as @{handler}</p>
      )}

      <div className="flex gap-2">
        {onCancel && (
          <Button type="button" variant="outline" className="flex-1" onClick={onCancel}>
            Cancel
          </Button>
        )}
        <Button type="submit" className="flex-1" disabled={loading || !remoteUrl.trim() || !token.trim()}>
          {loading ? "Connecting..." : submitLabel ?? "Connect"}
        </Button>
      </div>
    </form>
  );
}
```

- [ ] **Step 2: Refactor LocalSetup to use registry and auto restore**

Replace `products/gitim/frontend/src/components/setup/local-setup.tsx` with:

```tsx
import { useEffect, useMemo, useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import {
  listBrowserWorkspaces,
  loadSessionToken,
  type BrowserWorkspaceRecord,
} from "../../lib/browser-workspaces";
import { activateBrowserWorkspace } from "../../lib/client";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import { SetupShell } from "./setup-shell";
import { BrowserWorkspaceForm } from "./browser-workspace-form";
import { Button } from "@/components/ui/button";

export function LocalSetup() {
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setLocalReady = useConnectionStore((s) => s.setLocalReady);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);
  const cloneProgress = useConnectionStore((s) => s.cloneProgress);
  const setCloneProgress = useConnectionStore((s) => s.setCloneProgress);
  const setMode = useConnectionStore((s) => s.setMode);
  const fetchWorkspaces = useWorkspaceStore((s) => s.fetchAll);
  const setActive = useWorkspaceStore((s) => s.setActive);

  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<BrowserWorkspaceRecord | null>(null);
  const workspaces = useMemo(() => listBrowserWorkspaces(), []);

  async function openWorkspace(record: BrowserWorkspaceRecord, token?: string | null) {
    setLoading(true);
    setError(null);
    setCloneProgress(token ? "Connecting..." : "Opening cached workspace...");
    const res = await activateBrowserWorkspace(record.slug, {
      token: token ?? loadSessionToken(record.id) ?? null,
      onSyncReset: () => setCloneProgress(null),
    });
    setCloneProgress(null);
    setLoading(false);

    if (!res.ok) {
      setError(res.error ?? "Failed to open browser workspace");
      return;
    }

    setLocalReady(true);
    setStatus("ready");
    await fetchWorkspaces();
    setActive(record.slug);
  }

  useEffect(() => {
    const first = workspaces.find((ws) => loadSessionToken(ws.id));
    if (!first) return;
    void openWorkspace(first, loadSessionToken(first.id));
  }, []);

  return (
    <SetupShell
      step={2}
      title="Browser Mode"
      description="Open a GitIM repository directly in this browser"
      error={error}
      loading={loading}
      footer={
        <button
          type="button"
          onClick={() => setMode("remote")}
          className="text-text-muted hover:text-foreground transition-colors"
        >
          Use desktop runtime instead
        </button>
      }
    >
      {workspaces.length > 0 && !selected && (
        <div className="space-y-3">
          {workspaces.map((ws) => {
            const token = loadSessionToken(ws.id);
            return (
              <div key={ws.id} className="rounded-lg border border-border bg-surface/40 p-3">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-foreground">{ws.workspace_name}</p>
                    <p className="truncate text-xs font-mono text-text-muted">{ws.remoteUrl}</p>
                  </div>
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => token ? openWorkspace(ws, token) : setSelected(ws)}
                  >
                    {token ? "Open" : "Reconnect"}
                  </Button>
                </div>
              </div>
            );
          })}
          <Button type="button" variant="outline" className="w-full" onClick={() => setSelected(null)}>
            New browser workspace
          </Button>
        </div>
      )}

      {(workspaces.length === 0 || selected) && (
        <BrowserWorkspaceForm
          initial={selected ?? undefined}
          submitLabel={selected ? "Reconnect" : "Connect"}
          onCancel={workspaces.length > 0 ? () => setSelected(null) : undefined}
          onConnected={async (record, token) => {
            await openWorkspace(record, token);
          }}
        />
      )}

      {cloneProgress && (
        <p className="pt-3 text-sm text-text-muted animate-pulse">{cloneProgress}</p>
      )}
    </SetupShell>
  );
}
```

- [ ] **Step 3: Run setup-related e2e tests**

Run:

```bash
cd products/gitim/frontend
npm run test:e2e -- e2e/mobile-layout.spec.ts --grep "browser mode setup|fresh setup can switch|preflights"
```

Expected: existing setup tests pass after test selectors are updated in Task 7.

- [ ] **Step 4: Commit Task 5**

Run:

```bash
git add products/gitim/frontend/src/components/setup/browser-workspace-form.tsx \
  products/gitim/frontend/src/components/setup/local-setup.tsx
git commit -m "feat(frontend): add browser workspace setup flow"
```

Expected: commit succeeds.

---

### Task 6: Workspace Switcher Cache Actions

**Files:**
- Modify: `products/gitim/frontend/src/components/workspace/workspace-switcher.tsx`
- Modify: `products/gitim/frontend/src/lib/client.ts`

- [ ] **Step 1: Add cache wipe helpers to client**

Modify imports in `products/gitim/frontend/src/lib/client.ts`:

```ts
import { wipeAllBrowserWorkspaceCaches, wipeBrowserWorkspaceCache } from "./browser-workspaces";
```

Add exported functions:

```ts
export async function resetBrowserWorkspaceCache(slug: string): Promise<ApiResponse> {
  const record = getBrowserWorkspace(slug);
  if (!record) return { ok: false, error: "Browser workspace not found", error_code: "not_found" };
  shutdownBrowserWorkspace();
  await wipeBrowserWorkspaceCache(record.id);
  return { ok: true, data: {} };
}

export async function forgetBrowserWorkspaceAndCache(slug: string): Promise<ApiResponse> {
  const record = getBrowserWorkspace(slug);
  if (!record) return { ok: false, error: "Browser workspace not found", error_code: "not_found" };
  shutdownBrowserWorkspace();
  await wipeBrowserWorkspaceCache(record.id);
  forgetBrowserWorkspace(record.id);
  return { ok: true, data: {} };
}

export async function startOverBrowserWorkspaces(): Promise<ApiResponse> {
  shutdownBrowserWorkspace();
  await wipeAllBrowserWorkspaceCaches();
  clearAllBrowserWorkspaces();
  return { ok: true, data: {} };
}
```

Add cache wipe helpers to `products/gitim/frontend/src/lib/browser-workspaces.ts`:

```ts
export async function wipeBrowserWorkspaceCache(idOrSlug: string): Promise<void> {
  const record = getBrowserWorkspace(idOrSlug);
  if (!record) return;
  const { wipeFs } = await import("../daemon-web/storage");
  await wipeFs(record.storage.fsName);
}

export async function wipeAllBrowserWorkspaceCaches(): Promise<void> {
  const workspaces = loadBrowserWorkspaces();
  const { wipeFs } = await import("../daemon-web/storage");
  const fsNames = new Set(workspaces.map((record) => record.storage.fsName));
  fsNames.add(LEGACY_FS_NAME);
  for (const fsName of fsNames) {
    await wipeFs(fsName);
  }
}
```

- [ ] **Step 2: Add browser-mode switcher actions**

Modify imports in `products/gitim/frontend/src/components/workspace/workspace-switcher.tsx`:

```tsx
import { Check, ChevronsUpDown, Cloud, GitBranch, Plus, RefreshCcw, Trash2 } from "lucide-react";
import * as client from "@/lib/client";
import { BrowserWorkspaceForm } from "@/components/setup/browser-workspace-form";
import { getBrowserWorkspace } from "@/lib/browser-workspaces";
```

Inside `WorkspaceSwitcher`, add:

```tsx
  const mode = useConnectionStore((s) => s.mode);
  const fetchAll = useWorkspaceStore((s) => s.fetchAll);
  const [reconnectOpen, setReconnectOpen] = useState<WorkspaceSummary | null>(null);
  const [startOverOpen, setStartOverOpen] = useState(false);
```

Replace the dialog content:

```tsx
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New workspace</DialogTitle>
          </DialogHeader>
          {mode === "local" ? (
            <BrowserWorkspaceForm
              onConnected={async (record, token) => {
                await client.activateBrowserWorkspace(record.slug, { token });
                await fetchAll();
                setActive(record.slug);
                setCreateOpen(false);
                toast.success(`Created workspace ${record.workspace_name}`);
              }}
              onCancel={() => setCreateOpen(false)}
            />
          ) : (
            <CreateWorkspaceForm
              onCreated={(ws) => {
                setCreateOpen(false);
                toast.success(`Created workspace ${ws.workspace_name}`);
              }}
              onCancel={() => setCreateOpen(false)}
            />
          )}
        </DialogContent>
      </Dialog>

      <Dialog open={!!reconnectOpen} onOpenChange={(open) => !open && setReconnectOpen(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Reconnect workspace</DialogTitle>
          </DialogHeader>
          {reconnectOpen && (
            <BrowserWorkspaceForm
              initial={getBrowserWorkspace(reconnectOpen.slug)}
              submitLabel="Reconnect"
              onConnected={async (record, token) => {
                await client.activateBrowserWorkspace(record.slug, { token });
                await fetchAll();
                setActive(record.slug);
                setReconnectOpen(null);
                toast.success(`Reconnected ${record.workspace_name}`);
              }}
              onCancel={() => setReconnectOpen(null)}
            />
          )}
        </DialogContent>
      </Dialog>

      <Dialog open={startOverOpen} onOpenChange={setStartOverOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Start over in browser mode</DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <p className="text-sm text-text-muted">
              This clears all browser workspace entries, session tokens, and IndexedDB git caches for this origin.
            </p>
            <div className="flex justify-end gap-2">
              <Button type="button" variant="outline" onClick={() => setStartOverOpen(false)}>
                Cancel
              </Button>
              <Button
                type="button"
                variant="destructive"
                onClick={async () => {
                  const res = await client.startOverBrowserWorkspaces();
                  if (res.ok) {
                    await fetchAll();
                    setStartOverOpen(false);
                    toast.success("Browser workspaces cleared");
                  } else {
                    toast.error(res.error ?? "Failed to clear browser workspaces");
                  }
                }}
              >
                Start over
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
```

Add a browser-mode menu item after the new workspace item:

```tsx
          {mode === "local" && (
            <DropdownMenuItem
              data-testid="workspace-switcher-start-over"
              onSelect={(e) => {
                e.preventDefault();
                setMenuOpen(false);
                setStartOverOpen(true);
              }}
              variant="destructive"
              className="gap-2 cursor-pointer"
            >
              <Trash2 className="size-3.5" />
              <span>Start over</span>
            </DropdownMenuItem>
          )}
```

Pass mode actions into `WorkspaceRow`:

```tsx
              mode={mode}
              onReconnect={() => setReconnectOpen(ws)}
              onResetCache={async () => {
                const ok = await client.resetBrowserWorkspaceCache(ws.slug);
                if (ok.ok) toast.success(`Reset cache for ${ws.workspace_name}`);
                else toast.error(ok.error ?? "Failed to reset cache");
              }}
              onForgetAndClear={async () => {
                const ok = await client.forgetBrowserWorkspaceAndCache(ws.slug);
                if (ok.ok) {
                  toast.success(`Forgot workspace ${ws.workspace_name}`);
                  await fetchAll();
                } else {
                  toast.error(ok.error ?? "Failed to forget workspace");
                }
              }}
```

Update `WorkspaceRowProps`:

```tsx
  mode: "remote" | "local";
  onReconnect: () => void;
  onResetCache: () => void | Promise<void>;
  onForgetAndClear: () => void | Promise<void>;
```

Add browser actions under the delete button:

```tsx
      {mode === "local" && (
        <>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onReconnect();
            }}
            aria-label={`Reconnect workspace ${ws.workspace_name}`}
            className="p-1.5 rounded-sm text-text-muted hover:text-foreground opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
          >
            <RefreshCcw className="size-3.5" />
          </button>
          <button
            type="button"
            onClick={async (e) => {
              e.stopPropagation();
              await onResetCache();
            }}
            aria-label={`Reset cache for ${ws.workspace_name}`}
            className="p-1.5 rounded-sm text-text-muted hover:text-foreground opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
          >
            <Trash2 className="size-3.5" />
          </button>
        </>
      )}
```

Change delete confirmation body for local mode:

```tsx
          <p className="text-[11px] text-text-muted">
            {mode === "local"
              ? "This removes the browser workspace entry and clears its session token."
              : "This stops any running agents and unregisters the workspace from the runtime. Files on disk are not removed."}
          </p>
```

Change local delete confirmation action:

```tsx
              onClick={async () => {
                setConfirmOpen(false);
                if (mode === "local") {
                  await onForgetAndClear();
                } else {
                  await onRemove();
                }
              }}
```

- [ ] **Step 3: Typecheck UI changes**

Run:

```bash
cd products/gitim/frontend
npm run build
```

Expected: PASS.

- [ ] **Step 4: Commit Task 6**

Run:

```bash
git add products/gitim/frontend/src/components/workspace/workspace-switcher.tsx \
  products/gitim/frontend/src/lib/client.ts \
  products/gitim/frontend/src/lib/browser-workspaces.ts
git commit -m "feat(frontend): add browser workspace cache controls"
```

Expected: commit succeeds.

---

### Task 7: Mobile Browser Mode E2E Coverage

**Files:**
- Modify: `products/gitim/frontend/e2e/mobile-layout.spec.ts`

- [ ] **Step 1: Update stub worker to echo workspace session metadata**

In `stubBrowserModeWorker`, change `RpcRequest`:

```ts
    type RpcRequest = {
      id: number;
      method: string;
      args: unknown[];
      workspaceId: string;
      generation: number;
    };
```

Change the worker response:

```ts
          this.onmessage?.(
            new MessageEvent("message", {
              data: {
                id: request.id,
                workspaceId: request.workspaceId,
                generation: request.generation,
                result,
              },
            }),
          );
```

- [ ] **Step 2: Add refresh-safe sessionStorage test**

Add this test to `products/gitim/frontend/e2e/mobile-layout.spec.ts`:

```ts
test("browser mode refresh reopens the same workspace from session token", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.clear();
    sessionStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
  });
  await stubBrowserModeWorker(page);
  await page.route("https://api.github.com/user", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ login: "flame4", name: "Flame4", email: null }),
    });
  });

  await page.goto("/");
  await page.getByLabel("Workspace name").fill("Phone");
  await page.getByLabel("Git remote URL").fill("https://github.com/flame4/room");
  await page.getByLabel("Personal access token").fill("dummy-token");
  await page.getByRole("button", { name: "Connect" }).click();
  await expect(page.getByText("hello browser cards")).toBeVisible();

  await page.reload();

  await expect(page.getByText("hello browser cards")).toBeVisible();
  await expect(page.getByTestId("workspace-switcher-trigger")).toContainText("Phone");
  await expect(page.getByLabel("Personal access token")).toHaveCount(0);
});
```

- [ ] **Step 3: Add multi-workspace switch test**

Add this test:

```ts
test("browser mode can switch between registered mobile workspaces", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.clear();
    sessionStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
    localStorage.setItem("gitim-browser-workspaces-v2", JSON.stringify({
      version: 2,
      workspaces: [
        {
          id: "ws_phone",
          slug: "browser-ws_phone",
          workspace_name: "Phone",
          remoteUrl: "https://github.com/flame4/phone",
          corsProxy: "https://cors.isomorphic-git.org",
          handler: "flame4",
          storage: { fsName: "gitim-ws-ws_phone", repoDir: "/repo" },
          initialized: true,
          createdAt: "2026-05-08T12:00:00.000Z",
          updatedAt: "2026-05-08T12:00:00.000Z",
        },
        {
          id: "ws_tablet",
          slug: "browser-ws_tablet",
          workspace_name: "Tablet",
          remoteUrl: "https://github.com/flame4/tablet",
          corsProxy: "https://cors.isomorphic-git.org",
          handler: "flame4",
          storage: { fsName: "gitim-ws-ws_tablet", repoDir: "/repo" },
          initialized: true,
          createdAt: "2026-05-08T12:00:00.000Z",
          updatedAt: "2026-05-08T12:00:00.000Z",
        },
      ],
    }));
    sessionStorage.setItem("gitim-browser-token:ws_phone", "token-phone");
    sessionStorage.setItem("gitim-browser-token:ws_tablet", "token-tablet");
    localStorage.setItem("gitim-active-browser-workspace", "browser-ws_phone");
  });
  await stubBrowserModeWorker(page);

  await page.goto("/");
  await expect(page.getByText("hello browser cards")).toBeVisible();
  await expect(page.getByTestId("workspace-switcher-trigger")).toContainText("Phone");

  await page.getByTestId("workspace-switcher-trigger").click();
  await page.getByTestId("workspace-row-browser-ws_tablet").click();

  await expect(page.getByTestId("workspace-switcher-trigger")).toContainText("Tablet");
});
```

- [ ] **Step 4: Add reconnect prompt test after tab session token is gone**

Add this test:

```ts
test("browser mode asks to reconnect when a registered workspace has no session token", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.clear();
    sessionStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
    localStorage.setItem("gitim-browser-workspaces-v2", JSON.stringify({
      version: 2,
      workspaces: [
        {
          id: "ws_phone",
          slug: "browser-ws_phone",
          workspace_name: "Phone",
          remoteUrl: "https://github.com/flame4/phone",
          corsProxy: "https://cors.isomorphic-git.org",
          handler: "flame4",
          storage: { fsName: "gitim-ws-ws_phone", repoDir: "/repo" },
          initialized: true,
          createdAt: "2026-05-08T12:00:00.000Z",
          updatedAt: "2026-05-08T12:00:00.000Z",
        },
      ],
    }));
  });
  await stubBrowserModeWorker(page);
  await page.route("https://api.github.com/user", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ login: "flame4", name: "Flame4", email: null }),
    });
  });

  await page.goto("/");
  await expect(page.getByText("Phone")).toBeVisible();
  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(page.getByLabel("Personal access token")).toBeVisible();
});
```

- [ ] **Step 5: Run mobile e2e**

Run:

```bash
cd products/gitim/frontend
npm run test:e2e -- e2e/mobile-layout.spec.ts
```

Expected: PASS.

- [ ] **Step 6: Commit Task 7**

Run:

```bash
git add products/gitim/frontend/e2e/mobile-layout.spec.ts
git commit -m "test(frontend): cover mobile browser workspaces"
```

Expected: commit succeeds.

---

### Task 8: Final Verification

**Files:**
- No planned source edits.

- [ ] **Step 1: Run unit tests**

Run:

```bash
cd products/gitim/frontend
npm test -- src
```

Expected: PASS.

- [ ] **Step 2: Run frontend build**

Run:

```bash
cd products/gitim/frontend
npm run build
```

Expected: PASS.

- [ ] **Step 3: Run mobile and sidebar e2e suite**

Run:

```bash
cd products/gitim/frontend
npm run test:e2e
```

Expected: PASS.

- [ ] **Step 4: Inspect diff**

Run:

```bash
git diff --stat main...HEAD
git diff --check main...HEAD
```

Expected: `git diff --check` prints no whitespace errors.

- [ ] **Step 5: Record verification**

Create `docs/plans/2026-05-08-mobile-wasm-workspaces-verification.md`:

```md
# Mobile WASM Workspaces Verification

- `npm test -- src`
- `npm run build`
- `npm run test:e2e`
- `git diff --check main...HEAD`
```

- [ ] **Step 6: Commit verification note**

Run:

```bash
git add docs/plans/2026-05-08-mobile-wasm-workspaces-verification.md
git commit -m "docs: record mobile wasm workspace verification"
```

Expected: commit succeeds.

---

## Self-Review

- Spec coverage: registry, legacy migration, workspaceId identity, sessionStorage PAT, offline cached read, reconnect-required write guard, worker generation, UI create/reconnect, reset/forget/start-over entry points, and e2e refresh coverage are mapped to Tasks 1-7.
- Type consistency: `WorkspaceSummary.id`, `BrowserWorkspaceRecord.id`, `workspaceId`, `fsName`, `repoDir`, and `generation` use the same names across tasks.
- Verification: unit, build, e2e, and diff whitespace checks are included in Task 8.
