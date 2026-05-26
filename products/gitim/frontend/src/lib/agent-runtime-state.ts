import type { Agent, AgentActivityEvent, AgentStatus, SaturationSummary } from "./types";

export type AgentWorkState = "working" | "idle";
export type AgentPresenceState = "online" | "stopped" | "error";

export interface AgentWorkloadEntry {
  agent: Pick<Agent, "status">;
  latestActivity?: Pick<AgentActivityEvent, "event_type" | "detail">;
}

export interface AgentWorkloadSummary {
  working: number;
  total: number;
}

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

export function summarizeAgentWorkload(
  entries: AgentWorkloadEntry[],
): AgentWorkloadSummary {
  return entries.reduce(
    (summary, entry) => ({
      total: summary.total + 1,
      working:
        summary.working +
        (agentWorkState(entry.agent, entry.latestActivity) === "working" ? 1 : 0),
    }),
    { working: 0, total: 0 },
  );
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

export interface FleetSaturationView {
  today_ratio: number | null;
  today_working: number;
  today_total: number;
  last_7_days_ratios: Array<{ date: string; ratio: number | null }>;
}

export function summarizeFleetSaturation(
  summaries: Array<SaturationSummary | undefined>,
): FleetSaturationView {
  let today_working = 0;
  let today_total = 0;
  const by_date = new Map<string, { working: number; total: number }>();

  for (const s of summaries) {
    if (!s) continue;
    today_working += s.today.working_samples;
    today_total += s.today.total_samples;
    for (const d of s.last_7_days) {
      const cur = by_date.get(d.date) ?? { working: 0, total: 0 };
      cur.working += d.bucket.working_samples;
      cur.total += d.bucket.total_samples;
      by_date.set(d.date, cur);
    }
  }

  const today_ratio = today_total === 0 ? null : today_working / today_total;
  const last_7_days_ratios: Array<{ date: string; ratio: number | null }> = [];
  const dates = Array.from(by_date.keys()).sort();
  for (const date of dates) {
    const { working, total } = by_date.get(date)!;
    last_7_days_ratios.push({
      date,
      ratio: total === 0 ? null : working / total,
    });
  }

  return { today_ratio, today_working, today_total, last_7_days_ratios };
}
