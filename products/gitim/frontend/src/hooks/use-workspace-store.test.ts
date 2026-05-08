import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiResponse, WorkspaceSummary } from "@/lib/types";

let workspacesResponse: ApiResponse<{ workspaces: WorkspaceSummary[] }>;

vi.mock("@/lib/client", () => ({
  listWorkspaces: vi.fn(() => Promise.resolve(workspacesResponse)),
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

describe("useWorkspaceStore", () => {
  beforeEach(() => {
    resetStorage();
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
});
