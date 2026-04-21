import { useAgentStore } from "../../hooks/use-agent-store";
import { useAgentActivityStore } from "../../hooks/use-agent-activity";
import type { AgentActivityEvent } from "../../lib/types";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "../ui/hover-card";

const EMPTY_ACTIVITIES: AgentActivityEvent[] = [];

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
  const typeLabel =
    event.event_type === "tool_use"
      ? ""
      : event.event_type === "thinking"
        ? ""
        : event.event_type === "done"
          ? ""
          : "err ";

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

function UsageBadge({ agentId }: { agentId: string }) {
  const usage = useAgentStore(
    (s) => s.agents.find((a) => a.id === agentId)?.sessionUsage,
  );
  if (!usage) return null;

  const warning = usage.usedPercent >= 80;
  const pctColor = warning ? "text-warning" : "text-text-faint";
  // One decimal so sub-1% sessions don't flatten to "0%" in the UI, but keep
  // it short enough to fit on its own line. ≥10% displays as integer for
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

function AgentRow({ agentId, name }: { agentId: string; name: string }) {
  const activities = useAgentActivityStore(
    (s) => s.activities[agentId] ?? EMPTY_ACTIVITIES,
  );
  const status =
    useAgentStore((s) => s.agents.find((a) => a.id === agentId)?.status) ??
    "offline";
  const errorMessage = useAgentStore(
    (s) => s.agents.find((a) => a.id === agentId)?.errorMessage,
  );
  const latest = activities[0];
  const showError = status === "error";
  const hasDetail = activities.length > 0;

  const row = (
    <div className="relative rounded-md border border-border/60 bg-background/40">
      <div
        className={`flex items-center gap-2 px-2.5 py-1.5 min-w-0 select-none rounded-md transition-colors ${
          hasDetail ? "cursor-default hover:bg-surface-hover" : ""
        }`}
      >
        <StatusDot status={status} />
        <span className="text-xs font-medium text-text-secondary shrink-0">
          {name}
        </span>
        {showError ? (
          <span className="text-[11px] font-mono text-error truncate whitespace-pre-line">
            {errorMessage ?? "unknown error"}
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
          <UsageBadge agentId={agentId} />
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

  if (agents.length === 0) return null;

  return (
    <div className="px-3 pt-3 pb-1">
      <p className="text-xs font-semibold uppercase text-text-secondary tracking-wider mb-2 px-2">
        Agents
      </p>
      <div className="space-y-1.5">
        {agents.map((agent) => (
          <AgentRow
            key={agent.id}
            agentId={agent.id}
            name={agent.name || agent.id}
          />
        ))}
      </div>
    </div>
  );
}
