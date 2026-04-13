/**
 * Unified client — agent methods hit the real runtime HTTP API,
 * IM methods re-export from mock (temporary).
 */
import type { Agent, ApiResponse } from "./types";
import * as mockClient from "./mock/client";
import { useConnectionStore } from "@/hooks/use-connection-store";

// --- IM methods: re-export mock (temporary) ---

export const me = mockClient.me;
export const poll = mockClient.poll;
export const channels = mockClient.channels;
export const send = mockClient.send;
export const read = mockClient.read;
export const thread = mockClient.thread;
export const users = mockClient.users;

// --- Helpers ---

function baseUrl(): string {
  return useConnectionStore.getState().baseUrl();
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

function mapBackendAgent(raw: Record<string, unknown>): Agent {
  return {
    id: (raw.id ?? raw.handler) as string,
    name: (raw.display_name ?? raw.handler) as string,
    status: ((raw.status as string) === "idle" ? "offline" : raw.status) as Agent["status"],
    systemPrompt: (raw.system_prompt as string) ?? "",
    repoPath: (raw.repo_path as string) ?? "",
    messagesProcessed: (raw.messages_processed as number) ?? 0,
    lastActivity: raw.last_activity as string | undefined,
    currentChannel: raw.current_channel as string | undefined,
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
  systemPrompt: string,
): Promise<ApiResponse> {
  try {
    const handler = toHandler(name);
    const res = await fetch(`${baseUrl()}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler, display_name: name }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    const agent: Agent = {
      id: data.id ?? handler,
      name,
      status: "offline",
      systemPrompt,
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
