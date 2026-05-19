import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { formatTokens } from "@/lib/format-tokens";
import { sparklinePath } from "@/lib/sparkline";
import { usageBucketTokenTotal } from "@/lib/usage-totals";
import type { Agent } from "@/lib/types";

interface AgentUsageCardProps {
  agent: Agent;
}

/** Detail-page block: cumulative + today + 30-day sparkline + 4-field hover
 *  breakdown. Renders nothing when the agent has no usage data; renders a
 *  graceful "this provider does not report tokens" line when the provider
 *  flagged itself as non-reporting. */
export function AgentUsageCard({ agent }: AgentUsageCardProps) {
  const summary = agent.usageSummary;
  if (!summary) return null;

  const totals = summary.totals;
  const today = summary.today;
  const totalTokens = usageBucketTokenTotal(totals, agent.provider);
  const todayTokens = usageBucketTokenTotal(today, agent.provider);

  const sparklineValues = summary.byDay.map((d) =>
    usageBucketTokenTotal(d.bucket, agent.provider),
  );
  const peak = Math.max(0, ...sparklineValues);
  const startDate = summary.firstSeen ? summary.firstSeen.slice(0, 10) : "—";

  if (!summary.providerReportsUsage) {
    return (
      <section className="rounded-lg border border-border-soft bg-card/40 p-4 space-y-1">
        <header className="text-sm font-semibold text-foreground">
          Token Usage
        </header>
        <p className="text-sm text-text-muted">
          该 provider 不上报 token · {totals.turns} turns since {startDate}
        </p>
      </section>
    );
  }

  return (
    <section className="rounded-lg border border-border-soft bg-card/40 p-4 space-y-3">
      <header className="text-sm font-semibold text-foreground">Token Usage</header>

      <HoverCard>
        <HoverCardTrigger asChild>
          <div className="flex items-baseline gap-3 cursor-help">
            <span className="text-2xl font-mono text-foreground">
              {formatTokens(totalTokens)}
            </span>
            <span className="text-sm text-text-secondary">
              累计 · 今日 {formatTokens(todayTokens)} · 今日 {today.turns} turns
            </span>
          </div>
        </HoverCardTrigger>
        <HoverCardContent className="w-72">
          <div className="space-y-2">
            <div className="text-sm font-medium">累计明细</div>
            <dl className="grid grid-cols-2 gap-x-3 gap-y-1 text-xs font-mono">
              <dt className="text-text-muted">input</dt>
              <dd className="text-right">{totals.input.toLocaleString()}</dd>
              <dt className="text-text-muted">output</dt>
              <dd className="text-right">{totals.output.toLocaleString()}</dd>
              <dt className="text-text-muted">cache_read</dt>
              <dd className="text-right">{totals.cacheRead.toLocaleString()}</dd>
              <dt className="text-text-muted">cache_creation</dt>
              <dd className="text-right">{totals.cacheCreation.toLocaleString()}</dd>
              <dt className="text-text-muted">turns</dt>
              <dd className="text-right">{totals.turns.toLocaleString()}</dd>
            </dl>
            <div className="text-xs text-text-muted pt-1">统计自 {startDate} 起</div>
          </div>
        </HoverCardContent>
      </HoverCard>

      {sparklineValues.length > 0 && (
        <div className="text-primary">
          <svg
            width={240}
            height={40}
            viewBox="0 0 240 40"
            aria-label="近 30 天 token 用量"
            className="overflow-visible"
          >
            <path
              d={sparklinePath(sparklineValues, 240, 40)}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </div>
      )}

      <div className="text-xs text-text-muted">
        近 30 天 · 峰值 {formatTokens(peak)} 当日
      </div>
    </section>
  );
}
