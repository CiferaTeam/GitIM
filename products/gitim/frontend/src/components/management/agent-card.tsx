import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from "@/components/ui/card";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { Agent, AgentStatus } from "@/lib/types";
import { useState } from "react";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { AgentUsageTag } from "./agent-usage-tag";
import { BurnAgentDialog } from "./burn-agent-dialog";
import { Play, Pause, Settings, Flame } from "lucide-react";
import { relativeTime, statusBadge } from "./agent-status";

function statusBarColor(status: AgentStatus) {
  switch (status) {
    case "running": return "bg-success";
    case "idle": return "bg-warning";
    case "error": return "bg-destructive";
    case "offline": return "bg-text-muted";
  }
}

function initials(name: string) {
  return name.slice(0, 2).toUpperCase();
}

function avatarColor(name: string) {
  const hues = [210, 150, 30, 280, 340, 190, 45, 260];
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = name.charCodeAt(i) + ((hash << 5) - hash);
  const hue = hues[Math.abs(hash) % hues.length];
  return `hsl(${hue} 70% 55%)`;
}

interface AgentCardProps {
  agent: Agent;
}

export function AgentCard({ agent }: AgentCardProps) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const updateAgent = useAgentStore((s) => s.updateAgent);
  const navigate = useNavigate();
  const [burnOpen, setBurnOpen] = useState(false);

  const isRunning = agent.status !== "offline";

  async function handleToggle() {
    if (!activeSlug) return;
    if (isRunning) {
      const res = await client.stopAgent(activeSlug, agent.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to stop agent");
      }
    } else {
      const res = await client.startAgent(activeSlug, agent.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to start agent");
      }
    }
  }

  return (
    <>
      <Card
        className="relative overflow-hidden cursor-pointer hover:shadow-lg hover:shadow-[var(--color-shadow)] transition-all duration-200 hover:border-border-strong bg-card/60 group"
        onClick={() => navigate(`/management/${agent.id}`)}
      >
        {/* Status bar */}
        <div className={`absolute top-0 left-0 right-0 h-1 ${statusBarColor(agent.status)}`} />

        <CardHeader className="pb-2 pt-5">
          <div className="flex items-center justify-between gap-3">
            <div className="flex items-center gap-3 min-w-0">
              <div
                className="w-10 h-10 rounded-xl flex items-center justify-center text-sm font-bold text-white shadow-sm shrink-0"
                style={{ backgroundColor: avatarColor(agent.name || agent.id) }}
              >
                {initials(agent.name || agent.id)}
              </div>
              <div className="min-w-0">
                <span className="font-semibold text-lg truncate block">{agent.name}</span>
                <span className="text-xs text-text-muted truncate block">
                  {agent.provider ?? "—"} ·{" "}
                  {agent.model ??
                    (agent.provider === "opencode" || agent.provider === "pi" || agent.provider === "hermes"
                      ? "default"
                      : "—")}
                </span>
                {agent.status === "error" && (
                  <p className="text-xs text-destructive truncate">
                    {agent.errorMessage ?? "unknown error"}
                  </p>
                )}
              </div>
            </div>
            {statusBadge(agent.status)}
          </div>
        </CardHeader>

        <CardContent>
          {agent.introduction && (
            <p className="text-sm text-text-secondary mb-3 whitespace-pre-wrap break-words">
              {agent.introduction}
            </p>
          )}
          <div className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
            <span className="text-text-muted">Last activity</span>
            <span>
              {agent.lastActivity ? relativeTime(agent.lastActivity) : "—"}
            </span>

            <span className="text-muted-foreground">Messages processed</span>
            <span>{agent.messagesProcessed}</span>

            <span className="text-muted-foreground">Token usage</span>
            <AgentUsageTag agent={agent} />
          </div>
        </CardContent>

        <CardFooter className="gap-2 flex-wrap" onClick={(e) => e.stopPropagation()}>
          <Button
            variant={isRunning ? "outline" : "default"}
            size="sm"
            onClick={handleToggle}
            className={isRunning ? "border-border-strong hover:bg-surface-hover" : ""}
          >
            {isRunning ? (
              <><Pause className="size-3.5 mr-1" /> Stop</>
            ) : (
              <><Play className="size-3.5 mr-1" /> Start</>
            )}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => navigate(`/management/${agent.id}`)}
            className="border-border-strong hover:bg-surface-hover"
          >
            <Settings className="size-3.5 mr-1" />
            Details
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setBurnOpen(true)}
            className="text-destructive hover:text-destructive hover:bg-destructive/10"
          >
            <Flame className="size-3.5 mr-1" />
            Burn
          </Button>
        </CardFooter>
      </Card>

      <BurnAgentDialog
        agentId={agent.id}
        agentName={agent.name}
        open={burnOpen}
        onOpenChange={setBurnOpen}
      />
    </>
  );
}
