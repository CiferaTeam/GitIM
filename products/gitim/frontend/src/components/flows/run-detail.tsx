import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useParams } from "react-router";
import { ChevronLeft, ChevronRight } from "lucide-react";

import {
  cancelFlowRun as apiCancelFlowRun,
  getFlow as apiGetFlow,
  getFlowRun as apiGetFlowRun,
} from "@/lib/client";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { useFlowRunStore } from "@/hooks/use-flow-run-store";
import { Button } from "@/components/ui/button";
import { formatDateTime } from "@/lib/timezone";
import { cn } from "@/lib/utils";
import type { FlowDocument, NodeStatus, RunStatus } from "@/lib/types";

const FlowDAG = lazy(() =>
  import("./flow-dag").then((m) => ({ default: m.FlowDAG })),
);

const STATUS_TEXT: Record<NodeStatus, string> = {
  pending: "text-muted-foreground",
  in_progress: "text-warning",
  done: "text-success",
  failed: "text-destructive",
  skipped: "text-gray-400 line-through",
};

const STATUS_SURFACE: Record<NodeStatus | RunStatus, string> = {
  pending: "border-border bg-muted/50 text-foreground",
  in_progress: "border-warning/30 bg-warning/10 text-foreground",
  done: "border-success/30 bg-success/10 text-foreground",
  failed: "border-destructive/30 bg-destructive/10 text-foreground",
  skipped: "border-border bg-muted/40 text-muted-foreground opacity-75",
  cancelled: "border-border bg-muted/40 text-muted-foreground opacity-75",
};

const RUN_STEPS_PAGE_SIZE = 6;

