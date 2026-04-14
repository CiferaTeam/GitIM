import { useState } from "react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useAgentActivityStore } from "../../hooks/use-agent-activity";
import type { AgentActivityEvent } from "../../lib/types";

const EMPTY_ACTIVITIES: AgentActivityEvent[] = [];

function StatusDot({ status }: { status: string }) {
  const color =
    status === "running"
      ? "bg-success"
      : status === "error"
        ? "bg-error"
        : "bg-text-muted";

  return (
    <span
      className={`inline-block w-1.5 h-1.5 rounded-full shrink-0 ${color}`}
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
    <div className="flex items-baseline gap-1.5 py-0.5 text-[10px] leading-tight text-text-muted">
      <span className="text-text-faint shrink-0 font-mono">{time}</span>
      <span className="truncate font-mono">
        {typeLabel}
        {event.detail}
      </span>
    </div>
  );
}

function AgentRow({ agentId, name }: { agentId: string; name: string }) {
  const [expanded, setExpanded] = useState(false);
  const activities = useAgentActivityStore(
    (s) => s.activities[agentId] ?? EMPTY_ACTIVITIES,
  );
  const status =
    useAgentStore((s) => s.agents.find((a) => a.id === agentId)?.status) ??
    "offline";
  const latest = activities[0];

  return (
    <div
      className="group relative"
      onMouseEnter={() => setExpanded(true)}
      onMouseLeave={() => setExpanded(false)}
    >
      {/* Compact row */}
      <div className="flex items-center gap-1.5 px-2 py-1 min-w-0">
        <StatusDot status={status} />
        <span className="text-[11px] font-mono text-text-secondary shrink-0">
          {name}
        </span>
        {latest ? (
          <span className="text-[10px] font-mono text-text-muted truncate">
            {latest.detail}
          </span>
        ) : (
          <span className="text-[10px] text-text-faint italic">idle</span>
        )}
      </div>

      {/* Expanded popover on hover — show recent activities */}
      {expanded && activities.length > 0 && (
        <div className="absolute left-0 top-full z-50 w-72 max-h-52 overflow-y-auto rounded-md border border-border bg-popover shadow-lg p-2">
          <p className="text-[10px] font-semibold uppercase text-text-muted tracking-widest mb-1">
            {name} — Recent Activity
          </p>
          {activities.map((evt, i) => (
            <ActivityLine key={`${evt.timestamp}-${i}`} event={evt} />
          ))}
        </div>
      )}
    </div>
  );
}

export function AgentStatusPanel() {
  const agents = useAgentStore((s) => s.agents);

  if (agents.length === 0) return null;

  return (
    <div className="px-3 pt-3 pb-1">
      <p className="text-[10px] font-semibold uppercase text-muted-foreground tracking-widest mb-1 px-2">
        Agents
      </p>
      <div className="space-y-0">
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
