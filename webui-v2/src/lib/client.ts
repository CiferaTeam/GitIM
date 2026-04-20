/**
 * Unified client - all methods hit the real runtime HTTP API.
 * Agent methods fall back to mock if runtime is unreachable.
 *
 * Workspace-scoped methods take `slug` as the first parameter.
 * Global (unscoped) methods: health, listWorkspaces, createWorkspace,
 * deleteWorkspace, getWorkspace, preflightProvider.
 */
import type {
  Agent,
  ApiResponse,
  Card,
  CardStatus,
  Channel,
  CreateWorkspaceRequest,
  Message,
  WorkspaceSummary,
} from "./types";
import type { PreflightResult, ProviderId } from "./providers";
import * as mockClient from "./mock/client";
import { useConnectionStore } from "@/hooks/use-connection-store";

// --- Helpers ---

function baseUrl(): string {
  return useConnectionStore.getState().baseUrl();
}

function wsBase(slug: string): string {
  return `${baseUrl()}/workspaces/${encodeURIComponent(slug)}`;
}

// --- Health ---

export async function health(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/health`);
  if (!res.ok) return { ok: false, error: `health check failed: ${res.status}` };
  const data = await res.json();
  return { ok: true, data };
}

// --- Runtime self-update ---

export interface UpdateAndRestartData {
  job_id: string;
  target_version: string;
  started_at: string;
}

/**
 * POST /runtime/update-and-restart — kicks off self-update (Task 6/7).
 * Returns 202 on accept. After accept, the runtime HTTP server will stop
 * responding until the new binary re-binds the port; callers are expected
 * to poll `health()` to detect the transition.
 */
export async function updateAndRestart(): Promise<ApiResponse<UpdateAndRestartData>> {
  try {
    const res = await fetch(`${baseUrl()}/runtime/update-and-restart`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      return {
        ok: false,
        error: data.detail ?? data.error ?? `HTTP ${res.status}`,
        error_code: data.error_code,
      };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

// --- Workspace CRUD (global, no slug) ---

export async function listWorkspaces(): Promise<
  ApiResponse<{ workspaces: WorkspaceSummary[] }>
> {
  try {
    const res = await fetch(`${baseUrl()}/workspaces`);
    const data = await res.json();
    if (!res.ok) {
      return { ok: false, error: data.error ?? `HTTP ${res.status}`, error_code: data.error_code };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function createWorkspace(
  req: CreateWorkspaceRequest,
): Promise<ApiResponse<{ slug: string; workspace_name: string; path: string; provider: string }>> {
  try {
    const res = await fetch(`${baseUrl()}/workspaces`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    });
    const data = await res.json();
    if (!res.ok || data.ok === false) {
      return {
        ok: false,
        error: data.error ?? `HTTP ${res.status}`,
        error_code: data.error_code,
      };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function getWorkspace(slug: string): Promise<ApiResponse> {
  try {
    const res = await fetch(wsBase(slug));
    const data = await res.json();
    if (!res.ok) {
      return { ok: false, error: data.error ?? `HTTP ${res.status}`, error_code: data.error_code };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function deleteWorkspace(slug: string): Promise<ApiResponse> {
  try {
    const res = await fetch(wsBase(slug), { method: "DELETE" });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      return { ok: false, error: data.error ?? `HTTP ${res.status}`, error_code: data.error_code };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

// --- IM methods: real runtime HTTP (all scoped to a workspace) ---

export async function me(slug: string): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/me`);
  return await res.json();
}

export async function poll(slug: string, since?: string): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/poll`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ since }),
  });
  return await res.json();
}

export async function channels(slug: string): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/channels`);
  return await res.json();
}

export async function send(
  slug: string,
  channel: string,
  body: string,
  _author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/send`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, body, reply_to: replyTo }),
  });
  return await res.json();
}

export async function createChannel(
  slug: string,
  name: string,
  displayName?: string,
  introduction?: string,
  invitees?: string[],
): Promise<ApiResponse> {
  const payload: Record<string, unknown> = { name, display_name: displayName, introduction };
  if (invitees && invitees.length > 0) {
    payload.invitees = invitees;
  }
  const res = await fetch(`${wsBase(slug)}/im/create-channel`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export async function joinChannel(
  slug: string,
  channel: string,
  targets?: string[],
): Promise<ApiResponse> {
  const payload: Record<string, unknown> = { channel };
  if (targets && targets.length > 0) {
    payload.targets = targets;
  }
  const res = await fetch(`${wsBase(slug)}/im/join`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export async function read(
  slug: string,
  channel: string,
  limit?: number,
): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/read`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, limit }),
  });
  return await res.json();
}

