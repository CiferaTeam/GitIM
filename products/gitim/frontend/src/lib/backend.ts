/**
 * Backend interface — abstracts the communication layer between gitim and
 * the IM engine. Two implementations:
 *   - HttpBackend: talks to gitim-runtime via HTTP (desktop)
 *   - LocalBackend: talks to daemon-web via Web Worker (mobile)
 */
import type {
  ApiResponse,
  BoardReadResponse,
  BoardSummary,
  BoardWriteResponse,
  CardStatus,
} from "./types";
import type {
  WorkerRequest,
  WorkerResponse,
  WorkerEvent,
} from "../daemon-web/worker";
import { clearSessionToken } from "./browser-workspaces";
import {
  useConnectionDiagnosticsStore,
  type BrowserSyncStatus,
} from "@/hooks/use-connection-diagnostics-store";

interface LocalBackendConfig {
  workspaceId: string;
  generation: number;
  onSyncReset?: () => void;
}

interface LocalInitConfig {
  workspaceId: string;
  remoteUrl: string;
  corsProxy: string;
  token: string | null;
  handler: string;
  storage: { fsName: string; repoDir: "/repo" };
}

interface LegacyLocalInitConfig {
  remoteUrl: string;
  corsProxy: string;
  token: string;
  handler: string;
}

const LOCAL_BACKEND_CLOSED_ERROR = "browser worker session closed";

function responseNeedsReconnect(response: ApiResponse): boolean {
  const data = response.data as Record<string, unknown> | undefined;
  return (
    response.error_code === "reconnect_required" ||
    data?.error_code === "reconnect_required" ||
    data?.needs_token === true
  );
}

export interface Backend {
  health(): Promise<ApiResponse>;
  me(): Promise<ApiResponse>;
  poll(since?: string, signal?: AbortSignal): Promise<ApiResponse>;
  channels(): Promise<ApiResponse>;
  read(channel: string, limit?: number, since?: number): Promise<ApiResponse>;
  send(
    channel: string,
    body: string,
    author?: string,
    replyTo?: number,
  ): Promise<ApiResponse>;
  thread(channel: string, line: number): Promise<ApiResponse>;
  users(): Promise<ApiResponse>;
  joinChannel(channel: string): Promise<ApiResponse>;
}

function isBrowserSyncStatus(value: unknown): value is BrowserSyncStatus {
  return (
    value === "idle" ||
    value === "syncing" ||
    value === "error" ||
    value === "reconnect_required"
  );
}

function recordBrowserHealth(response: ApiResponse): void {
  const data = response.data as Record<string, unknown> | undefined;
  if (!response.ok || data?.service !== "daemon-web") return;

  const syncStatus = isBrowserSyncStatus(data.sync_status)
    ? data.sync_status
    : data.needs_token === true
      ? "reconnect_required"
      : "unknown";

  useConnectionDiagnosticsStore.getState().recordBrowserSyncEvent({
    status: syncStatus,
    needsToken: data.needs_token === true,
    corsProxy: typeof data.cors_proxy === "string" ? data.cors_proxy : null,
    remoteUrl: typeof data.remote_url === "string" ? data.remote_url : null,
    headCommit: typeof data.head_commit === "string" ? data.head_commit : null,
  });
}

export interface CreateCardOptions {
  labels?: string[];
  assignee?: string | null;
  status?: CardStatus;
}

export interface ListCardsQuery {
  channel?: string | null;
  labels?: string[];
  status?: CardStatus | null;
  assignee?: string | null;
}

export interface ReadCardQuery {
  limit?: number;
  since?: number;
}

export interface UpdateCardPatch {
  status?: CardStatus;
  labels?: string[];
  assignee?: string | null;
}

export interface CardBackend {
  createCard(
    channel: string,
    title: string,
    opts?: CreateCardOptions,
  ): Promise<ApiResponse>;
  listCards(query?: ListCardsQuery): Promise<ApiResponse>;
  readCard(
    channel: string,
    cardId: string,
    query?: ReadCardQuery,
  ): Promise<ApiResponse>;
  sendCardMessage(
    channel: string,
    cardId: string,
    body: string,
    replyTo?: number,
  ): Promise<ApiResponse>;
  updateCard(
    channel: string,
    cardId: string,
    patch: UpdateCardPatch,
  ): Promise<ApiResponse>;
  archiveCard(channel: string, cardId: string): Promise<ApiResponse>;
  unarchiveCard(channel: string, cardId: string): Promise<ApiResponse>;
  listArchivedCards(channel?: string): Promise<ApiResponse>;
}

