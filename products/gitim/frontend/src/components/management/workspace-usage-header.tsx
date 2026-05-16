import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { workspaceIdentity } from "@/lib/workspace-key";
import { formatTokens } from "@/lib/format-tokens";
import { sparklinePath } from "@/lib/sparkline";
import {
  DEFAULT_UI_STATE,
  readUiState,
  writeUiState,
  type UsageBreakdown,
} from "@/lib/ui-state";
import { aggregateWorkspaceUsage, useWorkspaceUsage } from "@/hooks/use-workspace-usage";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Agent, UsageBucket } from "@/lib/types";

function bucketTotal(b: UsageBucket): number {
  return b.input + b.output + b.cacheRead + b.cacheCreation;
}

interface WorkspaceUsageHeaderProps {
  agents?: Agent[];
  label?: string;
  className?: string;
}

/** Header strip rendered above the agents grid on the management page.
 *  Sums every agent's `usageSummary` client-side and renders the workspace-
 *  level totals + 30-day sparkline + breakdown. The breakdown grouping
 *  dimension (Provider | Handler) is user-controlled and persists per
 *  workspace via `lib/ui-state.ts`. Hides itself when no agent has
 *  produced usage data yet. */
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

  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeWorkspace = activeSlug
    ? workspaces.find((w) => w.slug === activeSlug)
    : undefined;
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;

  const initialBreakdown = workspaceKey
    ? readUiState(workspaceKey).usageBreakdown
    : DEFAULT_UI_STATE.usageBreakdown;
  const [breakdown, setBreakdown] = useState<UsageBreakdown>(initialBreakdown);
  const [persistedKey, setPersistedKey] = useState(workspaceKey);

  // Re-hydrate when the workspace key changes — switching workspaces without
  // remounting this component would otherwise show the previous workspace's
  // preference until the next click. Done as in-render setState rather than
  // useEffect because the value derives from props; see
  // https://react.dev/reference/react/useState#storing-information-from-previous-renders
  if (workspaceKey !== persistedKey) {
    setPersistedKey(workspaceKey);
    setBreakdown(initialBreakdown);
  }

  if (!usage.hasData) return null;

  const totalTokens = bucketTotal(usage.totals);
  const todayTokens = bucketTotal(usage.today);
  const sparklineValues = usage.byDay.map((d) => bucketTotal(d.bucket));

  const entries =
    breakdown === "provider"
      ? usage.byProvider.map((e) => ({ key: e.provider, label: e.provider, bucket: e.bucket }))
      : usage.byHandler.map((e) => ({ key: e.handler, label: e.handler, bucket: e.bucket }));

  function selectBreakdown(next: UsageBreakdown) {
    setBreakdown(next);
    if (workspaceKey) writeUiState(workspaceKey, { usageBreakdown: next });
  }

  return (
    <section
      className={`${className} rounded-lg border border-border-soft bg-card/40 px-4 py-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between`}
    >
      <div className="flex flex-col gap-1 min-w-0">
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
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-mono text-text-muted">
          <div
            role="group"
            aria-label="Usage breakdown grouping"
            className="flex shrink-0 items-center gap-1"
          >
            <BreakdownButton
              active={breakdown === "provider"}
              onClick={() => selectBreakdown("provider")}
            >
              Provider
            </BreakdownButton>
            <BreakdownButton
              active={breakdown === "handler"}
              onClick={() => selectBreakdown("handler")}
            >
              Handler
            </BreakdownButton>
          </div>
          {entries.map(({ key, label: l, bucket }) => (
            <span key={key}>
              {l} {formatTokens(bucketTotal(bucket))}
            </span>
          ))}
        </div>
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

function BreakdownButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Button
      type="button"
      size="xs"
      variant={active ? "default" : "ghost"}
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "tracking-wide uppercase",
        active
          ? "bg-accent-muted text-primary hover:bg-accent-muted hover:text-primary"
          : "text-muted-foreground",
      )}
    >
      {children}
    </Button>
  );
}
