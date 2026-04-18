/**
 * Unified client — all methods hit the real runtime HTTP API.
 * Agent methods fall back to mock if runtime is unreachable.
 */
import type { Agent, ApiResponse, Card, CardStatus, Message } from "./types";
import type { PreflightResult, ProviderId } from "./providers";
import * as mockClient from "./mock/client";
import { useConnectionStore } from "@/hooks/use-connection-store";

// --- Helpers ---

function baseUrl(): string {
  return useConnectionStore.getState().baseUrl();
}

// --- Health ---

export async function health(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/health`);
  if (!res.ok) return { ok: false, error: `health check failed: ${res.status}` };
  const data = await res.json();
  return { ok: true, data };
}

// --- IM methods: real runtime HTTP ---

export async function me(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/me`);
  return await res.json();
}

export async function poll(since?: string): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/poll`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ since }),
  });
  return await res.json();
}

export async function channels(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/channels`);
  return await res.json();
}

export async function send(
  channel: string,
  body: string,
  _author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/send`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, body, reply_to: replyTo }),
  });
  return await res.json();
}

export async function createChannel(
  name: string,
  displayName?: string,
  introduction?: string,
  invitees?: string[],
): Promise<ApiResponse> {
  const payload: Record<string, unknown> = { name, display_name: displayName, introduction };
  if (invitees && invitees.length > 0) {
    payload.invitees = invitees;
  }
  const res = await fetch(`${baseUrl()}/im/create-channel`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export async function joinChannel(channel: string, targets?: string[]): Promise<ApiResponse> {
  const payload: Record<string, unknown> = { channel };
  if (targets && targets.length > 0) {
    payload.targets = targets;
  }
  const res = await fetch(`${baseUrl()}/im/join`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export async function read(
  channel: string,
  limit?: number,
): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/read`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, limit }),
  });
  return await res.json();
}

export async function thread(
  channel: string,
  line: number,
): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/thread`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, line }),
  });
  return await res.json();
}

export async function users(): Promise<ApiResponse> {
  const res = await fetch(`${baseUrl()}/im/users`);
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

// --- Card API: real runtime HTTP ---

export interface CreateCardOpts {
  labels?: string[];
  assignee?: string | null;
  status?: CardStatus;
}

export async function createCard(
  channel: string,
  title: string,
  opts: CreateCardOpts = {},
): Promise<ApiResponse<{ channel: string; card_id: string; title: string }>> {
  const payload: Record<string, unknown> = { channel, title };
  if (opts.labels && opts.labels.length > 0) payload.labels = opts.labels;
  if (opts.assignee) payload.assignee = opts.assignee;
  if (opts.status) payload.status = opts.status;
  const res = await fetch(`${baseUrl()}/im/cards`, {
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
  const url = qs ? `${baseUrl()}/im/cards?${qs}` : `${baseUrl()}/im/cards`;
  const res = await fetch(url);
  return await res.json();
}

export interface ReadCardQuery {
  limit?: number;
  since?: number;
}

export async function readCard(
  channel: string,
  cardId: string,
  query: ReadCardQuery = {},
): Promise<ApiResponse<{ meta: Card; entries: Message[] }>> {
  const params = new URLSearchParams();
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.since != null) params.set("since", String(query.since));
  const qs = params.toString();
  const base = `${baseUrl()}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}`;
  const url = qs ? `${base}?${qs}` : base;
  const res = await fetch(url);
  return await res.json();
}

export async function sendCardMessage(
  channel: string,
  cardId: string,
  body: string,
  replyTo?: number,
): Promise<ApiResponse> {
  const res = await fetch(
    `${baseUrl()}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/messages`,
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
  channel: string,
  cardId: string,
  patch: UpdateCardPatch,
): Promise<ApiResponse> {
  const res = await fetch(
    `${baseUrl()}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}`,
    {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    },
  );
  return await res.json();
}

// --- Preflight ---

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
  };
}

// --- Agent API: real runtime HTTP ---

export async function listAgents(): Promise<ApiResponse> {
  try {
    const res = await fetch(`${baseUrl()}/agents`);
    const data = await res.json();
    if (!data.ok) return data;
    const agents = (data.agents ?? []).map(mapBackendAgent);
    return { ok: true, data: { agents } };
  } catch {
    return mockClient.listAgents();
  }
}

export async function getAgent(id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${baseUrl()}/agents/${id}`);
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: mapBackendAgent(data.agent) } };
  } catch {
    return mockClient.getAgent(id);
  }
}

export async function addAgent(
  name: string,
  provider: ProviderId,
  systemPrompt: string,
  model?: string,
  env?: Record<string, string>,
): Promise<ApiResponse> {
  try {
    const handler = toHandler(name);
    const res = await fetch(`${baseUrl()}/agents/add`, {
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
    const agentRes = await getAgent(data.id ?? handler);
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

export async function removeAgent(id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${baseUrl()}/agents/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    return await res.json();
  } catch {
    return mockClient.removeAgent(id);
  }
}

export async function startAgent(id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${baseUrl()}/agents/start`, {
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

export async function stopAgent(id: string): Promise<ApiResponse> {
  try {
    const res = await fetch(`${baseUrl()}/agents/stop`, {
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
