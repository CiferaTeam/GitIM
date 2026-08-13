import type { UserKind } from "@/lib/types";

export type { UserKind };

export interface RosterUser {
  handler: string;
  displayName?: string;
  kind: UserKind;
}

export interface PartitionInput {
  userInfos: Array<{ handler: string; display_name?: string; kind?: string }>;
  localAgents: Array<{ id: string; handler?: string }>;
  fleetSnapshots: Array<{ agent: { id: string; handler?: string } }>;
  query: string;
  statusFilter: string | null;
}

export interface PartitionResult {
  humans: RosterUser[];
  outside: RosterUser[];
  outsideAgentCount: number;
  outsideUnknownCount: number;
  showHumansAndOutside: boolean;
}

export function normalizeUserKind(raw?: string | null): UserKind {
  if (raw === "human" || raw === "agent") return raw;
  return "unknown";
}

function liveHandlers(input: PartitionInput): Set<string> {
  const set = new Set<string>();
  for (const a of input.localAgents) set.add(a.handler ?? a.id);
  for (const s of input.fleetSnapshots) set.add(s.agent.handler ?? s.agent.id);
  return set;
}

function matchesQuery(u: RosterUser, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    u.handler.toLowerCase().includes(q) ||
    (u.displayName ?? "").toLowerCase().includes(q)
  );
}

export function partitionAgentsRoster(input: PartitionInput): PartitionResult {
  if (input.statusFilter) {
    return {
      humans: [],
      outside: [],
      outsideAgentCount: 0,
      outsideUnknownCount: 0,
      showHumansAndOutside: false,
    };
  }
  const live = liveHandlers(input);
  const humans: RosterUser[] = [];
  const outside: RosterUser[] = [];
  for (const info of input.userInfos) {
    const kind = normalizeUserKind(info.kind);
    const row: RosterUser = {
      handler: info.handler,
      displayName: info.display_name,
      kind,
    };
    if (!matchesQuery(row, input.query)) continue;
    if (kind === "human") {
      humans.push(row);
      continue;
    }
    if (!live.has(info.handler)) outside.push(row);
  }
  humans.sort((a, b) => a.handler.localeCompare(b.handler));
  outside.sort((a, b) => a.handler.localeCompare(b.handler));
  return {
    humans,
    outside,
    outsideAgentCount: outside.filter((o) => o.kind === "agent").length,
    outsideUnknownCount: outside.filter((o) => o.kind === "unknown").length,
    showHumansAndOutside: true,
  };
}

export function formatOutsideSummary(
  agentCount: number,
  unknownCount: number,
): string | null {
  const parts: string[] = [];
  if (agentCount > 0) {
    parts.push(`${agentCount} agent${agentCount === 1 ? "" : "s"}`);
  }
  if (unknownCount > 0) parts.push(`${unknownCount} unclassified`);
  if (parts.length === 0) return null;
  return `Outside · ${parts.join(" · ")}`;
}
