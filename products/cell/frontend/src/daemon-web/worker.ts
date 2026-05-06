// Web Worker entry point for daemon-web.
// Receives RPC messages from LocalBackend, dispatches to handlers.

import "./browser-polyfills";
import * as handlers from "./handlers";
import { startSyncLoop, stopSyncLoop } from "./sync";

export interface WorkerRequest {
  id: number;
  method: string;
  args: unknown[];
}

export interface WorkerResponse {
  id: number;
  result?: unknown;
  error?: string;
}

// Also used for unsolicited messages from sync
export interface WorkerEvent {
  type: "sync_reset" | "sync_error";
}

const handler: Record<
  string,
  (...args: unknown[]) => Promise<unknown>
> = {
  preflight: () => handlers.preflight(),
  init: (config: unknown) =>
    handlers.init(
      config as {
        remoteUrl: string;
        corsProxy: string;
        token: string;
        handler: string;
      },
    ),
  health: () => handlers.health(),
  me: () => handlers.me(),
  poll: (since?: unknown) => handlers.poll(since as string | undefined),
  channels: () => handlers.channels(),
  read: (channel: unknown, limit?: unknown) =>
    handlers.read(channel as string, limit as number | undefined),
  send: (
    channel: unknown,
    body: unknown,
    author?: unknown,
    replyTo?: unknown,
  ) =>
    handlers.send(
      channel as string,
      body as string,
      author as string | undefined,
      replyTo as number | undefined,
    ),
  thread: (channel: unknown, line: unknown) =>
    handlers.thread(channel as string, line as number),
  users: () => handlers.users(),
  joinChannel: (channel: unknown) =>
    handlers.joinChannel(channel as string),
  startSync: () => {
    startSyncLoop();
    return Promise.resolve({ ok: true });
  },
  stopSync: () => {
    stopSyncLoop();
    return Promise.resolve({ ok: true });
  },
};

self.onmessage = async (event: MessageEvent<WorkerRequest>) => {
  const { id, method, args } = event.data;

  const fn = handler[method];
  if (!fn) {
    const response: WorkerResponse = {
      id,
      error: `unknown method: ${method}`,
    };
    self.postMessage(response);
    return;
  }

  try {
    const result = await fn(...args);
    const response: WorkerResponse = { id, result };
    self.postMessage(response);
  } catch (e) {
    const response: WorkerResponse = {
      id,
      error: String((e as Error).message ?? e),
    };
    self.postMessage(response);
  }
};
