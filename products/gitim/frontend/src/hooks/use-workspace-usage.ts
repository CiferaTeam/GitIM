import { useMemo } from "react";
import { usageBucketTokenTotal } from "../lib/usage-totals";
import type { Agent, UsageBucket, UsageDayEntry, UsageSummary } from "../lib/types";
import { useAgentStore } from "./use-agent-store";

export interface UsageBreakdownEntry {
  /** Lifetime bucket for this provider/handler. Kept as `bucket` for the
   *  existing UI/tests that already consume cumulative breakdowns. */
  bucket: UsageBucket;
  /** Today's bucket for the same provider/handler grouping. */
  today: UsageBucket;
  bucketTokens: number;
  todayTokens: number;
  providerReportsUsage: boolean;
}

interface UsageDayTokenTotal {
  date: string;
  tokens: number;
}

/** Workspace-level rollup of all agents' usage_summary. Computed on the
 *  client because the runtime's GET /agents already returns each agent's
 *  full breakdown. Keeping this on the client side guarantees the workspace
 *  totals never disagree with the per-agent totals shown in the same UI. */
export interface WorkspaceUsage {
  totals: UsageBucket;
  today: UsageBucket;
  totalTokens: number;
  todayTokens: number;
  byDay: UsageDayEntry[];
  byDayTokens: UsageDayTokenTotal[];
  /** One entry per distinct provider across the workspace. Providers that
   *  do not report token usage remain visible with providerReportsUsage=false
   *  so the UI can render turns without implying zero tokens. */
  byProvider: ({ provider: string } & UsageBreakdownEntry)[];
  /** One entry per agent (handler is unique per agent). Zero-usage agents
   *  appear with a ZERO_BUCKET; providers that do not report token usage
   *  remain visible with providerReportsUsage=false. */
  byHandler: ({ handler: string } & UsageBreakdownEntry)[];
  /** True only when at least one agent in the workspace has produced
   *  usage data. Lets components branch into a "no data yet" state. */
  hasData: boolean;
}

const ZERO_BUCKET: UsageBucket = {
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheCreation: 0,
  turns: 0,
};

const EMPTY_USAGE: WorkspaceUsage = {
  totals: ZERO_BUCKET,
  today: ZERO_BUCKET,
  totalTokens: 0,
  todayTokens: 0,
  byDay: [],
  byDayTokens: [],
  byProvider: [],
  byHandler: [],
  hasData: false,
};

/**
 * Aggregate every agent's `usageSummary` into a workspace-level rollup.
 *
 * Selector stability: this pulls the raw `agents` reference straight out of
 * the zustand store (a stable identity that only changes when the array
 * shape changes) and only recomputes the rollup inside `useMemo`. Inlining
 * the aggregation in a selector would build a new object on every render
 * and trigger an infinite update loop — see
 * `project_zustand_selector_pitfalls` for the historical incident this
 * pattern guards against.
 */
export function useWorkspaceUsage(): WorkspaceUsage {
  const agents = useAgentStore((s) => s.agents);
  return useMemo(() => aggregateWorkspaceUsage(agents), [agents]);
}

/** Pure aggregator. Exposed for unit testing — the hook just memoizes
 *  this against the agents array reference. */
