import { lazy, Suspense, useState } from "react";

import { Button } from "@/components/ui/button";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { FlowDocument } from "@/lib/types";
import { cn } from "@/lib/utils";

import { FlowDAG } from "./flow-dag";

const ReactMarkdown = lazy(() => import("react-markdown"));

export function FlowDetail({ doc }: { doc: FlowDocument }) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);

  async function handleRemove() {
    if (!activeSlug) return;
    if (!window.confirm(`Soft-delete flow "${doc.slug}"?`)) return;
    setRemoving(true);
    setRemoveError(null);
    const res = await client.removeFlow(activeSlug, doc.slug);
    setRemoving(false);
    if (!res.ok) {
      setRemoveError(res.error ?? "Failed to remove flow");
    }
    // The parent FlowsView will detect the missing slug on next refresh.
    // Nothing else to do here — caller refreshes via its own subscription.
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
            <span>{doc.created_at}</span>
            {doc.updated_at && (
              <>
                <span>·</span>
                <span>updated {doc.updated_at}</span>
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
            <FlowDAG nodes={doc.nodes} />
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
      </div>
    </section>
  );
}