export interface ChannelArchiveBackend {
  archiveChannel(channel: string): Promise<ApiResponse>;
  unarchiveChannel(channel: string): Promise<ApiResponse>;
  listArchivedChannels(opts?: {
    prefix?: string;
    offset?: number;
    limit?: number;
  }): Promise<ApiResponse>;
}

export interface DmArchiveBackend {
  archiveDm(peer: string): Promise<ApiResponse>;
  unarchiveDm(peer: string): Promise<ApiResponse>;
  listArchivedDms(opts?: {
    prefix?: string;
    offset?: number;
    limit?: number;
  }): Promise<ApiResponse>;
}

export interface BoardBackend {
  listBoards(): Promise<ApiResponse<{ boards: BoardSummary[] }>>;
  showBoard(handler: string): Promise<ApiResponse<BoardReadResponse>>;
  initBoard(): Promise<ApiResponse<BoardWriteResponse>>;
  publishBoard(content?: string): Promise<ApiResponse<BoardWriteResponse>>;
  setBoard(field: string, value: string): Promise<ApiResponse<BoardWriteResponse>>;
  setBoardSection(
    section: string,
    value: string,
  ): Promise<ApiResponse<BoardWriteResponse>>;
  appendBoardSection(
    section: string,
    value: string,
  ): Promise<ApiResponse<BoardWriteResponse>>;
}

export class HttpBackend implements Backend {
  private baseUrl: () => string;

  constructor(baseUrl: () => string) {
    this.baseUrl = baseUrl;
  }

  async health(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/health`);
    if (!res.ok)
      return { ok: false, error: `health check failed: ${res.status}` };
    const data = await res.json();
    return { ok: true, data };
  }

  async me(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/me`);
    return await res.json();
  }

  async poll(since?: string, signal?: AbortSignal): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ since }),
      signal,
    });
    return await res.json();
  }

  async channels(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/channels`);
    return await res.json();
  }

  async read(
    channel: string,
    limit?: number,
    since?: number,
  ): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, limit, since }),
    });
    return await res.json();
  }

  async send(
    channel: string,
    body: string,
    _author?: string,
    replyTo?: number,
  ): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, body, reply_to: replyTo }),
    });
    return await res.json();
  }

  async thread(channel: string, line: number): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/thread`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, line }),
    });
    return await res.json();
  }

  async users(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/users`);
    return await res.json();
  }

  async joinChannel(channel: string): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/join`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel }),
    });
    return await res.json();
  }
}

/**
 * LocalBackend — communicates with daemon-web Web Worker via postMessage RPC.
 * Used in mobile/local mode where there is no gitim-runtime server.
 */
