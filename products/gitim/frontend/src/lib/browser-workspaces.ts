import type { WorkspaceSummary } from "./types";
import { wipeFs } from "@/daemon-web/storage";

export const BROWSER_REGISTRY_KEY = "gitim-browser-workspaces-v2";
export const LEGACY_LOCAL_CONFIG_KEY = "gitim-local-config";
export const LEGACY_FS_NAME = "gitim";
export const REPO_DIR = "/repo";

const REGISTRY_VERSION = 2;
const TOKEN_KEY_PREFIX = "gitim-browser-token:";

function createMemoryStorage(): Storage {
  const values = new Map<string, string>();

  return {
    get length() {
      return values.size;
    },
    clear() {
      values.clear();
    },
    getItem(key: string) {
      return values.get(key) ?? null;
    },
    key(index: number) {
      return Array.from(values.keys())[index] ?? null;
    },
    removeItem(key: string) {
      values.delete(key);
    },
    setItem(key: string, value: string) {
      values.set(key, value);
    },
  };
}

function ensureStorage(name: "localStorage" | "sessionStorage"): void {
  if (typeof globalThis[name]?.clear === "function") {
    return;
  }

  Object.defineProperty(globalThis, name, {
    configurable: true,
    value: createMemoryStorage(),
  });
}

ensureStorage("localStorage");
ensureStorage("sessionStorage");

export interface BrowserWorkspaceRecord {
  id: string;
  slug: string;
  workspace_name: string;
  remoteUrl: string;
  corsProxy?: string;
  handler?: string;
  storage: {
    fsName: string;
    repoDir: string;
    legacy?: boolean;
  };
  createdAt: string;
  updatedAt: string;
}

export interface CreateBrowserWorkspaceInput {
  remoteUrl: string;
  corsProxy?: string;
  handler?: string;
  workspaceName?: string;
}

interface BrowserWorkspaceRegistry {
  version: 2;
  workspaces: BrowserWorkspaceRecord[];
}

interface LegacyBrowserConfig {
  remoteUrl?: string;
  corsProxy?: string;
}

function tokenKey(workspaceId: string): string {
  return `${TOKEN_KEY_PREFIX}${workspaceId}`;
}

function nowIso(): string {
  return new Date().toISOString();
}

function createWorkspaceId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `ws_${crypto.randomUUID().replace(/-/g, "").slice(0, 12)}`;
  }
  return `ws_${Math.random().toString(36).slice(2, 14)}`;
}

function readRegistry(): BrowserWorkspaceRegistry {
  const stored = localStorage.getItem(BROWSER_REGISTRY_KEY);
  if (!stored) {
    return { version: REGISTRY_VERSION, workspaces: [] };
  }

  try {
    const parsed = JSON.parse(stored) as Partial<BrowserWorkspaceRegistry>;
    if (parsed.version === REGISTRY_VERSION && Array.isArray(parsed.workspaces)) {
      return { version: REGISTRY_VERSION, workspaces: parsed.workspaces };
    }
  } catch {
    return { version: REGISTRY_VERSION, workspaces: [] };
  }

  return { version: REGISTRY_VERSION, workspaces: [] };
}

export function loadBrowserWorkspaces(): BrowserWorkspaceRecord[] {
  return readRegistry().workspaces;
}

export function saveBrowserWorkspaces(workspaces: BrowserWorkspaceRecord[]): void {
  localStorage.setItem(
    BROWSER_REGISTRY_KEY,
    JSON.stringify({ version: REGISTRY_VERSION, workspaces }),
  );
}

export function migrateLegacyBrowserWorkspace(): BrowserWorkspaceRecord | undefined {
  const existing = getBrowserWorkspace("legacy");
  if (existing) {
    return existing;
  }

  const stored = localStorage.getItem(LEGACY_LOCAL_CONFIG_KEY);
  if (!stored) {
    return undefined;
  }

  let config: LegacyBrowserConfig;
  try {
    config = JSON.parse(stored) as LegacyBrowserConfig;
  } catch {
    return undefined;
  }

  if (!config.remoteUrl) {
    return undefined;
  }

  const timestamp = nowIso();
  const workspace: BrowserWorkspaceRecord = {
    id: "legacy",
    slug: "browser-legacy",
    workspace_name: "Browser",
    remoteUrl: config.remoteUrl,
    corsProxy: config.corsProxy,
    storage: {
      fsName: LEGACY_FS_NAME,
      repoDir: REPO_DIR,
      legacy: true,
    },
    createdAt: timestamp,
    updatedAt: timestamp,
  };

  saveBrowserWorkspaces([...loadBrowserWorkspaces(), workspace]);
  return workspace;
}

