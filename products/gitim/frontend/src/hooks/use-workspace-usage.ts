import { useMemo } from "react";
import type { Agent, UsageBucket, UsageDayEntry, UsageSummary } from "../lib/types";
import { useAgentStore } from "./use-agent-store";

/** Workspace-level rollup of all agents' usage_summary. Computed on the
 *  client because the runtime's GET /agents already returns each agent's
 *  full breakdown. Keeping this on the client side guarantees the workspace
 *  totals never disagree with the per-agent totals shown in the same UI. */
export interface WorkspaceUsage {
  totals: UsageBucket;
  today: UsageBucket;
  byDay: UsageDayEntry[];
  byProvider: { provider: string; bucket: UsageBucket }[];
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
  byDay: [],
  byProvider: [],
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

  // Index every agent's by_day so the merger can reduce same-date buckets.
  // We use the longest individual byDay array as the canonical date set;
  // since the runtime always emits a 30-entry zero-filled window, every
  // agent contributes the same dates and aligning by index is sufficient.
  // Fall back to keying by date string when array lengths disagree (e.g.
  // an agent first seen mid-window — defensive).
  const byDay = mergeByDay(summaries.map((e) => e.summary.byDay));

  const byProviderMap = new Map<string, UsageBucket[]>();
  for (const { provider, summary } of summaries) {
    const arr = byProviderMap.get(provider) ?? [];
    arr.push(summary.totals);
    byProviderMap.set(provider, arr);
  }
  const byProvider = Array.from(byProviderMap.entries())
    .map(([provider, buckets]) => ({
      provider,
      bucket: mergeBuckets(buckets),
    }))
    .sort((a, b) => totalSum(b.bucket) - totalSum(a.bucket));

  return {
    totals,
    today,
    byDay,
    byProvider,
    hasData: true,
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

function totalSum(b: UsageBucket): number {
  return b.input + b.output + b.cacheRead + b.cacheCreation;
}
