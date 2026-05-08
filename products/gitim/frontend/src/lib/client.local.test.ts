import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiResponse } from "./types";

const localBackends: MockLocalBackend[] = [];
const httpBackends: MockHttpBackend[] = [];
let nextLocalInitResult: ApiResponse | null = null;
const nextLocalInitResponses: Array<ApiResponse | Promise<ApiResponse>> = [];
const wipedFsNames = vi.hoisted((): string[] => []);

class MockHttpBackend {
  baseUrl: () => string;

  constructor(baseUrl: () => string) {
    this.baseUrl = baseUrl;
    httpBackends.push(this);
  }

  health = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  me = vi.fn<() => Promise<ApiResponse>>(() =>
    Promise.resolve({ ok: true, data: { handler: "http" } }),
  );
  poll = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  channels = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  read = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  send = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  thread = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  users = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  joinChannel = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
}

class MockLocalBackend {
  config: Record<string, unknown>;
  initResult: ApiResponse | Promise<ApiResponse> = {
    ok: true,
    data: { handler: "flame4", display_name: "Flame4" },
  };
  init = vi.fn<(config: Record<string, unknown>) => Promise<ApiResponse>>(() =>
    Promise.resolve(this.initResult),
  );
  startSync = vi.fn<() => Promise<void>>(() => Promise.resolve());
  terminate = vi.fn();
  me = vi.fn<() => Promise<ApiResponse>>(() =>
    Promise.resolve({ ok: true, data: { handler: "local" } }),
  );
  health = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  poll = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  channels = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  read = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  send = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  thread = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  users = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  joinChannel = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  createCard = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  listCards = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  readCard = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  sendCardMessage = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  updateCard = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  archiveCard = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  unarchiveCard = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  listArchivedCards = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  archiveChannel = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  unarchiveChannel = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));
  listArchivedChannels = vi.fn<() => Promise<ApiResponse>>(() => Promise.resolve({ ok: true }));

  constructor(config: Record<string, unknown>) {
    this.config = config;
    const nextResponse = nextLocalInitResponses.shift();
    if (nextResponse) {
      this.initResult = nextResponse;
    } else if (nextLocalInitResult) {
      this.initResult = nextLocalInitResult;
      nextLocalInitResult = null;
    }
    localBackends.push(this);
  }
}