export function listBrowserWorkspaces(): BrowserWorkspaceRecord[] {
  migrateLegacyBrowserWorkspace();
  return loadBrowserWorkspaces();
}

export function listBrowserWorkspaceSummaries(): WorkspaceSummary[] {
  return listBrowserWorkspaces().map((workspace) => ({
    id: workspace.id,
    slug: workspace.slug,
    workspace_name: workspace.workspace_name,
    path: `indexeddb://${workspace.storage.fsName}${workspace.storage.repoDir}`,
    provider: "github",
    initialized: true,
    browser: true,
    remote_url: workspace.remoteUrl,
    needs_token: loadSessionToken(workspace.id) === undefined,
  }));
}

export function getBrowserWorkspace(idOrSlug: string): BrowserWorkspaceRecord | undefined {
  return loadBrowserWorkspaces().find(
    (workspace) => workspace.id === idOrSlug || workspace.slug === idOrSlug,
  );
}

export function createBrowserWorkspace(
  input: CreateBrowserWorkspaceInput,
): BrowserWorkspaceRecord {
  const id = createWorkspaceId();
  const timestamp = nowIso();
  const workspace: BrowserWorkspaceRecord = {
    id,
    slug: `browser-${id}`,
    workspace_name: input.workspaceName ?? input.remoteUrl,
    remoteUrl: input.remoteUrl,
    corsProxy: input.corsProxy,
    handler: input.handler,
    storage: {
      fsName: `gitim-ws-${id}`,
      repoDir: REPO_DIR,
    },
    createdAt: timestamp,
    updatedAt: timestamp,
  };

  saveBrowserWorkspaces([...loadBrowserWorkspaces(), workspace]);
  return workspace;
}

export function updateBrowserWorkspace(
  workspaceId: string,
  updates: Partial<CreateBrowserWorkspaceInput>,
): BrowserWorkspaceRecord | undefined {
  let updatedWorkspace: BrowserWorkspaceRecord | undefined;
  const workspaces = loadBrowserWorkspaces().map((workspace) => {
    if (workspace.id !== workspaceId) {
      return workspace;
    }

    updatedWorkspace = {
      ...workspace,
      remoteUrl: updates.remoteUrl ?? workspace.remoteUrl,
      corsProxy: updates.corsProxy ?? workspace.corsProxy,
      handler: updates.handler ?? workspace.handler,
      workspace_name: updates.workspaceName ?? workspace.workspace_name,
      updatedAt: nowIso(),
    };
    return updatedWorkspace;
  });

  saveBrowserWorkspaces(workspaces);
  return updatedWorkspace;
}

export function saveSessionToken(workspaceId: string, token: string): void {
  sessionStorage.setItem(tokenKey(workspaceId), token);
}

export function loadSessionToken(workspaceId: string): string | undefined {
  return sessionStorage.getItem(tokenKey(workspaceId)) ?? undefined;
}

export function clearSessionToken(workspaceId: string): void {
  sessionStorage.removeItem(tokenKey(workspaceId));
}

export function forgetBrowserWorkspace(workspaceId: string): void {
  saveBrowserWorkspaces(
    loadBrowserWorkspaces().filter((workspace) => workspace.id !== workspaceId),
  );
  if (workspaceId === "legacy") {
    localStorage.removeItem(LEGACY_LOCAL_CONFIG_KEY);
  }
  clearSessionToken(workspaceId);
}

export function clearAllBrowserWorkspaces(): void {
  for (let index = sessionStorage.length - 1; index >= 0; index -= 1) {
    const key = sessionStorage.key(index);
    if (key?.startsWith(TOKEN_KEY_PREFIX)) {
      sessionStorage.removeItem(key);
    }
  }
  localStorage.removeItem(BROWSER_REGISTRY_KEY);
  localStorage.removeItem(LEGACY_LOCAL_CONFIG_KEY);
}

export async function wipeBrowserWorkspaceCache(idOrSlug: string): Promise<void> {
  const workspace = getBrowserWorkspace(idOrSlug);
  if (!workspace) {
    return;
  }

  wipeFs(workspace.storage.fsName);
}

export async function wipeAllBrowserWorkspaceCaches(): Promise<void> {
  const fsNames = new Set<string>([
    ...listBrowserWorkspaces().map((workspace) => workspace.storage.fsName),
    LEGACY_FS_NAME,
  ]);

  for (const fsName of fsNames) {
    wipeFs(fsName);
  }
}
