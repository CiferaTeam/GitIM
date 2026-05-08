/**
 * Backend interface — abstracts the communication layer between gitim and
 * the IM engine. Two implementations:
 *   - HttpBackend: talks to gitim-runtime via HTTP (desktop)
 *   - LocalBackend: talks to daemon-web via Web Worker (mobile)
 */
import type { ApiResponse } from "./types";
import type { CardStatus } from "./types";
import type {
  WorkerRequest,
  WorkerResponse,
  WorkerEvent,
} from "../daemon-web/worker";
import { clearSessionToken } from "./browser-workspaces";

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
  poll(since?: string): Promise<ApiResponse>;
  channels(): Promise<ApiResponse>;
  read(channel: string, limit?: number): Promise<ApiResponse>;
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
  listArchivedChannels(): Promise<ApiResponse>;
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

  async poll(since?: string): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ since }),
    });
    return await res.json();
  }

  async channels(): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/channels`);
    return await res.json();
  }

  async read(channel: string, limit?: number): Promise<ApiResponse> {
    const res = await fetch(`${this.baseUrl()}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel, limit }),
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

      // Unsolicited events from sync loop
      if ("type" in data && data.type === "sync_reset") {
        this.onSyncReset?.();
        const reset = (globalThis as unknown as Record<string, unknown>)
          .__gitimSyncReset;
        if (typeof reset === "function") reset();
        return;
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

  private call(method: string, ...args: unknown[]): Promise<ApiResponse> {
    if (this.closed) {
      return Promise.resolve({
        ok: false,
        error: LOCAL_BACKEND_CLOSED_ERROR,
      });
    }

    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });
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

  health(): Promise<ApiResponse> {
    return this.call("health");
  }
  me(): Promise<ApiResponse> {
    return this.call("me");
  }
  poll(since?: string): Promise<ApiResponse> {
    return this.call("poll", since);
  }
  channels(): Promise<ApiResponse> {
    return this.call("channels");
  }
  read(channel: string, limit?: number): Promise<ApiResponse> {
    return this.call("read", channel, limit);
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
  joinChannel(channel: string): Promise<ApiResponse> {
    return this.call("joinChannel", channel);
  }
  archiveChannel(channel: string): Promise<ApiResponse> {
    return this.call("archiveChannel", channel);
  }
  unarchiveChannel(channel: string): Promise<ApiResponse> {
    return this.call("unarchiveChannel", channel);
  }
  listArchivedChannels(): Promise<ApiResponse> {
    return this.call("listArchivedChannels");
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
