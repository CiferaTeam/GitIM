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
import {
  aggregateWorkspaceUsage,
  bucketTokenTotal,
  useWorkspaceUsage,
  type UsageBreakdownEntry,
} from "@/hooks/use-workspace-usage";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Agent, UsageBucket } from "@/lib/types";

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
 *  produced usage data yet.
 *
 *  Fleet-mode caveat: when this component renders multiple times on the same
 *  screen (one per remote node — see `agent-list.tsx`), clicking the toggle
 *  on one instance writes the new value to the shared workspace persistence
 *  but does NOT push it to sibling instances. Siblings re-hydrate on their
 *  next re-render. Same-tab cross-instance live sync is intentionally
 *  out-of-scope for v1 (see `docs/plans/workspace-usage-breakdown-toggle/`). */
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

  const totalTokens = bucketTokenTotal(usage.totals);
  const todayTokens = bucketTokenTotal(usage.today);
  const sparklineValues = usage.byDay.map((d) => bucketTokenTotal(d.bucket));

  const entries: UsageEntry[] =
    breakdown === "provider"
      ? usage.byProvider.map((e) => ({
          key: e.provider,
          label: e.provider,
          bucket: e.bucket,
          today: e.today,
          providerReportsUsage: e.providerReportsUsage,
        }))
      : usage.byHandler.map((e) => ({
          key: e.handler,
          label: e.handler,
          bucket: e.bucket,
          today: e.today,
          providerReportsUsage: e.providerReportsUsage,
        }));
  const todayEntries = sortEntries(entries, (entry) => entry.today);
  const totalEntries = sortEntries(entries, (entry) => entry.bucket);

  function selectBreakdown(next: UsageBreakdown) {
    setBreakdown(next);
    if (workspaceKey) writeUiState(workspaceKey, { usageBreakdown: next });
  }

  return (
    <section
      className={`${className} rounded-lg border border-border-soft bg-card/40 px-4 py-3 flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between`}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="text-xs uppercase tracking-wide text-text-muted">
          {label}
        </div>
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="text-xs uppercase tracking-wide text-primary">
            近日
          </span>
          <span className="text-xl font-mono text-foreground">
            今日 {formatTokens(todayTokens)}
          </span>
          <span className="text-sm text-text-secondary">
            {usage.today.turns} turns
          </span>
        </div>
        <div
          data-testid="workspace-usage-today"
          className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-mono text-text-muted"
        >
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
          {todayEntries.map((entry) => (
            <BreakdownMetric key={entry.key} entry={entry} bucket={entry.today} />
          ))}
        </div>
        <div
          data-testid="workspace-usage-total"
          className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border-soft/80 pt-2 text-xs font-mono text-text-faint"
        >
          <span className="font-sans text-xs text-text-muted">
            累计 {formatTokens(totalTokens)}
          </span>
          {totalEntries.map((entry) => (
            <BreakdownMetric key={entry.key} entry={entry} bucket={entry.bucket} />
          ))}
        </div>
      </div>
      {sparklineValues.length > 0 && (
        <div className="flex shrink-0 flex-col items-start gap-1 text-primary lg:items-end">
          <span className="text-xs font-medium text-text-muted">近 30 天</span>
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

type UsageEntry = {
  key: string;
  label: string;
} & UsageBreakdownEntry;

function sortEntries(
  entries: UsageEntry[],
  bucketOf: (entry: UsageEntry) => UsageBucket,
): UsageEntry[] {
  return [...entries].sort((a, b) => {
    const diff = bucketTokenTotal(bucketOf(b)) - bucketTokenTotal(bucketOf(a));
    if (diff !== 0) return diff;
    const turnDiff = bucketOf(b).turns - bucketOf(a).turns;
    return turnDiff !== 0 ? turnDiff : a.label.localeCompare(b.label);
  });
}

function BreakdownMetric({
  entry,
  bucket,
}: {
  entry: UsageEntry;
  bucket: UsageBucket;
}) {
  return (
    <span>
      {entry.label}{" "}
      {entry.providerReportsUsage
        ? formatTokens(bucketTokenTotal(bucket))
        : `— · ${bucket.turns}t`}
    </span>
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
