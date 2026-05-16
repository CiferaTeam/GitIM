import { formatTokens } from "@/lib/format-tokens";
import { sparklinePath } from "@/lib/sparkline";
import { aggregateWorkspaceUsage, useWorkspaceUsage } from "@/hooks/use-workspace-usage";
import type { Agent, UsageBucket } from "@/lib/types";
import { useMemo } from "react";

function bucketTotal(b: UsageBucket): number {
  return b.input + b.output + b.cacheRead + b.cacheCreation;
}

/** Header strip rendered above the agents grid on the management page.
 *  Sums every agent's `usageSummary` client-side and renders the workspace-
 *  level totals + 30-day sparkline + per-provider breakdown. Hides itself
 *  when no agent has produced usage data yet. */
interface WorkspaceUsageHeaderProps {
  agents?: Agent[];
  label?: string;
  className?: string;
}

export function WorkspaceUsageHeader({
  agents,
  label = "Workspace Usage",
  className = "mb-4",
}: WorkspaceUsageHeaderProps) {
  const storeUsage = useWorkspaceUsage();
  const propUsage = useMemo(
    () => (agents ? aggregateWorkspaceUsage(agents) : null),
    [agents],
  );
  const usage = propUsage ?? storeUsage;
  if (!usage.hasData) return null;

  const totalTokens = bucketTotal(usage.totals);
  const todayTokens = bucketTotal(usage.today);
  const sparklineValues = usage.byDay.map((d) => bucketTotal(d.bucket));

  return (
    <section className={`${className} rounded-lg border border-border-soft bg-card/40 px-4 py-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between`}>
      <div className="flex flex-col gap-1">
        <div className="text-xs uppercase tracking-wide text-text-muted">
          {label}
        </div>
        <div className="flex items-baseline gap-2">
          <span className="text-xl font-mono text-foreground">
            {formatTokens(totalTokens)}
          </span>
          <span className="text-sm text-text-secondary">
            累计 · 今日 {formatTokens(todayTokens)} · 今日 {usage.today.turns} turns
          </span>
        </div>
        {usage.byProvider.length > 0 && (
          <div className="text-xs text-text-muted font-mono flex flex-wrap gap-x-3 gap-y-1">
            {usage.byProvider.map(({ provider, bucket }) => (
              <span key={provider}>
                {provider} {formatTokens(bucketTotal(bucket))}
              </span>
            ))}
          </div>
        )}
      </div>
      {sparklineValues.length > 0 && (
        <div className="text-primary shrink-0">
          <svg
            width={180}
            height={36}
            viewBox="0 0 180 36"
            aria-label="近 30 天 workspace token 用量"
            className="overflow-visible"
          >
            <path
              d={sparklinePath(sparklineValues, 180, 36)}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </div>
      )}
    </section>
  );
}