export async function thread(
  slug: string,
  channel: string,
  line: number,
): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/thread`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, line }),
  });
  return await res.json();
}

export async function users(slug: string): Promise<ApiResponse> {
  const res = await fetch(`${wsBase(slug)}/im/users`);
  return await res.json();
}

/** Sanitize a display name into a valid handler (a-z, 0-9, hyphens). */
export function toHandler(name: string): string {
  return name
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^a-z0-9-]/g, "")
    .replace(/-{2,}/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 39);
}

/** Validate a handler. Returns error message or null if valid. */
export function validateHandler(name: string): string | null {
  const handler = toHandler(name);
  if (!handler) return "Name must contain at least one letter or digit";
  if (handler === "system") return "\"system\" is reserved";
  return null;
}

/** Validate a channel name. Returns error message or null if valid. */
export function validateChannelName(name: string): string | null {
  if (!name) return "Channel name is required";
  if (name.length > 32) return "Channel name must be 32 characters or less";
  if (!/^[a-z0-9-]+$/.test(name)) return "Only lowercase letters, numbers, and hyphens";
  if (name.startsWith("-") || name.endsWith("-")) return "Cannot start or end with a hyphen";
  if (name.includes("--")) return "Cannot contain consecutive hyphens";
  return null;
}

// --- Card API: real runtime HTTP (all scoped to a workspace) ---

export interface CreateCardOpts {
  labels?: string[];
  assignee?: string | null;
  status?: CardStatus;
}

export async function createCard(
  slug: string,
  channel: string,
  title: string,
  opts: CreateCardOpts = {},
): Promise<ApiResponse<{ channel: string; card_id: string; title: string }>> {
  const payload: Record<string, unknown> = { channel, title };
  if (opts.labels && opts.labels.length > 0) payload.labels = opts.labels;
  if (opts.assignee) payload.assignee = opts.assignee;
  if (opts.status) payload.status = opts.status;
  const res = await fetch(`${wsBase(slug)}/im/cards`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export interface ListCardsQuery {
  channel?: string | null;
  labels?: string[];
  status?: CardStatus | null;
  assignee?: string | null;
}

export async function listCards(
  slug: string,
  query: ListCardsQuery = {},
): Promise<ApiResponse<{ cards: Card[] }>> {
  const params = new URLSearchParams();
  if (query.channel) params.set("channel", query.channel);
  if (query.status) params.set("status", query.status);
  if (query.assignee) params.set("assignee", query.assignee);
  if (query.labels) {
    for (const l of query.labels) params.append("label", l);
  }
  const qs = params.toString();
  const url = qs ? `${wsBase(slug)}/im/cards?${qs}` : `${wsBase(slug)}/im/cards`;
  const res = await fetch(url);
  return await res.json();
}

export interface ReadCardQuery {
  limit?: number;
  since?: number;
}

export async function readCard(
  slug: string,
  channel: string,
  cardId: string,
  query: ReadCardQuery = {},
): Promise<ApiResponse<{ meta: Card; entries: Message[]; archived: boolean }>> {
  const params = new URLSearchParams();
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.since != null) params.set("since", String(query.since));
  const qs = params.toString();
  const base = `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}`;
  const url = qs ? `${base}?${qs}` : base;
  const res = await fetch(url);
  return await res.json();
}

export async function sendCardMessage(
  slug: string,
  channel: string,
  cardId: string,
  body: string,
  replyTo?: number,
): Promise<ApiResponse> {
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/messages`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ body, reply_to: replyTo }),
    },
  );
  return await res.json();
}

export interface UpdateCardPatch {
  status?: CardStatus;
  labels?: string[];
  assignee?: string | null;
}