export class LocalBackend implements Backend {
  private worker: Worker;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: ApiResponse) => void; reject: (e: Error) => void }
  >();
  private onSyncReset?: () => void;
  private workspaceId: string;
  private generation: number;
  private closed = false;

  constructor(config: LocalBackendConfig);
  constructor(onSyncReset?: () => void);
  constructor(config: LocalBackendConfig | (() => void) = {
    workspaceId: "legacy",
    generation: 0,
  }) {
    if (typeof config === "function") {
      this.workspaceId = "legacy";
      this.generation = 0;
      this.onSyncReset = config;
    } else {
      this.workspaceId = config.workspaceId;
      this.generation = config.generation;
      this.onSyncReset = config.onSyncReset;
    }
    this.worker = new Worker(
      new URL("../daemon-web/worker.ts", import.meta.url),
      { type: "module" },
    );
    this.worker.onmessage = (event: MessageEvent) => {
      const data = event.data as WorkerResponse | WorkerEvent;
      if (
        data.workspaceId !== this.workspaceId ||
        data.generation !== this.generation
      ) {
        return;
      }

      if ("type" in data) {
        if (data.type === "sync_reset" || data.type === "repo_changed") {
          useConnectionDiagnosticsStore.getState().recordBrowserSyncEvent({
            status: "idle",
            headCommit: data.commit_id ?? null,
          });
          this.onSyncReset?.();
          const reset = (globalThis as unknown as Record<string, unknown>)
            .__gitimSyncReset;
          if (typeof reset === "function") reset();
          return;
        }
        if (data.type === "reconnect_required") {
          clearSessionToken(this.workspaceId);
          useConnectionDiagnosticsStore.getState().recordBrowserSyncEvent({
            status: "reconnect_required",
            error: data.error ?? "Reconnect token to sync this browser workspace.",
            needsToken: true,
            headCommit: data.commit_id ?? null,
          });
          return;
        }
        if (data.type === "sync_error") {
          useConnectionDiagnosticsStore.getState().recordBrowserSyncEvent({
            status: "error",
            error: data.error ?? "Browser sync failed",
          });
          return;
        }
      }

      // RPC response
      const resp = data as WorkerResponse;
      const handler = this.pending.get(resp.id);
      if (handler) {
        this.pending.delete(resp.id);
        if (resp.error) {
          handler.resolve({ ok: false, error: resp.error });
        } else {
          const result = resp.result as ApiResponse;
          if (responseNeedsReconnect(result)) {
            clearSessionToken(this.workspaceId);
          }
          handler.resolve(result);
        }
      }
    };
    this.worker.onerror = (event) => {
      this.rejectPending(event.message || "browser worker failed");
    };
    this.worker.onmessageerror = () => {
      this.rejectPending("browser worker sent an unreadable response");
    };
  }

  private rejectPending(error: string): void {
    for (const handler of this.pending.values()) {
      handler.reject(new Error(error));
    }
    this.pending.clear();
  }

  private call<T = Record<string, unknown>>(
    method: string,
    ...args: unknown[]
  ): Promise<ApiResponse<T>> {
    return this.callWithOptions(method, args);
  }

  private callWithOptions<T = Record<string, unknown>>(
    method: string,
    args: unknown[],
    options: {
      signal?: AbortSignal;
      abortError?: string;
    } = {},
  ): Promise<ApiResponse<T>> {
    if (this.closed) {
      return Promise.resolve({
        ok: false,
        error: LOCAL_BACKEND_CLOSED_ERROR,
      });
    }
    if (options.signal?.aborted) {
      return Promise.resolve({
        ok: false,
        error: options.abortError ?? "browser worker request aborted",
      });
    }

    return new Promise<ApiResponse<T>>((resolve, reject) => {
      const id = this.nextId++;
      const cleanup = () => {
        options.signal?.removeEventListener("abort", onAbort);
      };
      const onAbort = () => {
        this.pending.delete(id);
        cleanup();
        resolve({
          ok: false,
          error: options.abortError ?? "browser worker request aborted",
        });
      };
      this.pending.set(id, {
        resolve: (value) => {
          cleanup();
          if (method === "health") recordBrowserHealth(value);
          resolve(value as ApiResponse<T>);
        },
        reject: (error) => {
          cleanup();
          reject(error);
        },
      });
      options.signal?.addEventListener("abort", onAbort, { once: true });
      const request: WorkerRequest = {
        id,
        method,
        args,
        workspaceId: this.workspaceId,
        generation: this.generation,
      };
      try {
        this.worker.postMessage(request);
      } catch (error) {
        this.pending.delete(id);
        cleanup();
        resolve({
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    });
  }

  preflight(): Promise<ApiResponse> {
    return this.call("preflight");
  }

  async init(config: LocalInitConfig): Promise<ApiResponse>;
  async init(config: LegacyLocalInitConfig): Promise<ApiResponse>;
  async init(config: LocalInitConfig | LegacyLocalInitConfig): Promise<ApiResponse> {
    return this.call("init", config);
  }

  async startSync(): Promise<void> {
    await this.call("startSync");
  }

  async syncNow(): Promise<ApiResponse> {
    return this.call("syncNow");
  }

  health(): Promise<ApiResponse> {
    return this.call("health");
  }
  me(): Promise<ApiResponse> {
    return this.call("me");
  }
  poll(since?: string, signal?: AbortSignal): Promise<ApiResponse> {
    return this.callWithOptions("poll", [since], {
      signal,
      abortError: "browser worker poll aborted",
    });
  }
  channels(): Promise<ApiResponse> {
    return this.call("channels");
  }
  read(
    channel: string,
    limit?: number,
    since?: number,
  ): Promise<ApiResponse> {
    return this.call("read", channel, limit, since);
  }
  send(
    channel: string,
    body: string,
    author?: string,
    replyTo?: number,
  ): Promise<ApiResponse> {
    return this.call("send", channel, body, author, replyTo);
  }
  thread(channel: string, line: number): Promise<ApiResponse> {
    return this.call("thread", channel, line);
  }
  users(): Promise<ApiResponse> {
    return this.call("users");
  }
  listBoards(): Promise<ApiResponse<{ boards: BoardSummary[] }>> {
    return this.call("listBoards");
  }
  showBoard(handler: string): Promise<ApiResponse<BoardReadResponse>> {
    return this.call("showBoard", handler);
  }
  initBoard(): Promise<ApiResponse<BoardWriteResponse>> {
    return this.call("initBoard");
  }
  publishBoard(content?: string): Promise<ApiResponse<BoardWriteResponse>> {
    return this.call("publishBoard", content);
  }
  setBoard(field: string, value: string): Promise<ApiResponse<BoardWriteResponse>> {
    return this.call("setBoard", field, value);
  }
  setBoardSection(
    section: string,
    value: string,
  ): Promise<ApiResponse<BoardWriteResponse>> {
    return this.call("setBoardSection", section, value);
  }
  appendBoardSection(
    section: string,
    value: string,
  ): Promise<ApiResponse<BoardWriteResponse>> {
    return this.call("appendBoardSection", section, value);
  }
  joinChannel(channel: string): Promise<ApiResponse> {
    return this.call("joinChannel", channel);
  }
  archiveChannel(channel: string): Promise<ApiResponse> {
    return this.call("archiveChannel", channel);
  }
  unarchiveChannel(channel: string): Promise<ApiResponse> {
    return this.call("unarchiveChannel", channel);
  }
  listArchivedChannels(opts?: {
    prefix?: string;
    offset?: number;
    limit?: number;
  }): Promise<ApiResponse> {
    return this.call("listArchivedChannels", opts);
  }
  archiveDm(peer: string): Promise<ApiResponse> {
    return this.call("archiveDm", peer);
  }
  unarchiveDm(peer: string): Promise<ApiResponse> {
    return this.call("unarchiveDm", peer);
  }
  listArchivedDms(opts?: {
    prefix?: string;
    offset?: number;
    limit?: number;
  }): Promise<ApiResponse> {
    return this.call("listArchivedDms", opts);
  }
  createCard(
    channel: string,
    title: string,
    opts: CreateCardOptions = {},
  ): Promise<ApiResponse> {
    return this.call("createCard", channel, title, opts);
  }
  listCards(query: ListCardsQuery = {}): Promise<ApiResponse> {
    return this.call("listCards", query);
  }
  readCard(
    channel: string,
    cardId: string,
    query: ReadCardQuery = {},
  ): Promise<ApiResponse> {
    return this.call("readCard", channel, cardId, query);
  }
  sendCardMessage(
    channel: string,
    cardId: string,
    body: string,
    replyTo?: number,
  ): Promise<ApiResponse> {
    return this.call("sendCardMessage", channel, cardId, body, replyTo);
  }
  updateCard(
    channel: string,
    cardId: string,
    patch: UpdateCardPatch,
  ): Promise<ApiResponse> {
    return this.call("updateCard", channel, cardId, patch);
  }
  archiveCard(channel: string, cardId: string): Promise<ApiResponse> {
    return this.call("archiveCard", channel, cardId);
  }
  unarchiveCard(channel: string, cardId: string): Promise<ApiResponse> {
    return this.call("unarchiveCard", channel, cardId);
  }
  listArchivedCards(channel?: string): Promise<ApiResponse> {
    return this.call("listArchivedCards", channel);
  }

  terminate(): void {
    if (this.closed) return;

    this.closed = true;
    this.rejectPending(LOCAL_BACKEND_CLOSED_ERROR);
    this.worker.terminate();
  }
}
