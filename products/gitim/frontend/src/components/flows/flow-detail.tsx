import { ChevronRight } from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";
import { Link } from "react-router";

import { Button } from "@/components/ui/button";
import { useFlowStore } from "@/hooks/use-flow-store";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { formatDateTime } from "@/lib/timezone";
import type { FlowDocument, FlowNodeSummary, FlowRunSummary, NodeType } from "@/lib/types";
import { cn } from "@/lib/utils";

const ReactMarkdown = lazy(() => import("react-markdown"));
const FlowDAG = lazy(() =>
  import("./flow-dag").then((m) => ({ default: m.FlowDAG })),
);
const FlowStructureEditor = lazy(() =>
  import("./flow-structure-editor").then((m) => ({
    default: m.FlowStructureEditor,
  })),
);

const NODE_TYPE_LABEL: Record<NodeType, string> = {
  agent_mention: "agent",
  channel_thread: "channel",
  human_review: "review",
  wait_for_signal: "wait",
};

const NODE_TYPE_CLASS: Record<NodeType, string> = {
  agent_mention:
    "bg-sky-500/15 text-sky-700 ring-sky-500/30 dark:text-sky-300",
  channel_thread:
    "bg-violet-500/15 text-violet-700 ring-violet-500/30 dark:text-violet-300",
  human_review:
    "bg-amber-500/15 text-amber-700 ring-amber-500/30 dark:text-amber-300",
  wait_for_signal:
    "bg-zinc-500/15 text-zinc-700 ring-zinc-500/30 dark:text-zinc-300",
};

// Flow templates commonly write prompts as flat `Label:\n…body…` blocks.
// React-markdown renders that as one wall of text. We promote known labels
// to H4 so the reader sees structure without changing the on-disk format.
const KNOWN_LABELS = [
  "Prompt",
  "Inputs",
  "Outputs",
  "Gate",
  "Notes",
  "Context",
  "Goal",
  "Owner",
  "Participants",
] as const;
const LABEL_LINE_RE = new RegExp(
  String.raw`^(${KNOWN_LABELS.join("|")}):\s*$`,
  "gm",
);

function promotePromptLabels(prompt: string): string {
  return prompt.replace(LABEL_LINE_RE, "#### $1");
}

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
  const setSelectedFlow = useFlowStore((s) => s.setSelectedFlow);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [recentRuns, setRecentRuns] = useState<FlowRunSummary[]>([]);
  const [editingStructure, setEditingStructure] = useState(false);

  async function reloadFlow() {
    if (!activeSlug) return;
    const res = await client.getFlow(activeSlug, doc.slug);
    if (res.ok && res.data) setSelectedFlow(res.data);
  }

  async function handleSaveStructure(nodes: FlowNodeSummary[]) {
    if (!activeSlug) return { ok: false, error: "No active workspace" };
    const res = await client.replaceFlow(activeSlug, doc.slug, { nodes });
    if (res.ok) {
      await reloadFlow();
      setEditingStructure(false);
    }
    return { ok: res.ok, error: res.error };
  }

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
            {!editingStructure && (
              <div className="flex shrink-0 gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setEditingStructure(true)}
                >
                  Edit structure
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
            )}
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
          {recentRuns.length > 0 && (
            <RecentRunsDisclosure runs={recentRuns} />
          )}
        </header>

        {editingStructure ? (
          <Suspense
            fallback={
              <div className="text-xs text-muted-foreground">
                Loading editor…
              </div>
            }
          >
            <FlowStructureEditor
              doc={doc}
              onSave={handleSaveStructure}
              onCancel={() => setEditingStructure(false)}
            />
          </Suspense>
        ) : (
          <>
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
                  <FlowDAG
                    nodes={doc.nodes}
                    onSavePrompt={async (nodeId, prompt) => {
                      if (!activeSlug) {
                        return { ok: false, error: "No active workspace" };
                      }
                      const res = await client.updateFlowNodePrompt(
                        activeSlug,
                        doc.slug,
                        nodeId,
                        prompt,
                      );
                      if (res.ok) await reloadFlow();
                      return { ok: res.ok, error: res.error };
                    }}
                  />
                </Suspense>
              </div>
            </section>

            {/* Nodes */}
            {doc.nodes.length > 0 && (
              <section>
                <h3 className="mb-2 text-sm font-semibold">Nodes</h3>
                <div className="space-y-3">
                  {doc.nodes.map((n) => (
                    <FlowNodeCard
                      key={n.id}
                      node={n}
                      flowSlug={doc.slug}
                      onSaved={() => void reloadFlow()}
                    />
                  ))}
                </div>
              </section>
            )}
          </>
        )}
      </div>
    </section>
  );
}

