import type { PreflightResult, ProviderId } from "./providers";

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

/** Five-counter token bucket scoped to one UTC day or to the agent's
 *  lifetime (when used as `totals`). Mirrors `usage_log::UsageBucket`. */
export interface UsageBucket {
  input: number;
  output: number;
  cacheRead: number;
  cacheCreation: number;
  turns: number;
}

/** One calendar day's bucket as emitted by the runtime's
 *  `last_30_days` zero-filled view. */
export interface UsageDayEntry {
  date: string; // YYYY-MM-DD UTC
  bucket: UsageBucket;
}

/** Cumulative + 30-day breakdown of an agent's token usage.
 *
 *  Mirrors `usage_log::UsageSummary` from the runtime. The runtime serializes
 *  with snake_case keys; the http client converts to camelCase before
 *  putting the value on the agent. `byDay` is always 30 entries (zero-filled
 *  for days the agent had no turns). */
export interface UsageSummary {
  providerReportsUsage: boolean;
  firstSeen: string; // ISO8601 UTC
  lastUpdated: string; // ISO8601 UTC
  totals: UsageBucket;
  /** Convenience field: today's bucket from the agent's own POV (UTC). */
  today: UsageBucket;
  byDay: UsageDayEntry[];
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
  /** Hermes-only: the selected LLM provider id (e.g. "deepseek", "custom:myendpoint"). */
  llmProvider?: string;
  /** Hermes-only: the selected LLM model id (e.g. "deepseek-chat", or a custom model string). */
  llmModel?: string;
  /** Cumulative + 30-day breakdown. Patched in place by the runtime over
   *  the SSE `usage` event after every turn, plus initially populated from
   *  GET /agents on first load. Absent when the agent has never produced a
   *  usage entry (lazy init from the runtime side). */
  usageSummary?: UsageSummary;
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
  /**
   * Ephemeral delivery target list computed by daemon read/poll responses.
   * This is not written into Git.
   */
  recipients?: string[];
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
  workspace_id?: string;
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

export type FleetNodeConnectionStatus = "connecting" | "connected" | "down";

export interface FleetNodeStatus {
  nodeId: string;
  nodeIp?: string;
  nodeName?: string;
  remoteWorkspaceId?: string;
  workspaceIdentity?: string;
  workspaceId: string;
  status: FleetNodeConnectionStatus;
  lastConnectedAt?: string;
  lastEventAt?: string;
  lastError?: string;
  retryCount: number;
}

export interface FleetAgentSnapshot {
  nodeId: string;
  nodeIp?: string;
  nodeName?: string;
  remoteWorkspaceId?: string;
  workspaceIdentity?: string;
  workspaceId: string;
  agent: Agent;
}

export interface FleetAgentActivityEnvelope {
  kind: "agent_activity";
  node_id: string;
  node_ip?: string;
  node_name?: string;
  remote_workspace_id?: string;
  workspace_identity?: string;
  workspace_id: string;
  agent_id: string;
  received_at: string;
  event: AgentActivityEvent;
}

export interface FleetNodeStatusEnvelope {
  kind: "node_status";
  node_id: string;
  node_ip?: string;
  node_name?: string;
  remote_workspace_id?: string;
  workspace_identity?: string;
  workspace_id: string;
  status: FleetNodeConnectionStatus;
  last_connected_at?: string;
  last_event_at?: string;
  last_error?: string;
  retry_count: number;
}

export type FleetEventEnvelope =
  | FleetAgentActivityEnvelope
  | FleetNodeStatusEnvelope;

export interface ApiResponse<T = Record<string, unknown>> {
  ok: boolean;
  data?: T;
  error?: string;
  error_code?: string;
  /**
   * Nested preflight diagnostic from the runtime, present on
   * `POST /agents/add` failures whose `error_code` is preflight-class
   * (`provision_preflight_failed`, `hermes_default_profile_no_llm`,
   * `unknown_provider`, `missing_llm_provider`). Mirrors the on-the-wire
   * field name (snake_case) of the runtime's `ErrorBody.preflight_detail`.
   */
  preflight_detail?: PreflightResult;
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
  | "new_messages"
  | "channel"
  | "channel_meta"
  | "dm_archived"
  | "dm"
  | "board"
  | "card_meta"
  | "card_thread";

export interface PollChange {
  channel: string;
  kind: PollChangeKind;
  entries?: Message[];
}

export interface PollResponse {
  commit_id: string;
  changes: PollChange[];
  reset?: boolean;
  sync_enabled?: boolean;
  needs_token?: boolean;
}

export type CardStatus = "todo" | "doing" | "done";
export type ArchivedVia = "channel" | "manual";

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
  archived_via?: ArchivedVia;
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

// --- Cron --- (mirrors gitim-core::responses and runtime timeline shape)

export interface CronSummary {
  name: string;
  schedule: string;
  /** IANA timezone string, or omitted = UTC. */
  timezone?: string;
  target: string;
  enabled: boolean;
  created_by: string;
  /** RFC 3339 UTC. */
  created_at: string;
  /** RFC 3339 UTC. Absent when schedule fails to parse. */
  next_fire?: string;
}

export interface CronRunEntry {
  /** Filename stem `YYYY-MM-DDTHH-MM-SSZ`. */
  ts: string;
  filename: string;
}

export interface CronDetail {
  name: string;
  /** Raw spec.yaml body (server returns `serde_yaml::Value`, decoded to JSON
   *  on the wire). Treated as an opaque record by the UI. */
  spec: Record<string, unknown>;
  recent_runs: CronRunEntry[];
  next_fire?: string;
}

export type CronTimelineKind = "past" | "future" | "missed";

export interface CronTimelineEntry {
  /** RFC 3339 UTC with seconds + trailing `Z`. */
  ts: string;
  kind: CronTimelineKind;
  cron_name: string;
  /** Handler the cron fires at — the agent that "does" this task. Always
   *  present; the runtime never emits an entry without a target because
   *  CronSpec validation rejects empty target on the spec itself. */
  target: string;
  /** Present only when `kind === "past"` — runtime path to fetch the body. */
  thread_url?: string;
  /** Present only when `kind === "missed"`. */
  reason?: string;
}

export interface CronTimelineResponse {
  entries: CronTimelineEntry[];
  /** True when any single cron's iteration ceiling was hit. */
  truncated?: boolean;
}

export interface CronRunBody {
  body: string;
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

// --- Flow types (mirrors gitim-core::flow::types + responses) ---

export type NodeType =
  | "agent_mention"
  | "channel_thread"
  | "human_review"
  | "wait_for_signal";

export interface FlowNodeSummary {
  id: string;
  type: NodeType;
  owner?: string;
  participants?: string[];
  needs?: string[];
  prompt: string;
}

export interface FlowSummary {
  slug: string;
  name: string;
  description: string;
  node_count: number;
  updated_at?: string;
}

export interface FlowDocument {
  slug: string;
  name: string;
  description: string;
  created_by: string;
  created_at: string;
  updated_at?: string;
  nodes: FlowNodeSummary[];
  raw_markdown: string;
}

export interface FlowValidationItem {
  kind: "error" | "warning";
  message: string;
}

export interface FlowValidationResult {
  slug: string;
  ok: boolean;
  items: FlowValidationItem[];
}

// --- Flow run types (mirrors runtime flow_run responses) ---

export type RunStatus = "in_progress" | "done" | "failed" | "cancelled";
export type NodeStatus = "pending" | "in_progress" | "done" | "failed" | "skipped";

export interface FlowRunSummary {
  run_id: string;
  flow_slug: string;
  channel: string;
  status: RunStatus;
  started_by: string;
  started_at: string;
  updated_at: string;
  node_count: number;
  nodes_done: number;
}

export interface FlowRunNodeSummary {
  id: string;
  status: NodeStatus;
  actor?: string;
  started_at?: string;
  completed_at?: string;
  result_ref?: string;
}

export interface FlowRunDetail {
  run_id: string;
  flow_slug: string;
  channel: string;
  started_at: string;
  started_by: string;
  status: RunStatus;
  updated_at: string;
  nodes: FlowRunNodeSummary[];
}
