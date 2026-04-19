import { useState } from "react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useAgentActivityStore } from "../../hooks/use-agent-activity";
import type { AgentActivityEvent } from "../../lib/types";

const EMPTY_ACTIVITIES: AgentActivityEvent[] = [];

function StatusDot({ status }: { status: string }) {
  const color =
    status === "running"
      ? "bg-success shadow-[0_0_4px_var(--color-glow-success-soft)]"
      : status === "error"
        ? "bg-error shadow-[0_0_4px_rgba(248,113,113,0.4)]"
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

function AgentRow({ agentId, name }: { agentId: string; name: string }) {
  const [expanded, setExpanded] = useState(false);
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

  return (
    <div
      className="relative rounded-md border border-border/60 bg-background/40"
      onMouseLeave={() => setExpanded(false)}
    >
      <div
        className="flex items-center gap-2 px-2.5 py-1.5 min-w-0 cursor-pointer select-none hover:bg-surface-hover rounded-md transition-colors"
        onClick={() => setExpanded((v) => !v)}
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

      {expanded && activities.length > 0 && (
        <div className="absolute left-0 top-full z-50 w-72 max-h-52 overflow-y-auto rounded-md border border-border bg-popover shadow-xl p-2 mt-1">
          <p className="text-[11px] font-semibold uppercase text-text-muted tracking-wider mb-1">
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
