import { useCallback, useEffect, useRef, useState } from "react";

import { RefreshCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useFlowStore } from "@/hooks/use-flow-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { FlowDocument, FlowSummary } from "@/lib/types";
import { cn } from "@/lib/utils";

import { FlowDetail } from "./flow-detail";

type LoadState = "idle" | "loading" | "error";

export function FlowsView() {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const flows = useFlowStore((s) => s.flows);
  const selectedSlug = useFlowStore((s) => s.selectedSlug);
  const selectedFlow = useFlowStore((s) => s.selectedFlow);
  const setFlows = useFlowStore((s) => s.setFlows);
  const setSelectedSlug = useFlowStore((s) => s.setSelectedSlug);
  const setSelectedFlow = useFlowStore((s) => s.setSelectedFlow);
  const resetForWorkspaceSwitch = useFlowStore((s) => s.resetForWorkspaceSwitch);

  const [listState, setListState] = useState<LoadState>("idle");
  const [detailState, setDetailState] = useState<LoadState>("idle");
  const [error, setError] = useState<string | null>(null);

  const [newSlug, setNewSlug] = useState("");
  const [newName, setNewName] = useState("");
  const [createSubmitting, setCreateSubmitting] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const activeSlugRef = useRef(activeSlug);
  const listRequestIdRef = useRef(0);

  useEffect(() => {
    activeSlugRef.current = activeSlug;
  }, [activeSlug]);

  // Reset store when switching workspaces.
  useEffect(() => {
    resetForWorkspaceSwitch();
  }, [activeSlug, resetForWorkspaceSwitch]);

  const refreshFlows = useCallback(
    async (options?: { reloadSelectedDetail?: boolean }) => {
      const requestSlug = activeSlug;
      if (!requestSlug) return;
      const requestId = listRequestIdRef.current + 1;
      listRequestIdRef.current = requestId;
      const previousSelected = options?.reloadSelectedDetail
        ? useFlowStore.getState().selectedSlug
        : null;
      setListState("loading");
      setError(null);
      const res = await client.listFlows(requestSlug);
      if (
        listRequestIdRef.current !== requestId ||
        activeSlugRef.current !== requestSlug
      ) {
        return;
      }
      if (!res.ok || !res.data) {
        setListState("error");
        setError(res.error ?? "Failed to load flows");
        return;
      }
      setFlows(res.data.flows);
      setListState("idle");
      if (
        !previousSelected ||
        !res.data.flows.some((f) => f.slug === previousSelected)
      ) {
        return;
      }
      setDetailState("loading");
      const detailRes = await client.getFlow(requestSlug, previousSelected);
      if (
        listRequestIdRef.current !== requestId ||
        activeSlugRef.current !== requestSlug ||
        useFlowStore.getState().selectedSlug !== previousSelected
      ) {
        return;
      }
      if (!detailRes.ok || !detailRes.data) {
        setDetailState("error");
        setError(detailRes.error ?? "Failed to load flow");
        return;
      }
      setSelectedFlow(detailRes.data);
      setDetailState("idle");
    },
    [activeSlug, setFlows, setSelectedFlow],
  );

  const handleCreate = useCallback(async () => {
    if (!activeSlug || !newSlug || !newName) return;
    setCreateSubmitting(true);
    setCreateError(null);
    const res = await client.createFlow(activeSlug, newSlug, newName, "");
    setCreateSubmitting(false);
    if (!res.ok) {
      setCreateError(res.error ?? "Failed to create flow");
      return;
    }
    setNewSlug("");
    setNewName("");
    await refreshFlows();
  }, [activeSlug, newSlug, newName, refreshFlows]);

  useEffect(() => {
    let cancelled = false;
    queueMicrotask(() => {
      if (!cancelled) void refreshFlows();
    });
    return () => {
      cancelled = true;
    };
  }, [refreshFlows]);

  // Load detail when selection changes.
  useEffect(() => {
    const requestSlug = activeSlug;
    const requestFlowSlug = selectedSlug;
    if (!requestSlug || !requestFlowSlug) {
      setSelectedFlow(null);
      return;
    }
    const wsSlug: string = requestSlug;
    const flowSlug: string = requestFlowSlug;
    let cancelled = false;
    void loadDetail();
    async function loadDetail() {
      await Promise.resolve();
      if (cancelled) return;
      setDetailState("loading");
      setError(null);
      const res = await client.getFlow(wsSlug, flowSlug);
      if (cancelled) return;
      if (!res.ok || !res.data) {
        setDetailState("error");
        setError(res.error ?? "Failed to load flow");
        return;
      }
      setSelectedFlow(res.data);
      setDetailState("idle");
    }
    return () => {
      cancelled = true;
    };
  }, [activeSlug, selectedSlug, setSelectedFlow]);

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <h1 className="truncate text-xl font-semibold">Flows</h1>
          <p className="truncate text-xs text-muted-foreground">
            Workflow templates for the current workspace
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="icon-sm"
          aria-label="Refresh flows"
          title="Refresh flows"
          onClick={() => void refreshFlows({ reloadSelectedDetail: true })}
        >
          <RefreshCcw
            className={cn("size-4", listState === "loading" && "animate-spin")}
          />
        </Button>
      </div>

      {error && (
        <div className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      <div className="grid min-h-0 flex-1 grid-rows-[auto_1fr] overflow-hidden md:grid-cols-[18rem_1fr] md:grid-rows-1">
        <FlowSidebar
          flows={flows}
          loading={listState === "loading"}
          selectedSlug={selectedSlug}
          onSelect={setSelectedSlug}
          newSlug={newSlug}
          newName={newName}
          onNewSlugChange={setNewSlug}
          onNewNameChange={setNewName}
          createSubmitting={createSubmitting}
          createError={createError}
          onClearCreateError={() => setCreateError(null)}
          onCreate={() => void handleCreate()}
        />
        <FlowDetailPanel
          flows={flows}
          flow={selectedFlow}
          loading={detailState === "loading"}
        />
      </div>
    </div>
  );
}

function FlowSidebar({
  flows,
  loading,
  selectedSlug,
  onSelect,
  newSlug,
  newName,
  onNewSlugChange,
  onNewNameChange,
  createSubmitting,
  createError,
  onClearCreateError,
  onCreate,
}: {
  flows: FlowSummary[];
  loading: boolean;
  selectedSlug: string | null;
  onSelect: (slug: string) => void;
  newSlug: string;
  newName: string;
  onNewSlugChange: (v: string) => void;
  onNewNameChange: (v: string) => void;
  createSubmitting: boolean;
  createError: string | null;
  onClearCreateError: () => void;
  onCreate: () => void;
}) {
  const canCreate = newSlug.trim().length > 0 && newName.trim().length > 0;

  return (
    <aside className="flex flex-col border-b border-border md:border-b-0 md:border-r">
      <div className="border-b border-border px-3 py-3">
        <p className="mb-2 text-xs font-semibold text-muted-foreground uppercase tracking-wide">
          New flow
        </p>
        <div className="space-y-1.5">
          <Input
            placeholder="slug (e.g. release)"
            value={newSlug}
            onChange={(e) => {
              onClearCreateError();
              onNewSlugChange(e.target.value);
            }}
            disabled={createSubmitting}
            className="h-8 text-sm"
          />
          <Input
            placeholder="name"
            value={newName}
            onChange={(e) => {
              onClearCreateError();
              onNewNameChange(e.target.value);
            }}
            disabled={createSubmitting}
            className="h-8 text-sm"
          />
          {createError && (
            <p className="text-xs text-destructive">{createError}</p>
          )}
          <Button
            type="button"
            size="sm"
            className="w-full"
            disabled={!canCreate || createSubmitting}
            onClick={onCreate}
          >
            {createSubmitting ? "Creating..." : "+ Create"}
          </Button>
        </div>
      </div>

      <div className="flex flex-1 gap-2 overflow-x-auto px-3 py-3 md:h-full md:flex-col md:overflow-y-auto md:overflow-x-hidden">
        {flows.length === 0 && !loading && (
          <p className="text-xs text-muted-foreground">
            No flows yet — create one above.
          </p>
        )}
        {flows.map((flow) => {
          const selected = flow.slug === selectedSlug;
          return (
            <button
              key={flow.slug}
              type="button"
              onClick={() => onSelect(flow.slug)}
              className={cn(
                "min-w-[14rem] max-w-[16rem] rounded-md border px-3 py-2 text-left transition-colors md:min-w-0 md:max-w-none",
                selected
                  ? "border-primary bg-primary/10 text-foreground"
                  : "border-border bg-card/40 hover:bg-accent/50",
              )}
            >
              <div className="flex min-w-0 items-center justify-between gap-2">
                <span className="truncate text-sm font-medium">{flow.name}</span>
                <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {flow.node_count} nodes
                </span>
              </div>
              <p className="mt-1 truncate text-[10px] text-muted-foreground/80">
                {flow.slug}
              </p>
              {flow.description && (
                <p className="mt-1 line-clamp-2 break-words text-xs text-muted-foreground">
                  {flow.description}
                </p>
              )}
            </button>
          );
        })}
      </div>
    </aside>
  );
}

function FlowDetailPanel({
  flows,
  flow,
  loading,
}: {
  flows: FlowSummary[];
  flow: FlowDocument | null;
  loading: boolean;
}) {
  if (!flow) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center p-8 text-sm text-muted-foreground">
        {loading
          ? "Loading flow..."
          : flows.length === 0
            ? "Create your first flow using the form on the left"
            : "Select a flow to view its template"}
      </div>
    );
  }

  return <FlowDetail doc={flow} />;
}
