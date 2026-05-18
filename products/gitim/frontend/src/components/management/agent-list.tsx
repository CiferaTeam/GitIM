import { useMemo, useState, type ReactNode } from "react";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useFleetStore } from "@/hooks/use-fleet-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { Agent, FleetAgentSnapshot, FleetNodeStatus } from "@/lib/types";
import type { ArchivedUserEntry } from "@/lib/client";
import { AddAgentDialog } from "./add-agent-dialog";
import { AgentCard } from "./agent-card";
import { ArchivedAgentCard } from "./archived-agent-card";
import { WorkspaceUsageHeader } from "./workspace-usage-header";
import {
  Archive,
  Bot,
  ChevronDown,
  Search,
  Server,
  SlidersHorizontal,
} from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";

export function AgentList() {
  const agents = useAgentStore((s) => s.agents);
  const fleetAgents = useFleetStore((s) => s.agents);
  const fleetStatuses = useFleetStore((s) => s.statuses);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<string | null>(null);
  // Dual-source toggle: when on, we fetch archived users from the daemon
  // (via runtime proxy) and render them read-only alongside the active
  // ctx.agents list. We never mix the two arrays — the runtime has rich
  // metadata for active agents; the daemon only knows handlers for
  // archived ones (their clone metadata is gone).
  //
  // Fetch is driven by the toggle click (not useEffect): mirrors the
  // sidebar's archived-DMs pattern so we always get fresh data on
  // expand (out-of-band archives are visible) and avoid an effect-driven
  // setState cascade.
  const [showArchived, setShowArchived] = useState(false);
  const [archived, setArchived] = useState<ArchivedUserEntry[]>([]);
  const [archivedLoading, setArchivedLoading] = useState(false);
  const [archivedError, setArchivedError] = useState<string | null>(null);

  async function handleToggleArchived() {
    const next = !showArchived;
    setShowArchived(next);
    if (!next || !activeSlug) return;
    setArchivedLoading(true);
    setArchivedError(null);
    const res = await client.listArchivedUsers(activeSlug);
    if (res.ok) {
      setArchived(res.data?.users ?? []);
    } else {
      setArchived([]);
      setArchivedError(res.error ?? "Failed to load archived users");
    }
    setArchivedLoading(false);
  }

  const remoteSnapshots = useMemo(
    () => fleetAgents.filter((snapshot) => snapshot.workspaceId === activeSlug),
    [activeSlug, fleetAgents],
  );
  const remoteStatuses = useMemo(
    () => fleetStatuses.filter((status) => status.workspaceId === activeSlug),
    [activeSlug, fleetStatuses],
  );
  const allAgentsForUsage = useMemo(
    () => [...agents, ...remoteSnapshots.map((snapshot) => snapshot.agent)],
    [agents, remoteSnapshots],
  );
  const filteredAgents = agents.filter((agent) =>
    matchesAgent(agent, query, statusFilter),
  );
  const filteredRemoteSnapshots = remoteSnapshots.filter((snapshot) =>
    matchesAgent(snapshot.agent, query, statusFilter, snapshot),
  );
  const visibleRemoteStatuses = statusFilter
    ? []
    : remoteStatuses.filter((status) => matchesNodeStatus(status, query));
  const remoteGroups = groupRemoteAgents(
    filteredRemoteSnapshots,
    remoteStatuses,
    visibleRemoteStatuses,
  );
  const hasFleetUsageContext = remoteSnapshots.length > 0;

  // The status filter is meaningless for archived rows (runtime metadata is
  // gone), so we only filter archived by free-text query against the
  // handler / display_name. When statusFilter is set, hide the archived
  // section entirely — mixing "running" filter with archived rows would
  // be visually confusing.
  const filteredArchived = statusFilter
    ? []
    : archived.filter((a) => {
        if (!query.trim()) return true;
        const q = query.toLowerCase();
        return (
          a.handler.toLowerCase().includes(q) ||
          (a.display_name ?? "").toLowerCase().includes(q)
        );
      });

  const statusOptions = [
    { value: "running", label: "Running" },
    { value: "idle", label: "Idle" },
    { value: "error", label: "Error" },
    { value: "offline", label: "Offline" },
  ];

  function handleUnarchived(handler: string) {
    setArchived((prev) => prev.filter((a) => a.handler !== handler));
  }

  return (
    <div className="p-6 h-full overflow-y-auto">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-6">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Agents</h1>
          <p className="text-sm text-text-muted mt-0.5">
            Manage and monitor your AI agents
          </p>
        </div>
        <AddAgentDialog />
      </div>

      {/* Filters */}
      <div className="flex flex-col sm:flex-row gap-3 mb-6 flex-wrap">
        <div className="relative flex-1 max-w-md">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-text-faint" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search agents..."
            className="pl-9 bg-background border-border"
          />
        </div>
        <div className="flex items-center gap-2">
          <SlidersHorizontal className="size-4 text-text-muted" />
          <div className="flex gap-1.5">
            {statusOptions.map((opt) => {
              const active = statusFilter === opt.value;
              return (
                <Button
                  key={opt.value}
                  variant={active ? "default" : "outline"}
                  size="xs"
                  onClick={() => setStatusFilter(active ? null : opt.value)}
                  className={active ? "" : "border-border-strong text-text-secondary hover:bg-surface-hover"}
                >
                  {opt.label}
                </Button>
              );
            })}
          </div>
        </div>
        <div className="flex items-center gap-2 ml-auto">
          <Button
            variant={showArchived ? "default" : "outline"}
            size="xs"
            onClick={handleToggleArchived}
            className={
              showArchived
                ? ""
                : "border-border-strong text-text-secondary hover:bg-surface-hover"
            }
          >
            <Archive className="size-3.5 mr-1" />
            {showArchived ? "Hide archived" : "Show archived"}
          </Button>
        </div>
      </div>

      <WorkspaceUsageHeader
        agents={allAgentsForUsage}
        label={hasFleetUsageContext ? "Fleet Usage" : "Workspace Usage"}
      />

      {filteredAgents.length > 0 && (
        <AgentNodeSection
          title="Local"
          subtitle="This node"
          agents={filteredAgents}
          showUsage={hasFleetUsageContext}
        >
          {filteredAgents.map((agent) => (
            <AgentCard key={agent.id} agent={agent} />
          ))}
        </AgentNodeSection>
      )}

      {remoteGroups.map((group) => (
        <AgentNodeSection
          key={group.nodeId}
          title={group.title}
          subtitle={group.subtitle}
          agents={group.snapshots.map((snapshot) => snapshot.agent)}
          status={group.status}
          showUsage
        >
          {group.snapshots.length === 0 ? (
            <p className="text-sm text-text-muted">No agents reported.</p>
          ) : (
            group.snapshots.map((snapshot) => (
              <AgentCard
                key={`${snapshot.nodeId}:${snapshot.workspaceId}:${snapshot.agent.id}`}
                agent={snapshot.agent}
                readOnly
              />
            ))
          )}
        </AgentNodeSection>
      ))}

      {filteredAgents.length === 0 &&
        remoteGroups.length === 0 &&
        (!showArchived || filteredArchived.length === 0) && (
          <div className="flex flex-col items-center justify-center py-16 text-center">
            <div className="w-14 h-14 rounded-2xl bg-surface flex items-center justify-center mb-4 border border-border">
              <Bot className="size-7 text-primary" />
            </div>
            <p className="text-foreground font-medium">
              {agents.length === 0 && remoteSnapshots.length === 0
                ? "No agents yet"
                : "No agents match your filters"}
            </p>
            <p className="text-sm text-text-muted mt-1 max-w-xs">
              {agents.length === 0 && remoteSnapshots.length === 0
                ? "Get started by adding your first agent to the workspace."
                : "Try adjusting your search or filter criteria."}
            </p>
          </div>
      )}

      {/* Archived section (dual-source) */}
      {showArchived && (
        <div className="mt-8">
          <div className="flex items-center gap-2 mb-3">
            <Archive className="size-4 text-text-muted" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-text-muted">
              Archived
            </h2>
            {archivedLoading && (
              <span className="text-xs text-text-muted">loading…</span>
            )}
          </div>
          {archivedError ? (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              {archivedError}
            </div>
          ) : filteredArchived.length === 0 && !archivedLoading ? (
            <p className="text-sm text-text-muted">
              {archived.length === 0
                ? "No archived agents."
                : statusFilter
                  ? "Status filter active — clear it to view archived rows."
                  : "No archived agents match your search."}
            </p>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
              {filteredArchived.map((entry) => (
                <ArchivedAgentCard
                  key={entry.handler}
                  entry={entry}
                  onUnarchived={handleUnarchived}
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

interface AgentNodeSectionProps {
  title: string;
  subtitle: string;
  agents: Agent[];
  status?: FleetNodeStatus;
  showUsage?: boolean;
  children: ReactNode;
}

function AgentNodeSection({
  title,
  subtitle,
  agents,
  status,
  showUsage = false,
  children,
}: AgentNodeSectionProps) {
  const [usageExpanded, setUsageExpanded] = useState(false);
  const hasUsageData = agents.some((agent) => agent.usageSummary);
  const canShowUsage = showUsage && hasUsageData;
  const usageToggleLabel = `${usageExpanded ? "Hide" : "Show"} ${title} usage details`;

  return (
    <section className="mb-6">
      <div className="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-border bg-surface">
            <Server className="size-4 text-text-secondary" />
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h2 className="truncate text-base font-semibold">{title}</h2>
              <Badge variant="outline" className="border-border-strong text-text-secondary">
                {agents.length}
              </Badge>
              {status && nodeStatusBadge(status)}
              {canShowUsage && (
                <Button
                  type="button"
                  variant="outline"
                  size="icon-xs"
                  aria-label={usageToggleLabel}
                  aria-expanded={usageExpanded}
                  title={usageToggleLabel}
                  onClick={() => setUsageExpanded((expanded) => !expanded)}
                  className="border-border-strong text-text-secondary hover:bg-surface-hover"
                >
                  <ChevronDown
                    className={`size-3 transition-transform ${
                      usageExpanded ? "rotate-180" : ""
                    }`}
                  />
                </Button>
              )}
            </div>
            <p className="truncate text-xs text-text-muted">{subtitle}</p>
          </div>
        </div>
      </div>
      {canShowUsage && usageExpanded && (
        <WorkspaceUsageHeader
          agents={agents}
          label={`${title} Usage`}
          className="mb-3"
        />
      )}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {children}
      </div>
    </section>
  );
}

interface RemoteAgentGroup {
  nodeId: string;
  title: string;
  subtitle: string;
  snapshots: FleetAgentSnapshot[];
  status?: FleetNodeStatus;
}

function groupRemoteAgents(
  snapshots: FleetAgentSnapshot[],
  statuses: FleetNodeStatus[],
  statusOnlyStatuses: FleetNodeStatus[] = statuses,
): RemoteAgentGroup[] {
  const byNode = new Map<string, FleetAgentSnapshot[]>();
  for (const status of statusOnlyStatuses) {
    if (!byNode.has(status.nodeId)) {
      byNode.set(status.nodeId, []);
    }
  }
  for (const snapshot of snapshots) {
    const arr = byNode.get(snapshot.nodeId) ?? [];
    arr.push(snapshot);
    byNode.set(snapshot.nodeId, arr);
  }

  return Array.from(byNode.entries())
    .map(([nodeId, nodeSnapshots]) => {
      const first = nodeSnapshots[0];
      const status = statuses.find(
        (s) => s.nodeId === nodeId && s.workspaceId === (first?.workspaceId ?? s.workspaceId),
      );
      const title = first?.nodeName ?? status?.nodeName ?? nodeId;
      const subtitle = [
        nodeId,
        first?.nodeIp ?? status?.nodeIp,
        first?.remoteWorkspaceId ?? status?.remoteWorkspaceId,
      ]
        .filter(Boolean)
        .join(" · ");
      return {
        nodeId,
        title,
        subtitle,
        snapshots: [...nodeSnapshots].sort((a, b) =>
          a.agent.name.localeCompare(b.agent.name),
        ),
        status,
      };
    })
    .sort((a, b) => a.title.localeCompare(b.title));
}

function matchesAgent(
  agent: Agent,
  query: string,
  statusFilter: string | null,
  snapshot?: FleetAgentSnapshot,
) {
  const q = query.trim().toLowerCase();
  const matchesQuery =
    q.length === 0 ||
    [
      agent.name,
      agent.id,
      agent.provider,
      agent.model,
      snapshot?.nodeId,
      snapshot?.nodeName,
      snapshot?.nodeIp,
    ]
      .filter(Boolean)
      .some((value) => String(value).toLowerCase().includes(q));
  const matchesStatus = !statusFilter || agent.status === statusFilter;
  return matchesQuery && matchesStatus;
}

function matchesNodeStatus(status: FleetNodeStatus, query: string) {
  const q = query.trim().toLowerCase();
  if (q.length === 0) return true;
  return [status.nodeId, status.nodeName, status.nodeIp, status.remoteWorkspaceId]
    .filter(Boolean)
    .some((value) => String(value).toLowerCase().includes(q));
}

function nodeStatusBadge(status: FleetNodeStatus) {
  switch (status.status) {
    case "connected":
      return (
        <Badge className="bg-success/15 text-success border border-success/30 hover:bg-success/20">
          Connected
        </Badge>
      );
    case "connecting":
      return (
        <Badge className="bg-warning/15 text-warning border border-warning/30 hover:bg-warning/20">
          Connecting
        </Badge>
      );
    case "down":
      return <Badge variant="destructive">Down</Badge>;
  }
}
