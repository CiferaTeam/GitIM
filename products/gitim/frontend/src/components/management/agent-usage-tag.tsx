import { formatTokens } from "@/lib/format-tokens";
import { usageBucketTokenTotal } from "@/lib/usage-totals";
import type { Agent } from "@/lib/types";

interface AgentUsageTagProps {
  agent: Agent;
}

/** Compact one-line readout that lives on each AgentCard. Renders today's
 *  combined token usage and turn count, or a placeholder when the agent
 *  has no usage data yet (or its provider doesn't report tokens). */
export function AgentUsageTag({ agent }: AgentUsageTagProps) {
  const summary = agent.usageSummary;
  if (!summary) {
    return (
      <span className="text-xs text-text-muted font-mono">— · 0 turns</span>
    );
  }
  const today = summary.today;
  const todayTokens = usageBucketTokenTotal(today, agent.provider);

  if (!summary.providerReportsUsage) {
    return (
      <span className="text-xs text-text-muted font-mono">
        — · {today.turns} turns
      </span>
    );
  }
  if (today.turns === 0) {
    return (
      <span className="text-xs text-text-muted font-mono">— · 0 turns</span>
    );
  }
  return (
    <span className="text-xs text-text-secondary font-mono">
      Today: {formatTokens(todayTokens)} · {today.turns} turns
    </span>
  );
}
