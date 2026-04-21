import { useState } from "react";
import { useAgentStore } from "@/hooks/use-agent-store";
import { AddAgentDialog } from "./add-agent-dialog";
import { AgentCard } from "./agent-card";
import { Bot, Search, SlidersHorizontal } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

export function AgentList() {
  const agents = useAgentStore((s) => s.agents);
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<string | null>(null);

  const filteredAgents = agents.filter((a) => {
    const matchesQuery =
      !query.trim() || a.name.toLowerCase().includes(query.toLowerCase());
    const matchesStatus = !statusFilter || a.status === statusFilter;
    return matchesQuery && matchesStatus;
  });

  const statusOptions = [
    { value: "running", label: "Running" },
    { value: "idle", label: "Idle" },
    { value: "error", label: "Error" },
    { value: "offline", label: "Offline" },
  ];

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
      <div className="flex flex-col sm:flex-row gap-3 mb-6">
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
      </div>

      {/* Grid */}
      {filteredAgents.length > 0 ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {filteredAgents.map((agent) => (
            <AgentCard key={agent.id} agent={agent} />
          ))}
        </div>
      ) : (
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
      )}
    </div>
  );
}
