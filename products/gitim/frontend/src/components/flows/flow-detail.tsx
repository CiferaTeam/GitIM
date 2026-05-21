import { lazy, Suspense, useEffect, useState } from "react";
import { Link } from "react-router";

import { Button } from "@/components/ui/button";
import { useFlowStore } from "@/hooks/use-flow-store";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { formatDateTime } from "@/lib/timezone";
import type { FlowDocument, FlowRunSummary } from "@/lib/types";
import { cn } from "@/lib/utils";

const ReactMarkdown = lazy(() => import("react-markdown"));
const FlowDAG = lazy(() =>
  import("./flow-dag").then((m) => ({ default: m.FlowDAG })),
);

export function FlowDetail({
  doc,
  onDeleted,
}: {
  doc: FlowDocument;
  onDeleted?: () => void;
}) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const timezone = useTimezoneStore((s) => s.timezone);
  const setSelectedSlug = useFlowStore((s) => s.setSelectedSlug);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [recentRuns, setRecentRuns] = useState<FlowRunSummary[]>([]);

  useEffect(() => {
    if (!activeSlug) return;
    let cancelled = false;
    client
      .listFlowRuns(activeSlug, { slug: doc.slug })
      .then((res) => {
        if (!cancelled)
          setRecentRuns(
            res.ok && res.data ? res.data.runs.slice(0, 10) : [],
          );
      })
      .catch(() => {
        if (!cancelled) setRecentRuns([]);
      });
    return () => {
      cancelled = true;
    };
  }, [activeSlug, doc.slug]);

  async function handleRemove() {
    if (!activeSlug) return;
    if (!window.confirm(`Soft-delete flow "${doc.slug}"?`)) return;
    setRemoving(true);
    setRemoveError(null);
    const res = await client.removeFlow(activeSlug, doc.slug);
    setRemoving(false);
    if (!res.ok) {
      setRemoveError(res.error ?? "Failed to remove flow");
      return;
    }
    // Clear selection so the detail panel closes immediately, then
    // refresh the list so the deleted flow disappears from the sidebar.
    setSelectedSlug(null);
    onDeleted?.();
  }

  return (
    <section className="min-h-0 overflow-y-auto px-4 py-4 md:px-6">
      <div className="mx-auto flex max-w-4xl flex-col gap-5">
        {/* Header */}
        <header className="border-b border-border pb-4">
          <div className="flex items-start justify-between gap-3">
            <h2 className="break-all text-xl font-semibold">{doc.name}</h2>
            <div className="flex shrink-0 gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  void navigator.clipboard.writeText(
                    `@coordinator 用 ${doc.slug}`,
                  );
                }}
                title="复制到剪贴板；粘到 channel 输入框 review 后发送"
              >
                Run this flow
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className={cn(
                  "text-destructive hover:text-destructive hover:bg-destructive/10",
                  removing && "opacity-50 pointer-events-none",
                )}
                onClick={() => void handleRemove()}
                disabled={removing}
              >
                {removing ? "Removing…" : "Remove"}
              </Button>
            </div>
          </div>
          {removeError && (
            <p className="mt-2 text-xs text-destructive">{removeError}</p>
          )}
          {doc.description && (
            <p className="mt-2 max-w-3xl break-words text-sm text-muted-foreground">
              {doc.description}
            </p>
          )}
          <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span className="font-mono">{doc.slug}</span>
            <span>·</span>
            <span>created by @{doc.created_by}</span>
            <span>·</span>
            <span>{formatDateTime(doc.created_at, timezone)}</span>
            {doc.updated_at && (
              <>
                <span>·</span>
                <span>updated {formatDateTime(doc.updated_at, timezone)}</span>
              </>
            )}
            <span>·</span>
            <span>{doc.nodes.length} nodes</span>
          </div>
        </header>

        {/* DAG */}
        <section>
          <h3 className="mb-2 text-sm font-semibold">DAG</h3>
          <div className="rounded-md border border-border bg-card p-4 overflow-x-auto">
            <Suspense
              fallback={
                <div className="text-xs text-muted-foreground">
                  Loading diagram...
                </div>
              }
            >
              <FlowDAG nodes={doc.nodes} />
            </Suspense>
          </div>
        </section>

        {/* Nodes */}
        {doc.nodes.length > 0 && (
          <section>
            <h3 className="mb-2 text-sm font-semibold">Nodes</h3>
            <div className="space-y-3">
              {doc.nodes.map((n) => (
                <div
                  key={n.id}
                  className="rounded-md border border-border bg-card p-4"
                >
                  <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                    <span className="font-mono text-sm font-medium">{n.id}</span>
                    <div className="flex flex-wrap gap-x-2 text-xs text-muted-foreground">
                      <span>{n.type}</span>
                      {n.owner && <span>· @{n.owner}</span>}
                      {n.participants && n.participants.length > 0 && (
                        <span>
                          · participants:{" "}
                          {n.participants.map((p) => `@${p}`).join(", ")}
                        </span>
                      )}
                      {n.needs && n.needs.length > 0 && (
                        <span>· needs: {n.needs.join(", ")}</span>
                      )}
                    </div>
                  </div>
                  {n.prompt ? (
                    <Suspense
                      fallback={
                        <div className="text-xs text-muted-foreground">
                          Loading…
                        </div>
                      }
                    >
                      <div className="prose prose-sm max-w-none dark:prose-invert">
                        <ReactMarkdown>{n.prompt}</ReactMarkdown>
                      </div>
                    </Suspense>
                  ) : (
                    <div className="text-xs italic text-muted-foreground">
                      (no prompt body)
                    </div>
                  )}
                </div>
              ))}
            </div>
          </section>
        )}

        {/* Recent runs */}
        {recentRuns.length > 0 && (
          <section>
            <h3 className="mb-2 text-sm font-semibold">Recent runs</h3>
            <div className="space-y-1">
              {recentRuns.map((r) => (
                <Link
                  key={r.run_id}
                  to={`/runs/${r.run_id}`}
                  className="block px-3 py-1.5 rounded hover:bg-muted text-sm font-mono"
                >
                  <span>{r.run_id}</span>
                  <span className="ml-2 text-xs text-muted-foreground">
                    [{r.status}] · {r.nodes_done}/{r.node_count} nodes · #
                    {r.channel}
                  </span>
                </Link>
              ))}
            </div>
          </section>
        )}
      </div>
    </section>
  );
}
