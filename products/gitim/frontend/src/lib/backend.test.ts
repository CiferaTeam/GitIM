import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LocalBackend } from "./backend";
import type { WorkerEvent, WorkerResponse } from "../daemon-web/worker";

class StubWorker {
  static instances: StubWorker[] = [];

  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  messages: unknown[] = [];
  terminated = false;

  constructor() {
    StubWorker.instances.push(this);
  }

  postMessage(message: unknown): void {
    this.messages.push(message);
  }

  terminate(): void {
    this.terminated = true;
  }

  emit(message: WorkerResponse | WorkerEvent): void {
    this.onmessage?.({ data: message } as MessageEvent);
  }
}

describe("LocalBackend", () => {
  beforeEach(() => {
    StubWorker.instances = [];
    vi.stubGlobal("Worker", StubWorker);
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
});
