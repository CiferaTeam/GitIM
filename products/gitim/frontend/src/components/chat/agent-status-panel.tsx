import { List } from "lucide-react";
import type { CSSProperties } from "react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useAgentActivityStore } from "../../hooks/use-agent-activity";
import type { Agent, AgentActivityEvent } from "../../lib/types";
import { Button } from "../ui/button";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "../ui/hover-card";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../ui/popover";

const EMPTY_ACTIVITIES: AgentActivityEvent[] = [];
const PREVIEW_AGENT_LIMIT = 3;
const STATUS_RANK: Record<string, number> = {
  running: 3,
  error: 2,
  idle: 1,
  offline: 0,
};

function latestActivity(
  agentId: string,
  activities: Record<string, AgentActivityEvent[]>,
): AgentActivityEvent | undefined {
  return activities[agentId]?.[0];
}

function activityTime(event: AgentActivityEvent | undefined): number {
  if (!event) return 0;
  const ts = Date.parse(event.timestamp);
  return Number.isFinite(ts) ? ts : 0;
}

function agentLabel(agent: Agent): string {
  return agent.name || agent.id;
}

function clampUsagePercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(100, value));
}

function usageFillValue(agent: Agent): string | null {
  const usage = agent.sessionUsage;
  if (!usage) return null;

  const percent = clampUsagePercent(usage.usedPercent);
  return `${Number.isInteger(percent) ? percent.toFixed(0) : percent.toFixed(1)}%`;
}

function compareAgentsByActivity(
  activities: Record<string, AgentActivityEvent[]>,
): (a: Agent, b: Agent) => number {
  return (a, b) => {
    const byActivity =
      activityTime(latestActivity(b.id, activities)) -
      activityTime(latestActivity(a.id, activities));
    if (byActivity !== 0) return byActivity;

    const byStatus =
      (STATUS_RANK[b.status] ?? 0) - (STATUS_RANK[a.status] ?? 0);
    if (byStatus !== 0) return byStatus;

    return agentLabel(a).localeCompare(agentLabel(b));
  };
}

function StatusDot({ status }: { status: string }) {
  const color =
    status === "running"
      ? "bg-success shadow-[0_0_4px_var(--color-glow-success-soft)]"
      : status === "error"
        ? "bg-error shadow-[0_0_4px_var(--color-glow-error)]"
        : "bg-text-muted";

  return (
    <span
      className={`inline-block w-2 h-2 rounded-full shrink-0 ${color}`}
    />
  );
}

function ActivityLine({ event }: { event: AgentActivityEvent }) {
  // TODO(E.3): replace this string with proper rendering — the "burned"
  // case (broadcast by /agents/burn and the self-departed self-heal
  // path) deserves its own visual treatment. For now we narrow the
  // "err " label strictly to the "error" event_type so unknown types
  // ("burned", "usage", etc.) render label-less rather than as fake
  // errors.
  const typeLabel = event.event_type === "error" ? "err " : "";

  const time = event.timestamp.slice(11, 19);

  return (
    <div className="flex items-baseline gap-1.5 py-0.5 text-[11px] leading-tight text-text-muted">
      <span className="text-text-faint shrink-0 font-mono">{time}</span>
      <span className="truncate font-mono">
        {typeLabel}
        {event.detail}
      </span>
    </div>
  );
}

function UsageBadge({ agent }: { agent: Agent }) {
  const usage = agent.sessionUsage;
  if (!usage) return null;

  const warning = usage.usedPercent >= 80;
  const pctColor = warning ? "text-warning" : "text-text-faint";
  // One decimal so sub-1% sessions don't flatten to "0%" in the UI, but keep
  // it short enough to fit on its own line. >=10% displays as integer for
  // density.
  const pctText =
    usage.usedPercent >= 10
      ? usage.usedPercent.toFixed(0)
      : usage.usedPercent.toFixed(1);

  return (
    <div className="text-[10px] font-mono flex items-baseline gap-1.5 min-w-0">
      <span className={`${pctColor} shrink-0`}>{pctText}%</span>
      {usage.sessionId && (
        <span className="text-text-faint truncate" title={usage.sessionId}>
          sid:{usage.sessionId}
        </span>
      )}
    </div>
  );
}

