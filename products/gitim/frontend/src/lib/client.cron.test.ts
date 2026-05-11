// @vitest-environment jsdom
//
// Fetch-level integration tests for the cron client functions in lib/client.ts.
//
// Why fetch-level (not function-level mocks): the previous round of cron unit
// tests stubbed the client functions themselves and so couldn't catch the
// real bug — `client.ts` was wrapping its callers in `ApiResponse` semantics
// while the runtime cron endpoints actually return raw typed bodies like
// `{entries: [...]}` (timeline) / `{body: "..."}` (single run) / the
// `CronDetail` directly. By driving the actual fetch surface here, we lock
// in the contract that whatever shape the runtime emits, the consumer sees
// `{ok: true, data: <typed>}` on success and `{ok: false, error}` on
// failure with the runtime's `error_code` preserved.

import { beforeEach, describe, expect, it, vi } from "vitest";

// Set up an in-memory localStorage before any module touches it. Several
// modules dereference `window.localStorage` at import (connection store,
// browser workspaces registry, etc.), so we have to win the race.
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

// Stub the backend module so importing `client.ts` doesn't pull in the real
// isomorphic-git backend (whose internal init runs jsdom-hostile code).
vi.mock("./backend", () => ({
  HttpBackend: class {
    constructor(_baseUrl: () => string) {
      // no-op
    }
  },
  LocalBackend: class {
    constructor(_config: unknown) {
      // no-op
    }
  },
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({
  default: class {},
}));

// Matches the connection store's `baseUrl()` derivation for remote mode
// with port 9999 — hard-coded here so a contract change to baseUrl() will
// fail this constant, not the per-test assertions.
const RUNTIME_BASE = "http://127.0.0.1:9999";

async function mockFetch(
  fn: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>,
): Promise<{ restore: () => void; calls: Array<{ url: string; init?: RequestInit }> }> {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const spy = vi.spyOn(globalThis, "fetch").mockImplementation(((
    input: RequestInfo | URL,
    init?: RequestInit,
  ) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : String(input);
    calls.push({ url, init });
    return fn(input, init);
  }) as typeof fetch);
  return { restore: () => spy.mockRestore(), calls };
}

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

describe("cron client wire-shape contract (raw runtime typed bodies)", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  describe("getCronTimeline", () => {
    it("wraps runtime's raw {entries, truncated} body in {ok: true, data}", async () => {
      const client = await setupRemote();
      const runtimeBody = {
        entries: [
          { ts: "2026-05-11T09:00:00Z", kind: "past", cron_name: "alpha" },
          { ts: "2026-05-18T09:00:00Z", kind: "future", cron_name: "alpha" },
        ],
        truncated: false,
      };
      const { restore, calls } = await mockFetch(async () =>
        jsonResponse(200, runtimeBody),
      );

      const res = await client.getCronTimeline(
        "phone",
        "2026-05-01T00:00:00Z",
        "2026-05-31T23:59:59Z",
      );

      expect(res.ok).toBe(true);
      expect(res.data).toEqual(runtimeBody);
      expect(res.data?.entries).toHaveLength(2);
      expect(res.data?.truncated).toBe(false);
      expect(calls[0].url).toBe(
        `${RUNTIME_BASE}/workspaces/phone/crons/timeline?from=2026-05-01T00%3A00%3A00Z&to=2026-05-31T23%3A59%3A59Z`,
      );
      restore();
    });

    it("surfaces error_code from 4xx ErrorBody response", async () => {
      const client = await setupRemote();
      const { restore } = await mockFetch(async () =>
        jsonResponse(404, {
          ok: false,
          error: "unknown workspace",
          error_code: "not_found",
        }),
      );

      const res = await client.getCronTimeline("phone");

      expect(res.ok).toBe(false);
      expect(res.error).toBe("unknown workspace");
      expect(res.error_code).toBe("not_found");
      restore();
    });

    it("propagates AbortSignal to fetch so in-flight requests can be cancelled", async () => {
      const client = await setupRemote();
      const { restore, calls } = await mockFetch(async () => jsonResponse(200, { entries: [] }));
      const ac = new AbortController();
      await client.getCronTimeline("phone", undefined, undefined, ac.signal);
      expect(calls[0].init?.signal).toBe(ac.signal);
      restore();
    });
  });

  describe("getCronRunBody", () => {
    it("wraps runtime's raw {body} response", async () => {
      const client = await setupRemote();
      const { restore, calls } = await mockFetch(async () =>
        jsonResponse(200, {
          body: "[L000001][P000000][@system][20260511T090000Z] cron(weekly-report): hi\n",
        }),
      );

      const res = await client.getCronRunBody("phone", "weekly-report", "2026-05-11T09-00-00Z");

      expect(res.ok).toBe(true);
      expect(res.data?.body).toContain("cron(weekly-report): hi");
      expect(calls[0].url).toBe(
        `${RUNTIME_BASE}/workspaces/phone/crons/weekly-report/runs/2026-05-11T09-00-00Z`,
      );
      restore();
    });

    it("propagates 404 error_code from the runtime", async () => {
      const client = await setupRemote();
      const { restore } = await mockFetch(async () =>
        jsonResponse(404, {
          ok: false,
          error: "run not found",
          error_code: "not_found",
        }),
      );

      const res = await client.getCronRunBody("phone", "weekly-report", "2026-05-11T09-00-00Z");

      expect(res.ok).toBe(false);
      expect(res.error_code).toBe("not_found");
      restore();
    });
  });

  describe("showCron", () => {
    it("wraps runtime's raw CronDetail body in {ok: true, data}", async () => {
      const client = await setupRemote();
      const detail = {
        name: "weekly-report",
        spec: {
          version: 1,
          schedule: "0 9 * * 1",
          target: "alice",
          prompt: "summarize the week",
          enabled: true,
          created_by: "alice",
          created_at: "2026-05-01T00:00:00Z",
        },
        recent_runs: [{ ts: "2026-05-04T09-00-00Z", filename: "2026-05-04T09-00-00Z.thread" }],
        next_fire: "2026-05-11T09:00:00Z",
      };
      const { restore } = await mockFetch(async () => jsonResponse(200, detail));

      const res = await client.showCron("phone", "weekly-report");

      expect(res.ok).toBe(true);
      expect(res.data).toEqual(detail);
      expect(res.data?.spec.schedule).toBe("0 9 * * 1");
      restore();
    });
  });

  describe("listCrons", () => {
    it("wraps runtime's raw {crons} body in {ok: true, data}", async () => {
      const client = await setupRemote();
      const summary = [
        {
          name: "weekly-report",
          schedule: "0 9 * * 1",
          timezone: "UTC",
          target: "alice",
          enabled: true,
          created_by: "alice",
          created_at: "2026-05-01T00:00:00Z",
          next_fire: "2026-05-11T09:00:00Z",
        },
      ];
      const { restore } = await mockFetch(async () =>
        jsonResponse(200, { crons: summary }),
      );

      const res = await client.listCrons("phone");

      expect(res.ok).toBe(true);
      expect(res.data?.crons).toEqual(summary);
      restore();
    });
  });

  describe("listCronRuns", () => {
    it("wraps runtime's raw {runs} body in {ok: true, data}", async () => {
      const client = await setupRemote();
      const runs = [
        { ts: "2026-05-04T09-00-00Z", filename: "2026-05-04T09-00-00Z.thread" },
        { ts: "2026-05-11T09-00-00Z", filename: "2026-05-11T09-00-00Z.thread" },
      ];
      const { restore } = await mockFetch(async () => jsonResponse(200, { runs }));

      const res = await client.listCronRuns("phone", "weekly-report");

      expect(res.ok).toBe(true);
      expect(res.data?.runs).toEqual(runs);
      restore();
    });
  });

  describe("network failures", () => {
    it("translates fetch rejection to {ok: false, error}", async () => {
      const client = await setupRemote();
      const { restore } = await mockFetch(async () => {
        throw new TypeError("Failed to fetch");
      });

      const res = await client.getCronTimeline("phone");

      expect(res.ok).toBe(false);
      expect(res.error).toBe("Failed to fetch");
      restore();
    });

    it("returns local-mode placeholder when connection mode is local", async () => {
      vi.resetModules();
      const { useConnectionStore } = await import("@/hooks/use-connection-store");
      useConnectionStore.setState({ mode: "local", port: null });
      const client = await import("./client");

      const spy = vi.spyOn(globalThis, "fetch");
      const res = await client.getCronTimeline("phone");
      expect(res.ok).toBe(false);
      expect(res.error_code).toBe("runtime_required");
      expect(spy).not.toHaveBeenCalled();
      spy.mockRestore();
    });
  });
});
