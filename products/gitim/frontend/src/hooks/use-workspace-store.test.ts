import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiResponse, WorkspaceSummary } from "@/lib/types";

let workspacesResponse: ApiResponse<{ workspaces: WorkspaceSummary[] }>;
const workspacesResponses: Array<
  ApiResponse<{ workspaces: WorkspaceSummary[] }> |
  Promise<ApiResponse<{ workspaces: WorkspaceSummary[] }>>
> = [];

vi.mock("@/lib/client", () => ({
  listWorkspaces: vi.fn(() => Promise.resolve(workspacesResponses.shift() ?? workspacesResponse)),
  createWorkspace: vi.fn(),
  deleteWorkspace: vi.fn(),
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
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("useWorkspaceStore", () => {
  beforeEach(() => {
    resetStorage();
    workspacesResponses.length = 0;
    vi.clearAllMocks();
    vi.resetModules();
    workspacesResponse = {
      ok: true,
      data: {
        workspaces: [
          {
            id: "ws_2",
            slug: "browser-2",
            workspace_name: "Phone 2",
            path: "indexeddb://gitim-ws-ws_2/repo",
            provider: "github",
            initialized: true,
            browser: true,
          },
          {
            id: "ws_3",
            slug: "browser-3",
            workspace_name: "Phone 3",
            path: "indexeddb://gitim-ws-ws_3/repo",
            provider: "github",
            initialized: true,
            browser: true,
          },
        ],
      },
    };
  });

  it("rereads the active workspace from the current connection mode key", async () => {
    localStorage.setItem("gitim-active-workspace", "runtime-main");
    localStorage.setItem("gitim-active-browser-workspace", "browser-3");
    const { useConnectionStore } = await import("./use-connection-store");
    useConnectionStore.setState({ mode: "remote" });
    const { useWorkspaceStore } = await import("./use-workspace-store");

    useConnectionStore.setState({ mode: "local" });
    await useWorkspaceStore.getState().fetchAll();

    expect(useWorkspaceStore.getState().activeSlug).toBe("browser-3");
    expect(localStorage.getItem("gitim-active-workspace")).toBe("runtime-main");
    expect(localStorage.getItem("gitim-active-browser-workspace")).toBe("browser-3");
  });

  it("ignores a stale remote fetch after switching to local mode", async () => {
    const remoteFetch = deferred<ApiResponse<{ workspaces: WorkspaceSummary[] }>>();
    const localFetch = deferred<ApiResponse<{ workspaces: WorkspaceSummary[] }>>();
    workspacesResponses.push(remoteFetch.promise, localFetch.promise);
    const { useConnectionStore } = await import("./use-connection-store");
    useConnectionStore.setState({ mode: "remote" });
    const { useWorkspaceStore } = await import("./use-workspace-store");

    const remoteRequest = useWorkspaceStore.getState().fetchAll();
    useConnectionStore.setState({ mode: "local" });
    const localRequest = useWorkspaceStore.getState().fetchAll();
    localFetch.resolve({
      ok: true,
      data: {
        workspaces: [
          {
            id: "ws_local",
            slug: "browser-local",
            workspace_name: "Local",
            path: "indexeddb://gitim-ws-ws_local/repo",
            provider: "github",
            initialized: true,
            browser: true,
          },
        ],
      },
    });
    await localRequest;
    localStorage.removeItem("gitim-active-browser-workspace");

    remoteFetch.resolve({
      ok: true,
      data: {
        workspaces: [
          {
            slug: "runtime-main",
            workspace_name: "Runtime",
            path: "/tmp/runtime",
            provider: "local",
            initialized: true,
          },
        ],
      },
    });
    await remoteRequest;

    expect(useWorkspaceStore.getState().workspaces).toEqual([
      expect.objectContaining({ slug: "browser-local" }),
    ]);
    expect(localStorage.getItem("gitim-active-browser-workspace")).toBeNull();
  });

  it("refreshes and falls back when the active workspace disappears", async () => {
    localStorage.setItem("gitim-active-workspace", "deleted-ws");
    const { useWorkspaceStore } = await import("./use-workspace-store");

    useWorkspaceStore.setState({
      workspaces: [
        {
          slug: "deleted-ws",
          workspace_name: "Deleted",
          path: "/tmp/deleted",
          provider: "local",
          initialized: true,
        },
      ],
      activeSlug: "deleted-ws",
      loading: false,
      error: null,
      errorCode: null,
    });

    await useWorkspaceStore
      .getState()
      .refreshAfterActiveUnavailable("deleted-ws");

    expect(useWorkspaceStore.getState().workspaces).toEqual([
      expect.objectContaining({ slug: "browser-2" }),
      expect.objectContaining({ slug: "browser-3" }),
    ]);
    expect(useWorkspaceStore.getState().activeSlug).toBe("browser-2");
    expect(localStorage.getItem("gitim-active-workspace")).toBe("browser-2");
  });

  it("does not refresh when an old workspace reports unavailable after switching", async () => {
    const client = await import("@/lib/client");
    const { useWorkspaceStore } = await import("./use-workspace-store");

    useWorkspaceStore.setState({
      workspaces: [
        {
          slug: "current-ws",
          workspace_name: "Current",
          path: "/tmp/current",
          provider: "local",
          initialized: true,
        },
      ],
      activeSlug: "current-ws",
      loading: false,
      error: null,
      errorCode: null,
    });

    await useWorkspaceStore
      .getState()
      .refreshAfterActiveUnavailable("old-ws");

    expect(client.listWorkspaces).not.toHaveBeenCalled();
    expect(useWorkspaceStore.getState().activeSlug).toBe("current-ws");
  });
});
