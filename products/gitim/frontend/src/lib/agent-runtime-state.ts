import type { Agent, AgentActivityEvent, AgentStatus } from "./types";

export type AgentWorkState = "working" | "idle";
export type AgentPresenceState = "online" | "stopped" | "error";

const IDLE_ACTIVITY_TYPES = new Set(["done", "error", "burned", "steered"]);

function detailLooksDone(detail: string): boolean {
  const normalized = detail.trim().toLowerCase();
  return normalized === "done" || normalized.startsWith("done ");
}

export function agentWorkState(
  agent: Pick<Agent, "status">,
  latestActivity?: Pick<AgentActivityEvent, "event_type" | "detail">,
): AgentWorkState {
  if (agent.status !== "running" || !latestActivity) return "idle";
  if (IDLE_ACTIVITY_TYPES.has(latestActivity.event_type)) return "idle";
  if (detailLooksDone(latestActivity.detail)) return "idle";
  return "working";
}

export function agentPresenceState(status: AgentStatus): AgentPresenceState {
  if (status === "error") return "error";
  return status === "running" ? "online" : "stopped";
}

export function presenceMatchesFilter(
  status: AgentStatus,
  filter: string | null,
): boolean {
  if (!filter) return true;
  if (filter === "online") return agentPresenceState(status) === "online";
  if (filter === "stopped") return agentPresenceState(status) === "stopped";
  return status === filter;
}