function RecentRunsDisclosure({ runs }: { runs: FlowRunSummary[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-3 rounded-md border border-border bg-card text-sm">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-muted/50"
      >
        <ChevronRight
          className={cn(
            "h-3.5 w-3.5 shrink-0 transition-transform",
            open && "rotate-90",
          )}
          aria-hidden="true"
        />
        <span className="font-medium">Recent runs</span>
        <span className="text-xs text-muted-foreground">({runs.length})</span>
      </button>
      {open && (
        <div className="space-y-1 border-t border-border px-2 py-2">
          {runs.map((r) => (
            <Link
              key={r.run_id}
              to={`/runs/${r.run_id}`}
              className="block px-2 py-1 rounded hover:bg-muted text-sm font-mono"
            >
              <span>{r.run_id}</span>
              <span className="ml-2 text-xs text-muted-foreground">
                [{r.status}] · {r.nodes_done}/{r.node_count} nodes · #
                {r.channel}
              </span>
            </Link>
          ))}
        </div>
      )}
    </div>
  );
}

function MetaChip({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium ring-1 ring-inset",
        "bg-muted text-muted-foreground ring-border",
        className,
      )}
    >
      {children}
    </span>
  );
}

function FlowNodeCard({
  node,
  flowSlug,
  onSaved,
}: {
  node: FlowNodeSummary;
  flowSlug: string;
  onSaved: () => void;
}) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(node.prompt);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  function startEdit() {
    setDraft(node.prompt);
    setSaveError(null);
    setEditing(true);
  }

  function cancelEdit() {
    setDraft(node.prompt);
    setSaveError(null);
    setEditing(false);
  }

  async function save() {
    if (!activeSlug) return;
    if (draft === node.prompt) {
      setEditing(false);
      return;
    }
    setSaving(true);
    setSaveError(null);
    const res = await client.updateFlowNodePrompt(
      activeSlug,
      flowSlug,
      node.id,
      draft,
    );
    setSaving(false);
    if (!res.ok) {
      setSaveError(res.error ?? "Failed to save prompt");
      return;
    }
    setEditing(false);
    onSaved();
  }

  const rendered = node.prompt ? promotePromptLabels(node.prompt) : "";

  return (
    <div className="rounded-md border border-border bg-card p-4">
      <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
          <span className="font-mono text-sm font-semibold">{node.id}</span>
          <MetaChip className={NODE_TYPE_CLASS[node.type]}>
            {NODE_TYPE_LABEL[node.type]}
          </MetaChip>
          {node.owner && <MetaChip>owner @{node.owner}</MetaChip>}
          {node.participants && node.participants.length > 0 && (
            <MetaChip>
              with {node.participants.map((p) => `@${p}`).join(", ")}
            </MetaChip>
          )}
          {node.needs && node.needs.length > 0 && (
            <MetaChip>← {node.needs.join(", ")}</MetaChip>
          )}
        </div>
        {!editing && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs"
            onClick={startEdit}
          >
            Edit
          </Button>
        )}
      </div>

      {editing ? (
        <div className="space-y-2">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            disabled={saving}
            spellCheck={false}
            className={cn(
              "w-full min-h-[12rem] resize-y rounded-md border border-input bg-background",
              "px-3 py-2 font-mono text-sm leading-relaxed",
              "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              saving && "opacity-50",
            )}
          />
          {saveError && (
            <p className="text-xs text-destructive">{saveError}</p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={cancelEdit}
              disabled={saving}
            >
              Cancel
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => void save()}
              disabled={saving || draft === node.prompt}
            >
              {saving ? "Saving…" : "Save"}
            </Button>
          </div>
        </div>
      ) : node.prompt ? (
        <Suspense
          fallback={
            <div className="text-xs text-muted-foreground">Loading…</div>
          }
        >
          <div className="prose prose-sm max-w-none dark:prose-invert prose-h4:mt-3 prose-h4:mb-1 prose-h4:text-xs prose-h4:font-semibold prose-h4:uppercase prose-h4:tracking-wide prose-h4:text-muted-foreground">
            <ReactMarkdown>{rendered}</ReactMarkdown>
          </div>
        </Suspense>
      ) : (
        <div className="text-xs italic text-muted-foreground">
          (no prompt body — click Edit to add one)
        </div>
      )}
    </div>
  );
}