export function RunDetail() {
  const { runId } = useParams<{ runId: string }>();
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const timezone = useTimezoneStore((s) => s.timezone);
  const selectedRun = useFlowRunStore((s) => s.selectedRun);
  const setSelectedRun = useFlowRunStore((s) => s.setSelectedRun);

  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [flowTemplate, setFlowTemplate] = useState<FlowDocument | null>(null);
  const [stepsPage, setStepsPage] = useState(0);
  const loadRequestRef = useRef(0);

  const loadRun = useCallback(
    async (slug: string, id: string) => {
      const requestId = loadRequestRef.current + 1;
      loadRequestRef.current = requestId;
      setLoading(true);
      setError(null);
      setFlowTemplate(null);
      try {
        const res = await apiGetFlowRun(slug, id);
        if (loadRequestRef.current !== requestId) return;
        if (res.ok && res.data) {
          setSelectedRun(res.data);
          const flowRes = await apiGetFlow(slug, res.data.flow_slug);
          if (loadRequestRef.current !== requestId) return;
          if (flowRes.ok && flowRes.data) {
            setFlowTemplate(flowRes.data);
          }
        } else {
          setError(res.error ?? "Failed to load run");
        }
      } catch (e: unknown) {
        if (loadRequestRef.current !== requestId) return;
        setError(String(e));
      } finally {
        if (loadRequestRef.current === requestId) {
          setLoading(false);
        }
      }
    },
    [setSelectedRun],
  );

  useEffect(() => {
    if (activeSlug && runId) {
      void loadRun(activeSlug, runId);
    }
    return () => {
      loadRequestRef.current += 1;
      setSelectedRun(null);
      setFlowTemplate(null);
    };
  }, [activeSlug, runId, loadRun, setSelectedRun]);

  useEffect(() => {
    setStepsPage(0);
  }, [selectedRun?.run_id]);

  const handleCancel = useCallback(async () => {
    if (!activeSlug || !selectedRun) return;
    const requestId = loadRequestRef.current;
    const requestSlug = activeSlug;
    const requestRunId = selectedRun.run_id;
    if (!confirm(`Cancel run ${requestRunId}?`)) return;
    try {
      const res = await apiCancelFlowRun(requestSlug, requestRunId);
      if (loadRequestRef.current !== requestId) return;
      if (res.ok) {
        void loadRun(requestSlug, requestRunId);
      } else {
        setError(res.error ?? "Cancel failed");
      }
    } catch (e: unknown) {
      if (loadRequestRef.current !== requestId) return;
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

  const flowNodeById = new Map(
    (flowTemplate?.nodes ?? []).map((node) => [node.id, node]),
  );
  const dagNodes = selectedRun.nodes.map((n) => ({
    id: n.id,
    type: flowNodeById.get(n.id)?.type ?? ("agent_mention" as const),
    owner: n.actor ?? flowNodeById.get(n.id)?.owner,
    participants: flowNodeById.get(n.id)?.participants,
    signal: flowNodeById.get(n.id)?.signal,
    needs: flowNodeById.get(n.id)?.needs ?? [],
    exits: flowNodeById.get(n.id)?.exits,
    required_labels: flowNodeById.get(n.id)?.required_labels,
    prompt: flowNodeById.get(n.id)?.prompt ?? "",
  }));

  const runStatusClass =
    STATUS_SURFACE[selectedRun.status] ?? STATUS_SURFACE.pending;
  const stepCount = selectedRun.nodes.length;
  const stepPageCount = Math.max(1, Math.ceil(stepCount / RUN_STEPS_PAGE_SIZE));
  const boundedStepsPage = Math.min(stepsPage, stepPageCount - 1);
  const stepStart = boundedStepsPage * RUN_STEPS_PAGE_SIZE;
  const visibleSteps = selectedRun.nodes.slice(
    stepStart,
    stepStart + RUN_STEPS_PAGE_SIZE,
  );
  const stepEnd = Math.min(stepStart + visibleSteps.length, stepCount);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-4xl flex-col gap-5 px-4 py-4 md:px-6">
        <header>
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-2xl font-bold font-mono">
                {selectedRun.run_id}
              </h1>
              <p className="text-sm text-muted-foreground">
                flow {selectedRun.flow_slug} · channel #{selectedRun.channel} ·
                by @{selectedRun.started_by}
              </p>
              <p className="text-xs text-muted-foreground">
                started {formatDateTime(selectedRun.started_at, timezone)} ·
                updated {formatDateTime(selectedRun.updated_at, timezone)}
              </p>
            </div>
            <div className="flex gap-2 items-center">
              <span
                className={cn(
                  "rounded border px-2 py-1 text-xs font-medium",
                  runStatusClass,
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

        <section data-testid="run-steps">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-lg font-semibold">Steps</h2>
            {stepCount > RUN_STEPS_PAGE_SIZE && (
              <div className="flex items-center gap-2">
                <span className="font-mono text-xs text-muted-foreground">
                  {stepStart + 1}-{stepEnd} of {stepCount}
                </span>
                <div className="flex items-center gap-1">
                  <Button
                    type="button"
                    variant="outline"
                    size="icon-sm"
                    aria-label="Previous step page"
                    title="Previous"
                    disabled={boundedStepsPage === 0}
                    onClick={() =>
                      setStepsPage(Math.max(0, boundedStepsPage - 1))
                    }
                  >
                    <ChevronLeft className="size-3.5" />
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon-sm"
                    aria-label="Next step page"
                    title="Next"
                    disabled={boundedStepsPage >= stepPageCount - 1}
                    onClick={() =>
                      setStepsPage(
                        Math.min(stepPageCount - 1, boundedStepsPage + 1),
                      )
                    }
                  >
                    <ChevronRight className="size-3.5" />
                  </Button>
                </div>
              </div>
            )}
          </div>
          <div className="space-y-2">
            {visibleSteps.map((n) => (
              <div
                key={n.id}
                className={cn(
                  "flex items-center justify-between rounded border px-3 py-2",
                  STATUS_SURFACE[n.status] ?? STATUS_SURFACE.pending,
                )}
              >
                <div className="font-mono">{n.id}</div>
                <div className="text-xs flex gap-2 items-center">
                  <span className={STATUS_TEXT[n.status]}>{n.status}</span>
                  {n.actor && <span>@{n.actor}</span>}
                  {n.completed_at && (
                    <span className="text-muted-foreground">
                      {n.completed_at}
                    </span>
                  )}
                </div>
              </div>
            ))}
          </div>
        </section>

        <section data-testid="run-dag">
          <h2 className="text-lg font-semibold mb-2">DAG</h2>
          <div className="max-h-[65vh] overflow-auto rounded-md border border-border bg-card p-4">
            <Suspense fallback={<div>Loading diagram...</div>}>
              <FlowDAG nodes={dagNodes} />
            </Suspense>
          </div>
        </section>
      </div>
    </div>
  );
}
