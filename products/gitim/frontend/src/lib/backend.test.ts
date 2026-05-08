import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LocalBackend } from "./backend";
import type { WorkerEvent, WorkerResponse } from "../daemon-web/worker";

class StubWorker {
  static instances: StubWorker[] = [];
  postMessageError: Error | null = null;

  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  messages: unknown[] = [];
  terminated = false;
  terminateCount = 0;

  constructor() {
    StubWorker.instances.push(this);
  }

  postMessage(message: unknown): void {
    if (this.postMessageError) throw this.postMessageError;
    this.messages.push(message);
  }

  terminate(): void {
    this.terminated = true;
    this.terminateCount += 1;
  }

  emit(message: WorkerResponse | WorkerEvent): void {
    this.onmessage?.({ data: message } as MessageEvent);
  }
}

describe("LocalBackend", () => {
  beforeEach(() => {
    StubWorker.instances = [];
    vi.stubGlobal("Worker", StubWorker);
    localStorage.clear();
    sessionStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("scopes RPCs and ignores stale worker responses", async () => {
    const backend = new LocalBackend({
      workspaceId: "ws_current",
      generation: 2,
    });
    const worker = StubWorker.instances[0];

    const result = backend.health();

    expect(worker.messages[0]).toEqual({
      id: 1,
      method: "health",
      args: [],
      workspaceId: "ws_current",
      generation: 2,
    });

    let settled = false;
    result.then(() => {
      settled = true;
    });

    worker.emit({
      id: 1,
      workspaceId: "ws_current",
      generation: 1,
      result: { ok: true, data: "stale" },
    });
    await Promise.resolve();
    expect(settled).toBe(false);

    worker.emit({
      id: 1,
      workspaceId: "ws_current",
      generation: 2,
      result: { ok: true, data: "current" },
    });
    await expect(result).resolves.toEqual({ ok: true, data: "current" });
  });

  it("scopes sync reset events to the active session", () => {
    const onSyncReset = vi.fn();
    const globalReset = vi.fn();
    (
      globalThis as unknown as Record<string, unknown>
    ).__gitimSyncReset = globalReset;
    const backend = new LocalBackend({
      workspaceId: "ws_current",
      generation: 2,
      onSyncReset,
    });
    const worker = StubWorker.instances[0];

    worker.emit({
      type: "sync_reset",
      workspaceId: "ws_current",
      generation: 1,
    });
    worker.emit({
      type: "sync_reset",
      workspaceId: "ws_current",
      generation: 2,
    });

    expect(onSyncReset).toHaveBeenCalledTimes(1);
    expect(globalReset).toHaveBeenCalledTimes(1);
    backend.terminate();
    delete (
      globalThis as unknown as Record<string, unknown>
    ).__gitimSyncReset;
  });

  it("rejects pending RPCs when terminated", async () => {
    const backend = new LocalBackend({
      workspaceId: "ws_current",
      generation: 2,
    });
    const worker = StubWorker.instances[0];

    const result = backend.health();
    backend.terminate();

    expect(worker.terminated).toBe(true);
    await expect(result).rejects.toThrow("browser worker session closed");
  });

  it("returns a closed response for calls after terminate", async () => {
    const backend = new LocalBackend({
      workspaceId: "ws_current",
      generation: 2,
    });
    const worker = StubWorker.instances[0];

    backend.terminate();
    backend.terminate();
    const result = await backend.health();

    expect(result).toEqual({
      ok: false,
      error: "browser worker session closed",
    });
    expect(worker.messages).toEqual([]);
    expect(worker.terminated).toBe(true);
    expect(worker.terminateCount).toBe(1);
  });

  it("resolves cleanly when posting to the worker throws", async () => {
    const backend = new LocalBackend({
      workspaceId: "ws_current",
      generation: 2,
    });
    const worker = StubWorker.instances[0];
    worker.postMessageError = new Error("worker port closed");

    const result = await backend.health();

    expect(result).toEqual({
      ok: false,
      error: "worker port closed",
    });
    expect(worker.messages).toEqual([]);
    worker.postMessageError = null;
    worker.emit({
      id: 1,
      workspaceId: "ws_current",
      generation: 2,
      result: { ok: true, data: "late" },
    });
    const next = backend.channels();
    expect(worker.messages[0]).toEqual({
      id: 2,
      method: "channels",
      args: [],
      workspaceId: "ws_current",
      generation: 2,
    });
    worker.emit({
      id: 2,
      workspaceId: "ws_current",
      generation: 2,
      result: { ok: true, data: "next" },
    });
    await expect(next).resolves.toEqual({ ok: true, data: "next" });
  });

  it("clears the session token when the worker reports reconnect required", async () => {
    const { loadSessionToken, saveSessionToken } = await import("./browser-workspaces");
    saveSessionToken("ws_current", "github_pat_stale");
    const backend = new LocalBackend({
      workspaceId: "ws_current",
      generation: 2,
    });
    const worker = StubWorker.instances[0];

    const result = backend.poll("old-head");

    worker.emit({
      id: 1,
      workspaceId: "ws_current",
      generation: 2,
      result: {
        ok: true,
        data: {
          commit_id: "old-head",
          changes: [],
          needs_token: true,
        },
        error_code: "reconnect_required",
      },
    });

    await expect(result).resolves.toEqual({
      ok: true,
      data: {
        commit_id: "old-head",
        changes: [],
        needs_token: true,
      },
      error_code: "reconnect_required",
    });
    expect(loadSessionToken("ws_current")).toBeUndefined();
  });
});