vi.mock("./backend", () => ({
  HttpBackend: MockHttpBackend,
  LocalBackend: MockLocalBackend,
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({
  default: class MockLightningFS {
    constructor(name: string, options?: { wipe?: boolean }) {
      if (options?.wipe) {
        wipedFsNames.push(name);
      }
    }
  },
}));

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

function resetStorage(): void {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: createMemoryStorage(),
  });
  Object.defineProperty(globalThis, "sessionStorage", {
    configurable: true,
    value: createMemoryStorage(),
  });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("client local browser workspaces", () => {
  beforeEach(async () => {
    resetStorage();
    localBackends.length = 0;
    httpBackends.length = 0;
    wipedFsNames.length = 0;
    nextLocalInitResult = null;
    nextLocalInitResponses.length = 0;
    vi.resetModules();
    const { useConnectionStore } = await import("@/hooks/use-connection-store");
    useConnectionStore.setState({ mode: "local", port: null });
  });

  it("lists registry-backed browser workspace summaries", async () => {
    const { createBrowserWorkspace } = await import("./browser-workspaces");
    const client = await import("./client");
    const first = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/first",
      workspaceName: "First",
    });
    const second = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/second",
      workspaceName: "Second",
    });

    const res = await client.listWorkspaces();

    expect(res).toEqual({
      ok: true,
      data: {
        workspaces: [
          expect.objectContaining({ id: first.id, slug: first.slug, workspace_name: "First" }),
          expect.objectContaining({ id: second.id, slug: second.slug, workspace_name: "Second" }),
        ],
      },
    });
  });

  it("creates GitHub browser workspaces and keeps tokens in session storage", async () => {
    const client = await import("./client");
    const { loadBrowserWorkspaces, loadSessionToken } = await import("./browser-workspaces");

    const res = await client.createWorkspace({
      path: "",
      workspace_name: "Phone",
      git: {
        provider: "github",
        remote_url: "https://github.com/acme/phone",
        token: "github_pat_secret",
      },
    });

    const [record] = loadBrowserWorkspaces();
    expect(res).toEqual({
      ok: true,
      data: expect.objectContaining({
        slug: record.slug,
        workspace_name: "Phone",
        path: `indexeddb://${record.storage.fsName}${record.storage.repoDir}`,
        provider: "github",
      }),
    });
    expect(loadSessionToken(record.id)).toBe("github_pat_secret");
    expect(localStorage.getItem("gitim-browser-workspaces-v2")).not.toContain("github_pat_secret");
  });

  it("rejects non-GitHub browser workspace creation", async () => {
    const client = await import("./client");

    await expect(client.createWorkspace({
      path: "/tmp/repo",
      workspace_name: "Local",
      git: { provider: "local" },
    })).resolves.toEqual({
      ok: false,
      error: "Browser workspaces require a GitHub remote",
      error_code: "unsupported_provider",
    });
  });

  it("deletes browser workspaces by slug and clears their session token", async () => {
    const { createBrowserWorkspace, loadSessionToken, saveSessionToken } =
      await import("./browser-workspaces");
    const client = await import("./client");
    const record = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/phone",
      workspaceName: "Phone",
    });
    saveSessionToken(record.id, "github_pat_secret");

    const res = await client.deleteWorkspace(record.slug);

    expect(res.ok).toBe(true);
    expect((await client.listWorkspaces()).data?.workspaces).toEqual([]);
    expect(loadSessionToken(record.id)).toBeUndefined();
  });

  it("activates a browser workspace by slug after init succeeds", async () => {
    const { createBrowserWorkspace } = await import("./browser-workspaces");
    const client = await import("./client");
    const record = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/phone",
      workspaceName: "Phone",
    });
    client.rememberBrowserToken(record.id, "github_pat_secret");

    const res = await client.activateBrowserWorkspace(record.slug);

    expect(res).toEqual({
      ok: true,
      data: {
        workspace: expect.objectContaining({
          id: record.id,
          handler: "flame4",
          workspace_name: "Flame4",
        }),
      },
    });
    expect(localBackends[0].config).toEqual({ workspaceId: record.id, generation: 1 });
    expect(localBackends[0].init).toHaveBeenCalledWith({
      workspaceId: record.id,
      remoteUrl: record.remoteUrl,
      corsProxy: "",
      token: "github_pat_secret",
      handler: "",
      storage: record.storage,
    });
    expect(localBackends[0].startSync).toHaveBeenCalledTimes(1);
    await expect(client.me(record.slug)).resolves.toEqual({
      ok: true,
      data: { handler: "local" },
    });
  });

  it("keeps the previous backend active when activation fails", async () => {
    const { createBrowserWorkspace } = await import("./browser-workspaces");
    const client = await import("./client");
    const current = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/current",
      workspaceName: "Current",
    });
    const next = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/next",
      workspaceName: "Next",
    });
    await client.activateBrowserWorkspace(current.slug);
    nextLocalInitResult = {
      ok: false,
      error: "clone failed",
      error_code: "clone_failed",
    };

    const res = await client.activateBrowserWorkspace(next.slug);

    expect(res).toEqual({
      ok: false,
      error: "clone failed",
      error_code: "clone_failed",
    });
    expect(localBackends[1].terminate).toHaveBeenCalledTimes(1);
    expect(localBackends[0].terminate).not.toHaveBeenCalled();
    await expect(client.me(current.slug)).resolves.toEqual({
      ok: true,
      data: { handler: "local" },
    });
  });

  it("keeps the newer backend active when an earlier activation resolves late", async () => {
    const { createBrowserWorkspace } = await import("./browser-workspaces");
    const client = await import("./client");
    const first = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/first",
      workspaceName: "First",
    });
    const second = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/second",
      workspaceName: "Second",
    });
    const firstInit = deferred<ApiResponse>();
    const secondInit = deferred<ApiResponse>();
    nextLocalInitResponses.push(firstInit.promise, secondInit.promise);

    const firstActivation = client.activateBrowserWorkspace(first.slug);
    const secondActivation = client.activateBrowserWorkspace(second.slug);
    expect(localBackends).toHaveLength(2);
    localBackends[0].me.mockResolvedValue({ ok: true, data: { handler: "first" } });
    localBackends[1].me.mockResolvedValue({ ok: true, data: { handler: "second" } });

    secondInit.resolve({ ok: true, data: { handler: "second", display_name: "Second" } });
    await expect(secondActivation).resolves.toEqual({
      ok: true,
      data: { workspace: expect.objectContaining({ id: second.id }) },
    });

    firstInit.resolve({ ok: true, data: { handler: "first", display_name: "First" } });
    await expect(firstActivation).resolves.toEqual({
      ok: false,
      error: "Browser workspace activation was superseded.",
      error_code: "activation_superseded",
    });

    expect(localBackends[0].terminate).toHaveBeenCalledTimes(1);
    expect(localBackends[1].terminate).not.toHaveBeenCalled();
    await expect(client.me(second.slug)).resolves.toEqual({
      ok: true,
      data: { handler: "second" },
    });
  });

  it("shuts down the active local backend and restores an HTTP backend", async () => {
    const { createBrowserWorkspace } = await import("./browser-workspaces");
    const client = await import("./client");
    const record = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/phone",
      workspaceName: "Phone",
    });
    await client.activateBrowserWorkspace(record.id);

    client.shutdownBrowserWorkspace();

    expect(localBackends[0].terminate).toHaveBeenCalledTimes(1);
    expect(httpBackends).toHaveLength(2);
  });

  it("resets one browser workspace cache after shutting down the active backend", async () => {
    const { createBrowserWorkspace, loadBrowserWorkspaces, loadSessionToken, saveSessionToken } =
      await import("./browser-workspaces");
    const client = await import("./client");
    const record = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/phone",
      workspaceName: "Phone",
    });
    saveSessionToken(record.id, "github_pat_secret");
    await client.activateBrowserWorkspace(record.slug);

    const res = await client.resetBrowserWorkspaceCache(record.slug);

    expect(res).toEqual({ ok: true, data: { activeAffected: true } });
    expect(localBackends[0].terminate).toHaveBeenCalledTimes(1);
    expect(wipedFsNames).toEqual([record.storage.fsName]);
    expect(loadBrowserWorkspaces()).toEqual([expect.objectContaining({ id: record.id })]);
    expect(loadSessionToken(record.id)).toBe("github_pat_secret");
  });

  it("resets an inactive browser workspace cache without shutting down the active backend", async () => {
    const { createBrowserWorkspace, loadBrowserWorkspaces } =
      await import("./browser-workspaces");
    const client = await import("./client");
    const active = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/active",
      workspaceName: "Active",
    });
    const inactive = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/inactive",
      workspaceName: "Inactive",
    });
    await client.activateBrowserWorkspace(active.slug);

    const res = await client.resetBrowserWorkspaceCache(inactive.slug);

    expect(res).toEqual({ ok: true, data: { activeAffected: false } });
    expect(localBackends[0].terminate).not.toHaveBeenCalled();
    expect(wipedFsNames).toEqual([inactive.storage.fsName]);
    expect(loadBrowserWorkspaces()).toEqual([
      expect.objectContaining({ id: active.id }),
      expect.objectContaining({ id: inactive.id }),
    ]);
    await expect(client.me(active.slug)).resolves.toEqual({
      ok: true,
      data: { handler: "local" },
    });
  });

  it("forgets a browser workspace and cache after shutting down the active backend", async () => {
    const { createBrowserWorkspace, loadBrowserWorkspaces, loadSessionToken, saveSessionToken } =
      await import("./browser-workspaces");
    const client = await import("./client");
    const record = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/phone",
      workspaceName: "Phone",
    });
    saveSessionToken(record.id, "github_pat_secret");
    await client.activateBrowserWorkspace(record.slug);

    const res = await client.forgetBrowserWorkspaceAndCache(record.slug);

    expect(res).toEqual({ ok: true, data: { activeAffected: true } });
    expect(localBackends[0].terminate).toHaveBeenCalledTimes(1);
    expect(wipedFsNames).toEqual([record.storage.fsName]);
    expect(loadBrowserWorkspaces()).toEqual([]);
    expect(loadSessionToken(record.id)).toBeUndefined();
  });

  it("forgets an inactive browser workspace and cache without shutting down the active backend", async () => {
    const { createBrowserWorkspace, loadBrowserWorkspaces, loadSessionToken, saveSessionToken } =
      await import("./browser-workspaces");
    const client = await import("./client");
    const active = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/active",
      workspaceName: "Active",
    });
    const inactive = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/inactive",
      workspaceName: "Inactive",
    });
    saveSessionToken(inactive.id, "github_pat_inactive");
    await client.activateBrowserWorkspace(active.slug);

    const res = await client.forgetBrowserWorkspaceAndCache(inactive.slug);

    expect(res).toEqual({ ok: true, data: { activeAffected: false } });
    expect(localBackends[0].terminate).not.toHaveBeenCalled();
    expect(wipedFsNames).toEqual([inactive.storage.fsName]);
    expect(loadBrowserWorkspaces()).toEqual([expect.objectContaining({ id: active.id })]);
    expect(loadSessionToken(inactive.id)).toBeUndefined();
    await expect(client.me(active.slug)).resolves.toEqual({
      ok: true,
      data: { handler: "local" },
    });
  });

  it("starts over by wiping all browser caches before clearing registry and tokens", async () => {
    const {
      createBrowserWorkspace,
      loadBrowserWorkspaces,
      loadSessionToken,
      migrateLegacyBrowserWorkspace,
      saveSessionToken,
    } = await import("./browser-workspaces");
    const client = await import("./client");
    localStorage.setItem(
      "gitim-local-config",
      JSON.stringify({
        remoteUrl: "https://github.com/acme/legacy",
      }),
    );
    const legacy = migrateLegacyBrowserWorkspace();
    if (!legacy) throw new Error("expected legacy workspace");
    const record = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/phone",
      workspaceName: "Phone",
    });
    saveSessionToken(record.id, "github_pat_secret");
    await client.activateBrowserWorkspace(record.slug);

    const res = await client.startOverBrowserWorkspaces();

    expect(res).toEqual({ ok: true, data: { activeAffected: true } });
    expect(localBackends[0].terminate).toHaveBeenCalledTimes(1);
    expect(wipedFsNames).toEqual([legacy.storage.fsName, record.storage.fsName]);
    expect(loadBrowserWorkspaces()).toEqual([]);
    expect(loadSessionToken(record.id)).toBeUndefined();
  });
});
