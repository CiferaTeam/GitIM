import type { ProviderId } from "./providers";

export type AgentStatus = "running" | "idle" | "error" | "offline";

export interface SessionUsageSnapshot {
  sessionId: string;
  /**
   * For Claude, Anthropic's `input_tokens` excludes cached content. The
   * backend aggregates input + cache_read + cache_creation before computing
   * `usedPercent`, so the percentage here already reflects the true window
   * occupancy even when this field looks tiny.
   */
  inputTokens?: number;
  outputTokens?: number;
  maxTokens?: number;
  usedPercent: number;
  source: "provider_reported" | "runtime_estimated";
  updatedAt: string;
}

export interface Agent {
  id: string;
  name: string;
  status: AgentStatus;
  provider?: ProviderId;
  systemPrompt: string;
  model?: string;
  /**
   * Human-facing blurb stored in `users/<handler>.meta.yaml::introduction`.
   * Display-only — never fed to the LLM. Capped at 256 bytes server-side
   * (see MAX_INTRODUCTION_LEN in gitim-core).
   */
  introduction?: string;
  env?: Record<string, string>;
  repoPath: string;
  lastActivity?: string; // ISO8601
  messagesProcessed: number;
  errorMessage?: string;
  sessionUsage?: SessionUsageSnapshot;
}

/** Hard ceiling for the introduction blurb. Must stay in sync with
 * `gitim-core::types::MAX_INTRODUCTION_LEN`. */
export const MAX_INTRODUCTION_LEN = 256;

export type MessageStatus = "sending" | "sent" | "synced" | "failed";

export interface Message {
  type?: "message" | "event";
  line_number: number;
  point_to: number; // 0=root, >0=reply target line number
  author: string;
  timestamp: string; // 20260317T120000Z
  body: string;
  event_type?: string;
  /** Event payload. For `event_type: "join"` may contain `{ targets: [...] }`
   * identifying users who were added (vs self-join when omitted). */
  meta?: { targets?: string[] } & Record<string, unknown>;
  _status?: MessageStatus;
  _pendingId?: string;
}

export interface Channel {
  name: string;
  kind: "channel" | "dm";
  unreadCount: number;
  hasMention: boolean;
  members: string[];
}

export interface UserInfo {
  handler: string;
  display_name: string;
}

export interface AgentActivityEvent {
  agent_id: string;
  // "burned" is broadcast by /agents/burn (and the B.4 self-departed
  // self-heal path) once the agent is removed from the workspace. E.3
  // will render it; until then renderers fall through gracefully without
  // mis-labelling it as an error.
  event_type:
    | "tool_use"
    | "thinking"
    | "done"
    | "error"
    | "usage"
    | "burned";
  detail: string;
  timestamp: string; // ISO8601
}

export interface ApiResponse<T = Record<string, unknown>> {
  ok: boolean;
  data?: T;
  error?: string;
  error_code?: string;
}

export type WorkspaceProvider = "local" | "github";

export interface WorkspaceSummary {
  id?: string;
  slug: string;
  workspace_name: string;
  path: string;
  provider: WorkspaceProvider;
  initialized: boolean;
  agents_count?: number;
  browser?: boolean;
  remote_url?: string;
  needs_token?: boolean;
}

export type CreateWorkspaceGit =
  | { provider: "local" }
  | { provider: "github"; remote_url: string; token: string };

export interface CreateWorkspaceRequest {
  path: string;
  workspace_name?: string;
  git: CreateWorkspaceGit;
}

export type PollChangeKind =
  | "channel"
  | "channel_meta"
  | "dm"
  | "board"
  | "card_meta"
  | "card_thread";

export interface PollChange {
  channel: string;
  kind: PollChangeKind;
  entries?: Message[];
}

export type CardStatus = "todo" | "doing" | "done";

export interface Card {
  card_id: string;
  channel: string;
  title: string;
  status: CardStatus;
  labels: string[];
  assignee: string | null;
  created_by: string;
  created_at: string;
  updated_at: string;
}

export interface CardFilter {
  channel?: string | null;
  channels?: string[]; // for multi-select filter bar (client-side)
  labels?: string[];
  status?: CardStatus | null;
  assignee?: string | null;
}

export interface BoardMetaSummary {
  version: number;
  handler: string;
  updated_at: string;
  status: string;
  summary: string;
  tags: string[];
}

export interface BoardSummary {
  handler: string;
  path: string;
  updated_at: string;
  status: string;
  summary: string;
  tags: string[];
}

export interface BoardReadResponse {
  handler: string;
  path: string;
  meta: BoardMetaSummary;
  body: string;
}

export interface BoardWriteResponse {
  handler: string;
  path: string;
  status: "committed";
  commit_id: string;
  sync_status?: "pushed" | "commit_only";
  sync_error?: string;
  error_code?: string;
  needs_token?: boolean;
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