export function aggregateWorkspaceUsage(agents: Agent[]): WorkspaceUsage {
  const summaries = agents
    .map((a) => ({ provider: a.provider ?? "unknown", summary: a.usageSummary }))
    .filter(
      (e): e is { provider: string; summary: UsageSummary } =>
        e.summary !== undefined,
    );
  if (summaries.length === 0) return EMPTY_USAGE;

  const totals = mergeBuckets(summaries.map((e) => e.summary.totals));
  const today = mergeBuckets(summaries.map((e) => e.summary.today));
  const totalTokens = summaries.reduce(
    (acc, e) => acc + usageBucketTokenTotal(e.summary.totals, e.provider),
    0,
  );
  const todayTokens = summaries.reduce(
    (acc, e) => acc + usageBucketTokenTotal(e.summary.today, e.provider),
    0,
  );

  // Index every agent's by_day so the merger can reduce same-date buckets.
  // We use the longest individual byDay array as the canonical date set;
  // since the runtime always emits a 30-entry zero-filled window, every
  // agent contributes the same dates and aligning by index is sufficient.
  // Fall back to keying by date string when array lengths disagree (e.g.
  // an agent first seen mid-window — defensive).
  const byDay = mergeByDay(summaries.map((e) => e.summary.byDay));
  const byDayTokens = mergeByDayTokens(
    summaries.map((e) =>
      e.summary.byDay.map((day) => ({
        date: day.date,
        tokens: usageBucketTokenTotal(day.bucket, e.provider),
      })),
    ),
  );

  // byProvider: enumerate every distinct provider across ALL agents. A
  // provider whose agents have produced zero token usage still shows up as
  // `0`; a provider that does not report tokens shows up as turn-only data.
  const byProviderMap = new Map<string, UsageBreakdownEntry[]>();
  for (const a of agents) {
    const key = a.provider ?? "unknown";
    const entry = breakdownEntryForAgent(a.usageSummary, key);
    const arr = byProviderMap.get(key) ?? [];
    arr.push(entry);
    byProviderMap.set(key, arr);
  }
  const byProvider = Array.from(byProviderMap.entries())
    .map(([provider, entries]) => ({
      provider,
      ...mergeBreakdownEntries(entries),
    }))
    .sort(compareEntry((e) => e.provider));

  // byHandler: one entry per agent. Zero-usage agents get ZERO_BUCKET so the
  // UI can still list them; tokenless providers carry the same flag so the
  // renderer can avoid showing a misleading `0`.
  const byHandler = agents
    .map((a) => ({
      handler: a.id,
      ...breakdownEntryForAgent(a.usageSummary, a.provider ?? "unknown"),
    }))
    .sort(compareEntry((e) => e.handler));

  return {
    totals,
    today,
    totalTokens,
    todayTokens,
    byDay,
    byDayTokens,
    byProvider,
    byHandler,
    hasData: true,
  };
}

function breakdownEntryForAgent(
  summary: UsageSummary | undefined,
  provider: string,
): UsageBreakdownEntry {
  const bucket = summary?.totals ?? { ...ZERO_BUCKET };
  const today = summary?.today ?? { ...ZERO_BUCKET };
  return {
    bucket,
    today,
    bucketTokens: usageBucketTokenTotal(bucket, provider),
    todayTokens: usageBucketTokenTotal(today, provider),
    providerReportsUsage: summary?.providerReportsUsage ?? true,
  };
}

function mergeBreakdownEntries(entries: UsageBreakdownEntry[]): UsageBreakdownEntry {
  return {
    bucket: mergeBuckets(entries.map((e) => e.bucket)),
    today: mergeBuckets(entries.map((e) => e.today)),
    bucketTokens: entries.reduce((acc, e) => acc + e.bucketTokens, 0),
    todayTokens: entries.reduce((acc, e) => acc + e.todayTokens, 0),
    providerReportsUsage: entries.some((e) => e.providerReportsUsage),
  };
}

function compareEntry<T extends UsageBreakdownEntry>(
  labelOf: (e: T) => string,
) {
  return (a: T, b: T) => {
    const diff = b.bucketTokens - a.bucketTokens;
    return diff !== 0 ? diff : labelOf(a).localeCompare(labelOf(b));
  };
}

function mergeBuckets(buckets: UsageBucket[]): UsageBucket {
  return buckets.reduce(
    (acc, b) => ({
      input: acc.input + b.input,
      output: acc.output + b.output,
      cacheRead: acc.cacheRead + b.cacheRead,
      cacheCreation: acc.cacheCreation + b.cacheCreation,
      turns: acc.turns + b.turns,
    }),
    { ...ZERO_BUCKET },
  );
}

function mergeByDay(arrays: UsageDayEntry[][]): UsageDayEntry[] {
  if (arrays.length === 0) return [];
  // Group by date string; this tolerates agents joining mid-window where
  // their byDay arrays may not all be the same length.
  const byDate = new Map<string, UsageBucket[]>();
  for (const arr of arrays) {
    for (const entry of arr) {
      const buckets = byDate.get(entry.date) ?? [];
      buckets.push(entry.bucket);
      byDate.set(entry.date, buckets);
    }
  }
  return Array.from(byDate.entries())
    .map(([date, buckets]) => ({ date, bucket: mergeBuckets(buckets) }))
    .sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : 0));
}

function mergeByDayTokens(arrays: UsageDayTokenTotal[][]): UsageDayTokenTotal[] {
  const byDate = new Map<string, number>();
  for (const arr of arrays) {
    for (const entry of arr) {
      byDate.set(entry.date, (byDate.get(entry.date) ?? 0) + entry.tokens);
    }
  }
  return Array.from(byDate.entries())
    .map(([date, tokens]) => ({ date, tokens }))
    .sort((a, b) => (a.date < b.date ? -1 : a.date > b.date ? 1 : 0));
}
