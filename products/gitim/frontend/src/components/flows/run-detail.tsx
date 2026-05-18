import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import { useParams } from "react-router";

import {
  cancelFlowRun as apiCancelFlowRun,
  getFlowRun as apiGetFlowRun,
} from "@/lib/client";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { useFlowRunStore } from "@/hooks/use-flow-run-store";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { NodeStatus, RunStatus } from "@/lib/types";

const FlowDAG = lazy(() =>
  import("./flow-dag").then((m) => ({ default: m.FlowDAG })),
);

const STATUS_COLORS: Record<NodeStatus, string> = {
  pending: "text-muted-foreground",
  in_progress: "text-yellow-600",
  done: "text-green-600",
  failed: "text-red-600",
  skipped: "text-gray-400 line-through",
};

const STATUS_BG: Record<NodeStatus | RunStatus, string> = {
  pending: "bg-muted",
  in_progress: "bg-yellow-100 dark:bg-yellow-950",
  done: "bg-green-100 dark:bg-green-950",
  failed: "bg-red-100 dark:bg-red-950",
  skipped: "bg-muted opacity-60",
  cancelled: "bg-muted opacity-60",
};

export function RunDetail() {
  const { runId } = useParams<{ runId: string }>();
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const selectedRun = useFlowRunStore((s) => s.selectedRun);
  const setSelectedRun = useFlowRunStore((s) => s.setSelectedRun);

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadRun = useCallback(
    async (slug: string, id: string) => {
      setLoading(true);
      setError(null);
      try {
        const res = await apiGetFlowRun(slug, id);
        if (res.ok && res.data) {
          setSelectedRun(res.data);
        } else {
          setError(res.error ?? "Failed to load run");
        }
      } catch (e: unknown) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    },
    [setSelectedRun],
  );

  useEffect(() => {
    if (activeSlug && runId) {
      void loadRun(activeSlug, runId);
    }
    return () => {
      setSelectedRun(null);
    };
  }, [activeSlug, runId, loadRun, setSelectedRun]);

  const handleCancel = useCallback(async () => {
    if (!activeSlug || !selectedRun) return;
    if (!confirm(`Cancel run ${selectedRun.run_id}?`)) return;
    try {
      const res = await apiCancelFlowRun(activeSlug, selectedRun.run_id);
      if (res.ok) {
        void loadRun(activeSlug, selectedRun.run_id);
      } else {
        setError(res.error ?? "Cancel failed");
      }
    } catch (e: unknown) {
      setError(String(e));
    }
  }, [activeSlug, selectedRun, loadRun]);

  if (loading) {
    return <div className="p-6 text-muted-foreground">Loading...</div>;
  }
  if (error) {
    return <div className="p-6 text-destructive">{error}</div>;
  }
  if (!selectedRun) {
    return <div className="p-6 text-muted-foreground">Run not found.</div>;
  }

  // Build flat DAG nodes — edges not available from run state.yaml,
  // so the diagram renders nodes-only (no arrows). FlowDAG handles empty needs[].
  const dagNodes = selectedRun.nodes.map((n) => ({
    id: n.id,
    type: "agent_mention" as const,
    owner: n.actor,
    needs: [] as string[],
    prompt: "",
  }));

  const runStatusBg = STATUS_BG[selectedRun.status] ?? STATUS_BG.pending;

  return (
    <div className="p-6 space-y-6 max-w-4xl mx-auto">
      <header>
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-2xl font-bold font-mono">{selectedRun.run_id}</h1>
            <p className="text-sm text-muted-foreground">
              flow {selectedRun.flow_slug} · channel #{selectedRun.channel} · by
              @{selectedRun.started_by}
            </p>
            <p className="text-xs text-muted-foreground">
              started {selectedRun.started_at} · updated {selectedRun.updated_at}
            </p>
          </div>
          <div className="flex gap-2 items-center">
            <span
              className={cn(
                "px-2 py-1 rounded text-xs font-medium",
                runStatusBg,
              )}
            >
              {selectedRun.status}
            </span>
            {selectedRun.status === "in_progress" && (
              <Button
                size="sm"
                variant="outline"
                className="text-destructive"
                onClick={handleCancel}
              >
                Cancel run
              </Button>
            )}
          </div>
        </div>
      </header>

      <section>
        <h2 className="text-lg font-semibold mb-2">DAG</h2>
        <div className="border rounded p-4 bg-card overflow-x-auto">
          <Suspense fallback={<div>Loading diagram...</div>}>
            <FlowDAG nodes={dagNodes} />
          </Suspense>
        </div>
      </section>

      <section>
        <h2 className="text-lg font-semibold mb-2">Nodes</h2>
        <div className="space-y-2">
          {selectedRun.nodes.map((n) => (
            <div
              key={n.id}
              className={cn(
                "border rounded px-3 py-2 flex items-center justify-between",
                STATUS_BG[n.status] ?? STATUS_BG.pending,
              )}
            >
              <div className="font-mono">{n.id}</div>
              <div className="text-xs flex gap-2 items-center">
                <span className={STATUS_COLORS[n.status]}>{n.status}</span>
                {n.actor && <span>@{n.actor}</span>}
                {n.completed_at && (
                  <span className="text-muted-foreground">{n.completed_at}</span>
                )}
              </div>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
