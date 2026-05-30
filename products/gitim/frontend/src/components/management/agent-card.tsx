import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useAgentActivityStore } from "@/hooks/use-agent-activity";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { agentWorkState } from "@/lib/agent-runtime-state";
import * as client from "@/lib/client";
import type { Agent, AgentStatus } from "@/lib/types";
import { useState } from "react";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { AgentUsageTag } from "./agent-usage-tag";
import { BurnAgentDialog } from "./burn-agent-dialog";
import { Play, Pause, Settings, Flame } from "lucide-react";
import { presenceBadge, relativeTime, workBadge } from "./agent-status";
import { agentModelLabel } from "./agent-model-label";

function statusBarColor(status: AgentStatus) {
  switch (status) {
    case "running":
      return "bg-success";
    case "idle":
      return "bg-text-muted";
    case "error":
      return "bg-destructive";
    case "offline":
      return "bg-text-muted";
  }
}

function initials(name: string) {
  return name.slice(0, 2).toUpperCase();
}

function avatarColor(name: string) {
  const hues = [210, 150, 30, 280, 340, 190, 45, 260];
  let hash = 0;
  for (let i = 0; i < name.length; i++)
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  const hue = hues[Math.abs(hash) % hues.length];
  return `hsl(${hue} 70% 55%)`;
}

const INTRODUCTION_PREVIEW_MAX = 72;

function introductionPreview(introduction: string | undefined): string | null {
  const value = introduction?.trim();
  if (!value) return null;
  if (value.length <= INTRODUCTION_PREVIEW_MAX) return value;
  return `${value.slice(0, INTRODUCTION_PREVIEW_MAX).trimEnd()}...`;
}

interface AgentCardProps {
  agent: Agent;
  readOnly?: boolean;
  activityKey?: string;
}

export function AgentCard({
  agent,
  readOnly = false,
  activityKey,
}: AgentCardProps) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const updateAgent = useAgentStore((s) => s.updateAgent);
  const latestActivity = useAgentActivityStore(
    (s) => s.activities[activityKey ?? agent.id]?.[0],
  );
  const navigate = useNavigate();
  const [burnOpen, setBurnOpen] = useState(false);

  const isRunning = agent.status === "running";
  const introPreview = introductionPreview(agent.introduction);
  const workState = agentWorkState(agent, latestActivity);

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
        data-testid="agent-card"
        className={`group relative overflow-visible rounded-md border-border bg-card/60 py-0 shadow-none transition-colors hover:z-20 hover:border-border-strong hover:bg-card focus-within:z-20 ${readOnly ? "" : "cursor-pointer"}`}
        onClick={() => {
          if (!readOnly) navigate(`/management/${agent.id}`);
        }}
      >
        <div
          className={`absolute inset-y-0 left-0 w-1 rounded-l-md ${statusBarColor(agent.status)}`}
        />

        <div
          data-testid="agent-card-summary"
          className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 p-3 pl-4 md:grid-cols-[auto_minmax(0,1fr)_minmax(10rem,16rem)_auto]"
        >
          <div
            className="flex size-8 shrink-0 items-center justify-center rounded-lg text-xs font-bold text-white shadow-sm"
            style={{ backgroundColor: avatarColor(agent.name || agent.id) }}
          >
            {initials(agent.name || agent.id)}
          </div>
          <div className="min-w-0">
            <div className="flex min-w-0 items-center gap-2">
              <span className="truncate text-base font-semibold" title={agent.name}>
                {agent.name}
              </span>
              {agent.name !== agent.handler && (
                <span
                  className="shrink-0 font-mono text-xs font-normal text-text-muted"
                  title={`Handler: @${agent.handler}`}
                >
                  @{agent.handler}
                </span>
              )}
              <span className="flex shrink-0 items-center gap-1 md:hidden">
                {workBadge(workState)}
                {presenceBadge(agent.status)}
              </span>
            </div>
            <span
              className="block truncate text-xs text-text-muted"
              title={`${agent.provider ?? "—"} · ${agentModelLabel(agent)}`}
            >
              {agent.provider ?? "—"} · {agentModelLabel(agent)}
            </span>
          </div>
          {introPreview && (
            <div
              data-testid="agent-card-introduction"
              title={agent.introduction?.trim()}
              className="hidden min-w-0 text-xs leading-5 text-text-secondary md:col-start-3 md:row-start-1 md:row-span-2 md:block"
            >
              <span className="block truncate">{introPreview}</span>
            </div>
          )}
          <div className="hidden shrink-0 md:col-start-4 md:row-start-1 md:block">
            <div className="flex items-center justify-end gap-1">
              {workBadge(workState)}
              {presenceBadge(agent.status)}
            </div>
          </div>

          <div
            className={`col-start-2 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-text-secondary ${
              readOnly ? "col-span-2" : "col-span-1"
            }`}
          >
            <span className="whitespace-nowrap text-text-muted">
              Last {agent.lastActivity ? relativeTime(agent.lastActivity) : "—"}
            </span>
            <span className="whitespace-nowrap text-text-muted">
              Msg {agent.messagesProcessed}
            </span>
            <span className="min-w-0 max-w-full truncate [&>span]:block [&>span]:truncate">
              <AgentUsageTag agent={agent} />
            </span>
          </div>

          {!readOnly && (
            <div
              className="col-start-3 row-start-2 flex items-center justify-end gap-1 md:col-start-4"
              onClick={(e) => e.stopPropagation()}
            >
              <Button
                variant={isRunning ? "outline" : "default"}
                size="icon-xs"
                aria-label={isRunning ? `Stop ${agent.name}` : `Start ${agent.name}`}
                title={isRunning ? "Stop" : "Start"}
                onClick={handleToggle}
                className={
                  isRunning ? "border-border-strong hover:bg-surface-hover" : ""
                }
              >
                {isRunning ? <Pause className="size-3" /> : <Play className="size-3" />}
              </Button>
              <Button
                variant="outline"
                size="icon-xs"
                aria-label={`Details for ${agent.name}`}
                title="Details"
                onClick={() => navigate(`/management/${agent.id}`)}
                className="border-border-strong hover:bg-surface-hover"
              >
                <Settings className="size-3" />
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={`Burn ${agent.name}`}
                title="Burn"
                onClick={() => setBurnOpen(true)}
                className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              >
                <Flame className="size-3" />
              </Button>
            </div>
          )}
        </div>

      </Card>

      {!readOnly && (
        <BurnAgentDialog
          agentId={agent.id}
          agentName={agent.name}
          open={burnOpen}
          onOpenChange={setBurnOpen}
        />
      )}
    </>
  );
}
