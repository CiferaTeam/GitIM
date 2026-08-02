// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

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

Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: createMemoryStorage(),
});

vi.mock("./backend", () => ({
  HttpBackend: class {
    constructor(baseUrl: () => string) {
      void baseUrl;
    }
  },
  LocalBackend: class {
    constructor(config: unknown) {
      void config;
    }
  },
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({ default: class {} }));

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

async function setup(mode: "remote" | "local" = "remote") {
  vi.resetModules();
  const { useConnectionStore } = await import("@/hooks/use-connection-store");
  useConnectionStore.setState({
    mode,
    port: mode === "remote" ? 9999 : null,
    status: "ready",
  });
  return import("./client");
}

describe("pickWorkspaceDirectory", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("posts to the runtime picker endpoint and returns the selected path", async () => {
    const client = await setup();
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockResolvedValue(
        jsonResponse(200, { path: "/Users/dev/Workspaces/team-alpha" }),
      );

    await expect(client.pickWorkspaceDirectory()).resolves.toEqual({
      ok: true,
      data: { path: "/Users/dev/Workspaces/team-alpha" },
    });
    expect(fetchSpy).toHaveBeenCalledWith(
      "http://127.0.0.1:9999/runtime/workspace-directory",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("preserves a cancelled selection as a successful null path", async () => {
    const client = await setup();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse(200, { path: null }),
    );

    await expect(client.pickWorkspaceDirectory()).resolves.toEqual({
      ok: true,
      data: { path: null },
    });
  });

  it("does not call the runtime picker in Browser/WASM mode", async () => {
    const client = await setup("local");
    const fetchSpy = vi.spyOn(globalThis, "fetch");

    await expect(client.pickWorkspaceDirectory()).resolves.toEqual({
      ok: false,
      error: "Native folder selection requires the local GitIM runtime.",
      error_code: "runtime_required",
    });
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
