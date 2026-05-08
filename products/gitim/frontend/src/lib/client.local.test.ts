import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiResponse } from "./types";

const localBackends: MockLocalBackend[] = [];
const httpBackends: MockHttpBackend[] = [];
let nextLocalInitResult: ApiResponse | null = null;

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
  initResult: ApiResponse = {
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
    if (nextLocalInitResult) {
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

describe("client local browser workspaces", () => {
  beforeEach(async () => {
    resetStorage();
    localBackends.length = 0;
    httpBackends.length = 0;
    nextLocalInitResult = null;
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
});
