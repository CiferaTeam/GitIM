// Web Worker entry point for daemon-web.
// Receives RPC messages from LocalBackend, dispatches to handlers.

import "./browser-polyfills";
import * as handlers from "./handlers";
import { reconcileOrphanCards } from "./handlers";
import { startSyncLoop, stopSyncLoop } from "./sync";

export interface WorkerRequest {
  id: number;
  method: string;
  args: unknown[];
  workspaceId: string;
  generation: number;
}

export interface WorkerResponse {
  id: number;
  workspaceId: string;
  generation: number;
  result?: unknown;
  error?: string;
}

// Also used for unsolicited messages from sync
export interface WorkerEvent {
  type: "sync_reset" | "repo_changed" | "sync_error" | "reconnect_required";
  workspaceId: string;
  generation: number;
  commit_id?: string;
  reason?: "fast_forward" | "rebase" | "push";
  error?: string;
  error_code?: string;
  needs_token?: boolean;
}

let currentWorkspaceId = "";
let currentGeneration = 0;
const postWorkerMessage = self.postMessage.bind(self);

function isUnscopedWorkerEvent(
  message: unknown,
): message is { type: WorkerEvent["type"] } {
  if (!message || typeof message !== "object") return false;
  const data = message as Record<string, unknown>;
  return (
    (
      data.type === "sync_reset" ||
      data.type === "repo_changed" ||
      data.type === "sync_error" ||
      data.type === "reconnect_required"
    ) &&
    (typeof data.workspaceId !== "string" ||
      typeof data.generation !== "number")
  );
}

self.postMessage = ((message: unknown) => {
  const scopedMessage = isUnscopedWorkerEvent(message)
    ? {
        ...message,
        workspaceId: currentWorkspaceId,
        generation: currentGeneration,
      }
    : message;
  postWorkerMessage(scopedMessage);
}) as typeof self.postMessage;

const handler: Record<
  string,
  (...args: unknown[]) => Promise<unknown>
> = {
  preflight: () => handlers.preflight(),
  init: async (config: unknown) => {
    const result = await handlers.init(
      config as {
        workspaceId: string;
        remoteUrl: string;
        corsProxy: string;
        token: string | null;
        handler: string;
        storage: { fsName: string; repoDir: "/repo" };
      },
    );
    // Best-effort: migrate legacy orphan card dirs left by old archiveChannel.
    // Non-blocking — a failure here must not prevent the worker from serving.
    // Only fires on successful init; on failure init has cleared state, so
    // reconcileOrphanCards's getState() would throw spurious warnings.
    if (result.ok) {
      reconcileOrphanCards().catch((e: unknown) => {
        console.warn("[daemon-web] reconcileOrphanCards failed at boot:", e);
      });
    }
    return result;
  },
  health: () => handlers.health(),
  me: () => handlers.me(),
  poll: (since?: unknown) => handlers.poll(since as string | undefined),
  channels: () => handlers.channels(),
  read: (channel: unknown, limit?: unknown, since?: unknown) =>
    handlers.read(
      channel as string,
      limit as number | undefined,
      since as number | undefined,
    ),
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
  listBoards: () => handlers.listBoards(),
  showBoard: (handler: unknown) =>
    handlers.showBoard(handler as string),
  initBoard: () => handlers.initBoard(),
  publishBoard: (content?: unknown) =>
    handlers.publishBoard(content as string | undefined),
  setBoard: (field: unknown, value: unknown) =>
    handlers.setBoard(field as string, value as string),
  setBoardSection: (section: unknown, value: unknown) =>
    handlers.setBoardSectionValue(section as string, value as string),
  appendBoardSection: (section: unknown, value: unknown) =>
    handlers.appendBoardSectionValue(section as string, value as string),
  joinChannel: (channel: unknown) =>
    handlers.joinChannel(channel as string),
  archiveChannel: (channel: unknown) =>
    handlers.archiveChannel(channel as string),
  unarchiveChannel: (channel: unknown) =>
    handlers.unarchiveChannel(channel as string),
  listArchivedChannels: () =>
    handlers.listArchivedChannels(),
  archiveDm: (peer: unknown) => handlers.archiveDm(peer as string),
  unarchiveDm: (peer: unknown) => handlers.unarchiveDm(peer as string),
  listArchivedDms: (payload: unknown) =>
    handlers.listArchivedDms(
      payload as { prefix?: string; offset?: number; limit?: number } | undefined,
    ),
  listCards: (query?: unknown) =>
    handlers.listCards((query ?? {}) as handlers.ListCardsQuery),
  createCard: (channel: unknown, title: unknown, opts?: unknown) =>
    handlers.createCard(
      channel as string,
      title as string,
      (opts ?? {}) as handlers.CreateCardOptions,
    ),
  readCard: (channel: unknown, cardId: unknown, query?: unknown) =>
    handlers.readCard(
      channel as string,
      cardId as string,
      (query ?? {}) as handlers.ReadCardQuery,
    ),
  sendCardMessage: (
    channel: unknown,
    cardId: unknown,
    body: unknown,
    replyTo?: unknown,
  ) =>
    handlers.sendCardMessage(
      channel as string,
      cardId as string,
      body as string,
      replyTo as number | undefined,
    ),
  updateCard: (channel: unknown, cardId: unknown, patch: unknown) =>
    handlers.updateCard(
      channel as string,
      cardId as string,
      (patch ?? {}) as handlers.UpdateCardPatch,
    ),
  archiveCard: (channel: unknown, cardId: unknown) =>
    handlers.archiveCard(channel as string, cardId as string),
  unarchiveCard: (channel: unknown, cardId: unknown) =>
    handlers.unarchiveCard(channel as string, cardId as string),
  listArchivedCards: (channel?: unknown) =>
    handlers.listArchivedCards(channel as string | undefined),
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
  const { id, method, args, workspaceId, generation } = event.data;
  currentWorkspaceId = workspaceId;
  currentGeneration = generation;

  const fn = handler[method];
  if (!fn) {
    const response: WorkerResponse = {
      id,
      workspaceId,
      generation,
      error: `unknown method: ${method}`,
    };
    self.postMessage(response);
    return;
  }

  try {
    const result = await fn(...args);
    const response: WorkerResponse = { id, workspaceId, generation, result };
    self.postMessage(response);
  } catch (e) {
    const response: WorkerResponse = {
      id,
      workspaceId,
      generation,
      error: String((e as Error).message ?? e),
    };
    self.postMessage(response);
  }
};
