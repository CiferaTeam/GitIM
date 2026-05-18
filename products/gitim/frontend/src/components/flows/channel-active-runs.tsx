import { useEffect, useState } from "react";
import { Link } from "react-router-dom";

import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { listFlowRuns } from "@/lib/client";
import { cn } from "@/lib/utils";
import type { FlowRunSummary, RunStatus } from "@/lib/types";

const STATUS_PILL: Record<RunStatus, string> = {
  in_progress:
    "bg-yellow-100 dark:bg-yellow-950 text-yellow-800 dark:text-yellow-200",
  done: "bg-green-100 dark:bg-green-950 text-green-800 dark:text-green-200",
  failed: "bg-red-100 dark:bg-red-950 text-red-800 dark:text-red-200",
  cancelled: "bg-muted text-muted-foreground",
};

export function ChannelActiveRuns({ channel }: { channel: string }) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [runs, setRuns] = useState<FlowRunSummary[]>([]);

  useEffect(() => {
    if (!activeSlug) return;
    let cancelled = false;
    listFlowRuns(activeSlug, { channel, status: "in_progress" })
      .then((res) => {
        if (!cancelled) setRuns(res.ok && res.data ? res.data.runs : []);
      })
      .catch(() => {
        if (!cancelled) setRuns([]);
      });
    return () => {
      cancelled = true;
    };
  }, [activeSlug, channel]);

  if (runs.length === 0) return null;

  return (
    <div className="border-b bg-muted/30 px-4 py-2 flex flex-wrap gap-2 items-center">
      <span className="text-xs text-muted-foreground">Active runs:</span>
      {runs.map((r) => (
        <Link
          key={r.run_id}
          to={`/runs/${r.run_id}`}
          className={cn(
            "px-2 py-0.5 rounded text-xs font-mono hover:underline",
            STATUS_PILL[r.status],
          )}
          title={`${r.flow_slug} · by @${r.started_by}`}
        >
          {r.flow_slug} · {r.nodes_done}/{r.node_count}
        </Link>
      ))}
    </div>
  );
}
