export type AgentStatus = "running" | "idle" | "error" | "offline";

export interface Agent {
  id: string;
  name: string;
  status: AgentStatus;
  systemPrompt: string;
  model?: string;
  env?: Record<string, string>;
  repoPath: string;
  lastActivity?: string; // ISO8601
  messagesProcessed: number;
}

export type MessageStatus = "sending" | "sent" | "synced" | "failed";

export interface Message {
  type?: "message" | "event";
  line_number: number;
  point_to: number; // 0=root, >0=reply target line number
  author: string;
  timestamp: string; // 20260317T120000Z
  body: string;
  event_type?: string;
  _status?: MessageStatus;
  _pendingId?: string;
}

export interface Channel {
  name: string;
  kind: "channel" | "dm";
  unreadCount: number;
  members: string[];
}

export interface UserInfo {
  handler: string;
  display_name: string;
}

export interface AgentActivityEvent {
  agent_id: string;
  event_type: "tool_use" | "thinking" | "done" | "error";
  detail: string;
  timestamp: string; // ISO8601
}

export interface ApiResponse {
  ok: boolean;
  data?: Record<string, unknown>;
  error?: string;
}

export interface PollChange {
  channel: string;
  kind: string;
  entries?: Message[];
}

/** Format timestamp "20260317T120000Z" → "12:00" */
export function formatTimestamp(ts: string): string {
  const match = ts.match(/T(\d{2})(\d{2})\d{2}Z$/);
  if (!match) return "??:??";
  return `${match[1]}:${match[2]}`;
}

/** Current UTC time as a compact timestamp: "20260317T120000Z" */
export function nowTimestamp(): string {
  const d = new Date();
  const pad = (n: number, len = 2) => String(n).padStart(len, "0");
  return (
    `${d.getUTCFullYear()}${pad(d.getUTCMonth() + 1)}${pad(d.getUTCDate())}` +
    `T${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}${pad(d.getUTCSeconds())}Z`
  );
}
