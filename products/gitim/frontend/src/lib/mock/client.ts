import type { Agent, ApiResponse, Channel, Message, PollChange } from "../types";
import type { ProviderId } from "../providers";
import { nowTimestamp } from "../types";
import {
  mockAgents as initialAgents,
  mockChannels,
  mockMessages as initialMessages,
  mockUsers,
} from "./data";

// --- Mutable in-memory state (cloned from fixtures) ---

const messages: Record<string, Message[]> = Object.fromEntries(
  Object.entries(initialMessages).map(([k, v]) => [k, [...v]])
);

const agents: Agent[] = initialAgents.map((a) => ({ ...a }));

// Channels are mutable so new DMs survive the poll loop.
const channelList: Channel[] = mockChannels.map((c) => ({ ...c }));

let pollCommitCounter = 0;
const changeQueue: PollChange[] = [];

// --- Helpers ---

function delay(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 50));
}

/** "dm:alice,lewis" → "alice--lewis" */
function dmApiToKey(channel: string): string {
  if (channel.startsWith("dm:")) {
    return channel.slice(3).replace(",", "--");
  }
  return channel;
}

function findAgent(id: string): Agent | undefined {
  return agents.find((a) => a.id === id);
}

// --- Exported accessors for timer (Task 4) ---

export function getMockMessages(): Record<string, Message[]> {
  return messages;
}

export function getMockAgents(): Agent[] {
  return agents;
}

export function pushChange(change: PollChange): void {
  changeQueue.push(change);
}

// --- Chat API ---

export async function me(): Promise<ApiResponse> {
  await delay();
  return { ok: true, data: { handler: "lewis", display_name: "Lewis" } };
}

export async function poll(since?: string): Promise<ApiResponse> {
  await delay();
  // `since` is accepted but ignored in mock — we return whatever is queued
  void since;
  const changes = changeQueue.splice(0, changeQueue.length);
  pollCommitCounter += 1;
  return {
    ok: true,
    data: { commit_id: String(pollCommitCounter), changes },
  };
}

export async function channels(): Promise<ApiResponse> {
  await delay();
  return {
    ok: true,
    data: {
      channels: channelList.map(({ name, kind, members }) => ({
        name,
        kind,
        members,
      })),
    },
  };
}

/** Add a channel to the mutable list (e.g. a new DM). No-op if already present. */
export function addChannel(channel: Channel): void {
  if (!channelList.some((c) => c.name === channel.name)) {
    channelList.push({ ...channel });
  }
}

export async function createChannel(
  name: string,
  _displayName?: string,
  _introduction?: string,
): Promise<ApiResponse> {
  await delay();
  void _displayName;
  void _introduction;
  if (!name || !/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(name) || name.length > 32 || name.includes("--")) {
    return { ok: false, error: "invalid channel name" };
  }
  if (channelList.some((c) => c.name === name)) {
    return { ok: false, error: "channel already exists" };
  }
  const channel: Channel = {
    name,
    kind: "channel",
    unreadCount: 0,
    hasMention: false,
    members: ["lewis"],
  };
  addChannel(channel);
  return { ok: true, data: { channel: name, created_by: "lewis" } };
}

export async function users(): Promise<ApiResponse> {
  await delay();
  return { ok: true, data: { users: [...mockUsers] } };
}

export async function read(
  channel: string,
  limit?: number
): Promise<ApiResponse> {
  await delay();
  const key = dmApiToKey(channel);
  const all = messages[key] ?? [];
  const entries = limit !== undefined ? all.slice(-limit) : [...all];
  return { ok: true, data: { entries } };
}

export async function send(
  channel: string,
  body: string,
  author?: string,
  replyTo?: number
): Promise<ApiResponse> {
  await delay();
  const key = dmApiToKey(channel);
  if (!messages[key]) {
    messages[key] = [];
  }
  const existing = messages[key];
  const maxLine = existing.reduce((m, msg) => Math.max(m, msg.line_number), 0);
  const line_number = maxLine + 1;

  const msg: Message = {
    line_number,
    point_to: replyTo ?? 0,
    author: author ?? "lewis",
    timestamp: nowTimestamp(),
    body,
  };

  existing.push(msg);
  pushChange({ channel: key, kind: "channel" });

  return { ok: true, data: { line_number, status: "pushed" } };
}

export async function thread(
  channel: string,
  line: number
): Promise<ApiResponse> {
  await delay();
  const key = dmApiToKey(channel);
  const all = messages[key] ?? [];

  // Trace point_to chain upward to find root
  const byLine = new Map(all.map((m) => [m.line_number, m]));
  let root = line;
  // Walk up until we reach a message with point_to === 0
  const visited = new Set<number>();
  let cursor = line;
  while (true) {
    if (visited.has(cursor)) break;
    visited.add(cursor);
    const msg = byLine.get(cursor);
    if (!msg || msg.point_to === 0) {
      root = cursor;
      break;
    }
    cursor = msg.point_to;
  }

  // BFS from root to collect the full thread tree
  const result: Message[] = [];
  const queue: number[] = [root];
  const seen = new Set<number>();
  while (queue.length > 0) {
    const current = queue.shift()!;
    if (seen.has(current)) continue;
    seen.add(current);
    const msg = byLine.get(current);
    if (msg) {
      result.push(msg);
      // Enqueue children
      for (const m of all) {
        if (m.point_to === current && !seen.has(m.line_number)) {
          queue.push(m.line_number);
        }
      }
    }
  }

  result.sort((a, b) => a.line_number - b.line_number);
  return { ok: true, data: { entries: result } };
}

// --- Agent API ---

export async function listAgents(): Promise<ApiResponse> {
  await delay();
  return { ok: true, data: { agents: agents.map((a) => ({ ...a })) } };
}

export async function getAgent(id: string): Promise<ApiResponse> {
  await delay();
  const agent = findAgent(id);
  if (!agent) return { ok: false, error: `Agent not found: ${id}` };
  return { ok: true, data: { agent: { ...agent } } };
}

export async function addAgent(
  name: string,
  provider: ProviderId,
  systemPrompt: string,
  _llmProvider?: string,
  _llmModel?: string,
): Promise<ApiResponse> {
  await delay();
  const id = name.toLowerCase().replace(/\s+/g, "-");
  const agent: Agent = {
    id,
    name,
    status: "offline",
    provider,
    systemPrompt,
    repoPath: `~/gitim-agents/${id}/`,
    messagesProcessed: 0,
  };
  agents.push(agent);
  return { ok: true, data: { agent: { ...agent } } };
}

export async function removeAgent(id: string): Promise<ApiResponse> {
  await delay();
  const idx = agents.findIndex((a) => a.id === id);
  if (idx === -1) return { ok: false, error: `Agent not found: ${id}` };
  agents.splice(idx, 1);
  return { ok: true, data: { status: "removed" } };
}

export async function startAgent(id: string): Promise<ApiResponse> {
  await delay();
  const agent = findAgent(id);
  if (!agent) return { ok: false, error: `Agent not found: ${id}` };
  agent.status = "running";
  agent.lastActivity = new Date().toISOString();
  return { ok: true, data: { agent: { ...agent } } };
}

export async function stopAgent(id: string): Promise<ApiResponse> {
  await delay();
  const agent = findAgent(id);
  if (!agent) return { ok: false, error: `Agent not found: ${id}` };
  agent.status = "offline";
  return { ok: true, data: { agent: { ...agent } } };
}
