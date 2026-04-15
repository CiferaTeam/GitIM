/**
 * Unified client — IM methods delegate to the active Backend instance.
 * Agent methods always hit the runtime HTTP API (desktop only).
 */
import type { Agent, ApiResponse } from "./types";
import type { Backend } from "./backend";
import { HttpBackend } from "./backend";
import * as mockClient from "./mock/client";
import { useConnectionStore } from "@/hooks/use-connection-store";

// --- Backend management ---

let activeBackend: Backend = new HttpBackend(
  () => useConnectionStore.getState().baseUrl(),
);

export function setBackend(backend: Backend): void {
  activeBackend = backend;
}

export function getBackend(): Backend {
  return activeBackend;
}

// --- Helpers ---

function baseUrl(): string {
  return useConnectionStore.getState().baseUrl();
}

// --- IM methods: delegate to active backend ---

export async function health(): Promise<ApiResponse> {
  return activeBackend.health();
}

export async function me(): Promise<ApiResponse> {
  return activeBackend.me();
}

export async function poll(since?: string): Promise<ApiResponse> {
  return activeBackend.poll(since);
}

export async function channels(): Promise<ApiResponse> {
  return activeBackend.channels();
}

export async function send(
  channel: string,
  body: string,
  _author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  return activeBackend.send(channel, body, _author, replyTo);
}

export async function joinChannel(channel: string): Promise<ApiResponse> {
  return activeBackend.joinChannel(channel);
}

export async function read(
  channel: string,
  limit?: number,
): Promise<ApiResponse> {
  return activeBackend.read(channel, limit);
}

export async function thread(
  channel: string,
  line: number,
): Promise<ApiResponse> {
  return activeBackend.thread(channel, line);
}

export async function users(): Promise<ApiResponse> {
  return activeBackend.users();
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
  if (handler === "system") return '"system" is reserved';
  return null;
}

function mapBackendAgent(raw: Record<string, unknown>): Agent {
  return {
    id: (raw.id ?? raw.handler) as string,
    name: (raw.display_name ?? raw.handler) as string,
    status: ((raw.status as string) === "idle"
      ? "offline"
      : raw.status) as Agent["status"],
    systemPrompt: (raw.system_prompt as string) ?? "",
    model: (raw.model as string) ?? undefined,
    env: (raw.env as Record<string, string>) ?? undefined,
    repoPath: (raw.repo_path as string) ?? "",
    messagesProcessed: (raw.messages_processed as number) ?? 0,
    lastActivity: raw.last_activity as string | undefined,
  };
}

// --- Agent API: always HTTP (desktop only) ---

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
      systemPrompt,
      model,
      env,
      repoPath: "",
      messagesProcessed: 0,
    };
    return { ok: true, data: { agent } };
  } catch {
    return mockClient.addAgent(name, systemPrompt);
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
