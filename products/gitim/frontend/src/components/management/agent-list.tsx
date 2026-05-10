import { useState } from "react";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { ArchivedUserEntry } from "@/lib/client";
import { AddAgentDialog } from "./add-agent-dialog";
import { AgentCard } from "./agent-card";
import { ArchivedAgentCard } from "./archived-agent-card";
import { WorkspaceUsageHeader } from "./workspace-usage-header";
import { Archive, Bot, Search, SlidersHorizontal } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

export function AgentList() {
  const agents = useAgentStore((s) => s.agents);
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

  const filteredAgents = agents.filter((a) => {
    const matchesQuery =
      !query.trim() || a.name.toLowerCase().includes(query.toLowerCase());
    const matchesStatus = !statusFilter || a.status === statusFilter;
    return matchesQuery && matchesStatus;
  });

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

      <WorkspaceUsageHeader />

      {/* Active agents grid */}
      {filteredAgents.length > 0 ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {filteredAgents.map((agent) => (
            <AgentCard key={agent.id} agent={agent} />
          ))}
        </div>
      ) : (
        // Only show the empty state when there are no archived rows to
        // render either — otherwise the empty state stomps on a populated
        // archived section below.
        (!showArchived || filteredArchived.length === 0) && (
          <div className="flex flex-col items-center justify-center py-16 text-center">
            <div className="w-14 h-14 rounded-2xl bg-surface flex items-center justify-center mb-4 border border-border">
              <Bot className="size-7 text-primary" />
            </div>
            <p className="text-foreground font-medium">
              {agents.length === 0 ? "No agents yet" : "No agents match your filters"}
            </p>
            <p className="text-sm text-text-muted mt-1 max-w-xs">
              {agents.length === 0
                ? "Get started by adding your first agent to the workspace."
                : "Try adjusting your search or filter criteria."}
            </p>
          </div>
        )
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
