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

vi.mock("@isomorphic-git/lightning-fs", () => ({
  default: class {},
}));

const RUNTIME_BASE = "http://127.0.0.1:9999";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

async function setupRemote(): Promise<typeof import("./client")> {
  vi.resetModules();
  const { useConnectionStore } = await import("@/hooks/use-connection-store");
  useConnectionStore.setState({
    mode: "remote",
    port: 9999,
    status: "ready",
  });
  return await import("./client");
}

describe("fleet client compatibility", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("falls back to direct node agent fetches when /fleet/agents is absent", async () => {
    const client = await setupRemote();
    const calls: string[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation((async (input) => {
      const url = typeof input === "string" ? input : String(input);
      calls.push(url);
      if (url === `${RUNTIME_BASE}/fleet/agents`) {
        return new Response("", { status: 404 });
      }
      if (url === `${RUNTIME_BASE}/fleet/nodes`) {
        return jsonResponse(200, {
          ok: true,
          nodes: [
            {
              node_id: "mac-mini",
              node_name: "lewismac-mini",
              base_url: "http://127.0.0.1:18068",
              workspace_mappings: [
                {
                  remote_workspace_id: "room",
                  local_workspace_id: "room",
                  workspace_identity: "github.com/flame4/room",
                },
              ],
            },
          ],
        });
      }
      if (url === "http://127.0.0.1:18068/workspaces/room/agents") {
        return jsonResponse(200, {
          ok: true,
          agents: [
            {
              id: "glm51op",
              display_name: "glm51op",
              status: "running",
              repo_path: "/Users/lewis/ateam/room/glm51op",
              provider: "pi",
              messages_processed: 24,
            },
          ],
        });
      }
      return new Response("", { status: 500 });
    }) as typeof fetch);

    const res = await client.listFleetAgents();

    expect(res.ok).toBe(true);
    expect(res.data?.agents).toHaveLength(1);
    expect(res.data?.agents[0]).toMatchObject({
      nodeId: "mac-mini",
      nodeName: "lewismac-mini",
      remoteWorkspaceId: "room",
      workspaceIdentity: "github.com/flame4/room",
      workspaceId: "room",
      agent: {
        id: "glm51op",
        name: "glm51op",
        provider: "pi",
      },
    });
    expect(calls).toEqual([
      `${RUNTIME_BASE}/fleet/agents`,
      `${RUNTIME_BASE}/fleet/nodes`,
      "http://127.0.0.1:18068/workspaces/room/agents",
    ]);
  });
});