export async function updateCard(
  slug: string,
  channel: string,
  cardId: string,
  patch: UpdateCardPatch,
): Promise<ApiResponse> {
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}`,
    {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    },
  );
  return await res.json();
}

// --- Archive API: runtime derives author from workspace me.json, so no body needed. ---

export async function archiveCard(
  slug: string,
  channel: string,
  cardId: string,
): Promise<ApiResponse> {
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/archive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function unarchiveCard(
  slug: string,
  channel: string,
  cardId: string,
): Promise<ApiResponse> {
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/unarchive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function listArchivedCards(
  slug: string,
  channel?: string,
): Promise<ApiResponse<{ cards: Card[] }>> {
  const qs = channel ? `?channel=${encodeURIComponent(channel)}` : "";
  const res = await fetch(`${wsBase(slug)}/im/cards/archived${qs}`);
  return await res.json();
}

export async function archiveChannel(
  slug: string,
  name: string,
): Promise<ApiResponse> {
  const res = await fetch(
    `${wsBase(slug)}/im/channels/${encodeURIComponent(name)}/archive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function unarchiveChannel(
  slug: string,
  name: string,
): Promise<ApiResponse> {
  const res = await fetch(
    `${wsBase(slug)}/im/channels/${encodeURIComponent(name)}/unarchive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function listArchivedChannels(
  slug: string,
): Promise<ApiResponse<{ channels: Channel[] }>> {
  const res = await fetch(`${wsBase(slug)}/im/channels/archived`);
  return await res.json();
}

// --- Preflight (global, no slug) ---

export async function preflightProvider(
  provider: ProviderId,
): Promise<ApiResponse<PreflightResult>> {
  try {
    const res = await fetch(`${baseUrl()}/preflight/${provider}`);
    const data = await res.json();
    if (res.ok) {
      return { ok: true, data };
    }
    return { ok: false, error: data.error ?? `HTTP ${res.status}` };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

function mapBackendAgent(raw: Record<string, unknown>): Agent {
  const rawUsage = raw.session_usage as Record<string, unknown> | undefined;
  const sessionUsage: Agent["sessionUsage"] = rawUsage
    ? {
        sessionId: (rawUsage.session_id as string) ?? "",
        inputTokens: rawUsage.input_tokens as number | undefined,
        outputTokens: rawUsage.output_tokens as number | undefined,
        maxTokens: rawUsage.max_tokens as number | undefined,
        usedPercent: (rawUsage.used_percent as number) ?? 0,
        source: (rawUsage.source as "provider_reported" | "runtime_estimated") ?? "provider_reported",
        updatedAt: (rawUsage.updated_at as string) ?? "",
      }
    : undefined;

  return {
    id: (raw.id ?? raw.handler) as string,
    name: (raw.display_name ?? raw.handler) as string,
    status: ((raw.status as string) === "idle" ? "offline" : raw.status) as Agent["status"],
    provider: (raw.provider as ProviderId) ?? undefined,
    systemPrompt: (raw.system_prompt as string) ?? "",
    model: (raw.model as string) ?? undefined,
    env: (raw.env as Record<string, string>) ?? undefined,
    repoPath: (raw.repo_path as string) ?? "",
    messagesProcessed: (raw.messages_processed as number) ?? 0,
    lastActivity: raw.last_activity as string | undefined,
    errorMessage: (raw.error_message as string) ?? undefined,
    sessionUsage,
  };
}

// --- Agent API: real runtime HTTP (all scoped to a workspace) ---

export async function listAgents(slug: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${wsBase(slug)}/agents`);
    const data = await res.json();
    if (!data.ok) return data;
    const agents = (data.agents ?? []).map(mapBackendAgent);
    return { ok: true, data: { agents } };
  } catch {
    return mockClient.listAgents();
  }
}

export async function getAgent(slug: string, id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${wsBase(slug)}/agents/${id}`);
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: mapBackendAgent(data.agent) } };
  } catch {
    return mockClient.getAgent(id);
  }
}

export async function addAgent(
  slug: string,
  name: string,
  provider: ProviderId,
  systemPrompt: string,
  model?: string,
  env?: Record<string, string>,
): Promise<ApiResponse> {
  try {
    const handler = toHandler(name);
    const res = await fetch(`${wsBase(slug)}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        handler,
        display_name: name,
        provider,
        model: model || undefined,
        system_prompt: systemPrompt || undefined,
        env: env && Object.keys(env).length > 0 ? env : undefined,
      }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    // Fetch the full agent info from backend (has repo_path etc.)
    const agentRes = await getAgent(slug, data.id ?? handler);
    if (agentRes.ok && agentRes.data?.agent) {
      return agentRes;
    }
    // Fallback: construct locally if fetch fails
    const agent: Agent = {
      id: data.id ?? handler,
      name,
      status: "offline",
      provider,
      systemPrompt,
      model,
      env,
      repoPath: "",
      messagesProcessed: 0,
    };
    return { ok: true, data: { agent } };
  } catch {
    return mockClient.addAgent(name, provider, systemPrompt);
  }
}

export async function removeAgent(slug: string, id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${wsBase(slug)}/agents/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    return await res.json();
  } catch {
    return mockClient.removeAgent(id);
  }
}

export async function startAgent(slug: string, id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${wsBase(slug)}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: { status: "running" } } };
  } catch {
    return mockClient.startAgent(id);
  }
}

export async function stopAgent(slug: string, id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${wsBase(slug)}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: { status: "offline" } } };
  } catch {
    return mockClient.stopAgent(id);
  }
}