function AgentRow({
  agent,
  activities,
  testId,
}: {
  agent: Agent;
  activities: AgentActivityEvent[];
  testId?: string;
}) {
  const name = agentLabel(agent);
  const latest = activities[0];
  const showError = agent.status === "error";
  const hasDetail = activities.length > 0;
  const usageFill = usageFillValue(agent);
  const usageTone =
    (agent.sessionUsage?.usedPercent ?? 0) >= 80 ? "warning" : "normal";

  const row = (
    <div
      className="relative overflow-hidden rounded-md border border-border/60 bg-background/40 animate-[agent-row-enter_180ms_ease-out]"
      data-testid={testId}
    >
      {usageFill && (
        <div
          className="agent-usage-liquid"
          data-testid="agent-usage-liquid"
          data-tone={usageTone}
          aria-hidden="true"
          style={
            { "--agent-usage-fill": usageFill } as CSSProperties
          }
        />
      )}
      <div
        className={`relative z-10 flex items-center gap-2 px-2.5 py-1.5 min-w-0 select-none rounded-md transition-colors ${
          hasDetail ? "cursor-default hover:bg-surface-hover/60" : ""
        }`}
      >
        <StatusDot status={agent.status} />
        <span className="text-xs font-medium text-text-secondary shrink-0">
          {name}
        </span>
        {showError ? (
          <span className="text-[11px] font-mono text-error truncate">
            {agent.errorMessage ?? "unknown error"}
          </span>
        ) : latest ? (
          <span className="text-[11px] font-mono text-text-muted truncate">
            {latest.detail}
          </span>
        ) : (
          <span className="text-[11px] text-text-faint italic">idle</span>
        )}
      </div>
    </div>
  );

  if (!hasDetail) return row;

  return (
    <HoverCard>
      <HoverCardTrigger asChild>{row}</HoverCardTrigger>
      <HoverCardContent
        side="bottom"
        align="start"
        sideOffset={4}
        className="w-max max-w-[32rem] min-w-72 max-h-52 overflow-y-auto p-2 shadow-xl"
      >
        <div className="mb-1.5">
          <p className="text-[11px] font-semibold uppercase text-text-muted tracking-wider truncate">
            {name} — Recent Activity
          </p>
          <UsageBadge agent={agent} />
        </div>
        {activities.map((evt, i) => (
          <ActivityLine key={`${evt.timestamp}-${i}`} event={evt} />
        ))}
      </HoverCardContent>
    </HoverCard>
  );
}

export function AgentStatusPanel() {
  const agents = useAgentStore((s) => s.agents);
  const activities = useAgentActivityStore((s) => s.activities);

  if (agents.length === 0) return null;

  const sortedAgents = [...agents].sort(compareAgentsByActivity(activities));
  const previewAgents = sortedAgents.slice(0, PREVIEW_AGENT_LIMIT);
  const hiddenCount = Math.max(0, sortedAgents.length - previewAgents.length);

  return (
    <div className="px-3 pt-3 pb-2 shrink-0">
      <div className="flex items-center justify-between mb-2 px-2">
        <p className="text-xs font-semibold uppercase text-text-secondary tracking-wider">
          Agents
        </p>
        {hiddenCount > 0 && (
          <Popover>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                size="xs"
                aria-label="Show all agents"
                title="Show all agents"
                className="h-6 gap-1 px-1.5 text-[11px] text-text-muted hover:text-foreground"
              >
                <List className="size-3.5" />
                <span className="font-mono">+{hiddenCount}</span>
              </Button>
            </PopoverTrigger>
            <PopoverContent
              side="right"
              align="start"
              sideOffset={8}
              className="w-80 max-h-[min(70vh,32rem)] p-3"
            >
              <div className="flex items-baseline justify-between gap-3 px-1 pb-2">
                <p className="text-xs font-semibold uppercase text-text-secondary tracking-wider">
                  Agents
                </p>
                <span className="text-[11px] font-mono text-text-faint">
                  {sortedAgents.length}
                </span>
              </div>
              <div className="max-h-[calc(min(70vh,32rem)-3rem)] overflow-y-auto space-y-1.5 pr-1">
                {sortedAgents.map((agent) => (
                  <AgentRow
                    key={`${agent.id}-${latestActivity(agent.id, activities)?.timestamp ?? "idle"}`}
                    agent={agent}
                    activities={activities[agent.id] ?? EMPTY_ACTIVITIES}
                    testId="agent-full-row"
                  />
                ))}
              </div>
            </PopoverContent>
          </Popover>
        )}
      </div>
      <div className="space-y-1.5">
        {previewAgents.map((agent) => (
          <AgentRow
            key={`${agent.id}-${latestActivity(agent.id, activities)?.timestamp ?? "idle"}`}
            agent={agent}
            activities={activities[agent.id] ?? EMPTY_ACTIVITIES}
            testId="agent-preview-row"
          />
        ))}
      </div>
    </div>
  );
}
