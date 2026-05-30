import { useState } from "react";

import { Button } from "@/components/ui/button";
import type { FlowDocument, FlowNodeSummary, NodeType } from "@/lib/types";
import { cn } from "@/lib/utils";

import { FlowDAG } from "./flow-dag";

const NODE_TYPES: { value: NodeType; label: string }[] = [
  { value: "agent_mention", label: "agent" },
  { value: "channel_thread", label: "channel" },
  { value: "human_review", label: "review" },
  { value: "wait_for_signal", label: "wait" },
];

const NODE_ID_RE = /^[a-z0-9_-]+$/;

/** A node being edited. `_key` is a stable React key; `_isNew` gates id editing
 *  (existing node ids are immutable — rename = delete + re-add). */
interface DraftNode extends FlowNodeSummary {
  _key: string;
  _isNew: boolean;
}

let draftKeyCounter = 0;
function nextKey(): string {
  draftKeyCounter += 1;
  return `d${draftKeyCounter}`;
}

function seedDraft(nodes: FlowNodeSummary[]): DraftNode[] {
  return nodes.map((n) => ({
    ...n,
    participants: n.participants ?? [],
    needs: n.needs ?? [],
    required_labels: n.required_labels ?? [],
    prompt: n.prompt ?? "",
    _key: nextKey(),
    _isNew: false,
  }));
}

const inputCls = cn(
  "rounded-md border border-input bg-background px-2 py-1 text-sm",
  "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
);

/**
 * In-place editor for a flow's node structure. Pure controlled component:
 * `onSave` is injected by the parent (which wires it to the replaceFlow client
 * + reload + mode exit), so this component owns no IO and is unit-testable.
 */
export function FlowStructureEditor({
  doc,
  onSave,
  onCancel,
}: {
  doc: FlowDocument;
  onSave: (
    nodes: FlowNodeSummary[],
  ) => Promise<{ ok: boolean; error?: string }>;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<DraftNode[]>(() => seedDraft(doc.nodes));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function patchNode(key: string, patch: Partial<DraftNode>) {
    setDraft((cur) => cur.map((n) => (n._key === key ? { ...n, ...patch } : n)));
  }

  // Renaming a (new) node's id must follow into every dependent's needs, or the
  // checkbox list hides a now-stale reference and Save ships an unknown need.
  function handleIdChange(key: string, newId: string) {
    setDraft((cur) => {
      const oldId = cur.find((n) => n._key === key)?.id;
      return cur.map((n) => {
        if (n._key === key) return { ...n, id: newId };
        if (oldId && newId && (n.needs ?? []).includes(oldId)) {
          return {
            ...n,
            needs: (n.needs ?? []).map((d) => (d === oldId ? newId : d)),
          };
        }
        return n;
      });
    });
  }

  function addNode() {
    setDraft((cur) => [
      ...cur,
      {
        id: "",
        type: "agent_mention",
        owner: "",
        participants: [],
        needs: [],
        required_labels: [],
        prompt: "",
        _key: nextKey(),
        _isNew: true,
      },
    ]);
  }

  function removeNode(key: string) {
    setDraft((cur) => {
      const removedId = cur.find((n) => n._key === key)?.id;
      // Cascade: drop the removed id from every other node's needs so the
      // draft can never carry a dangling reference (which the backend would
      // reject anyway). Keeps the live preview and save path always valid.
      return cur
        .filter((n) => n._key !== key)
        .map((n) =>
          removedId
            ? { ...n, needs: (n.needs ?? []).filter((d) => d !== removedId) }
            : n,
        );
    });
  }

  function toggleNeed(key: string, dep: string) {
    setDraft((cur) =>
      cur.map((n) => {
        if (n._key !== key) return n;
        const has = (n.needs ?? []).includes(dep);
        return {
          ...n,
          needs: has
            ? (n.needs ?? []).filter((d) => d !== dep)
            : [...(n.needs ?? []), dep],
        };
      }),
    );
  }

  async function handleSave() {
    const ids = draft.map((n) => n.id.trim());
    if (ids.some((id) => !id)) {
      setError("每个节点都需要一个 id");
      return;
    }
    if (new Set(ids).size !== ids.length) {
      setError("节点 id 必须唯一");
      return;
    }
    const bad = ids.find((id) => !NODE_ID_RE.test(id));
    if (bad) {
      setError(`节点 id "${bad}" 只能包含 a-z 0-9 - _`);
      return;
    }

    setSaving(true);
    setError(null);
    const payload: FlowNodeSummary[] = draft.map((n) => ({
      id: n.id.trim(),
      type: n.type,
      owner: n.owner?.trim() || undefined,
      participants: n.participants?.length ? n.participants : undefined,
      signal: n.signal?.trim() || undefined,
      needs: n.needs?.length ? n.needs : undefined,
      // Carry exits through untouched — UI doesn't edit them but must not drop them.
      exits: n.exits?.length ? n.exits : undefined,
      required_labels: n.required_labels?.length
        ? n.required_labels
        : undefined,
      prompt: n.prompt ?? "",
    }));
    const res = await onSave(payload);
    setSaving(false);
    if (!res.ok) {
      setError(res.error ?? "保存失败");
    }
    // On success the parent reloads the flow and exits edit mode, unmounting us.
  }

  return (
    <div className="space-y-4">
      <div className="rounded-md border border-border bg-card p-4 overflow-x-auto">
        <FlowDAG nodes={draft} />
      </div>

      <div className="space-y-3">
        {draft.map((node) => {
          const otherNamed = draft.filter(
            (n) => n._key !== node._key && n.id.trim().length > 0,
          );
          return (
            <div
              key={node._key}
              data-testid="fse-node-row"
              className="space-y-3 rounded-md border border-border bg-card p-4"
            >
              <div className="flex items-center gap-2">
                <input
                  data-testid="fse-node-id"
                  value={node.id}
                  disabled={!node._isNew}
                  placeholder="node-id"
                  onChange={(e) => handleIdChange(node._key, e.target.value)}
                  className={cn(
                    inputCls,
                    "flex-1 font-mono",
                    !node._isNew && "opacity-60",
                  )}
                />
                <select
                  data-testid="fse-node-type"
                  value={node.type}
                  onChange={(e) =>
                    patchNode(node._key, { type: e.target.value as NodeType })
                  }
                  className={inputCls}
                >
                  {NODE_TYPES.map((t) => (
                    <option key={t.value} value={t.value}>
                      {t.label}
                    </option>
                  ))}
                </select>
                <Button
                  type="button"
                  data-testid="fse-remove"
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
                  onClick={() => removeNode(node._key)}
                >
                  ✕
                </Button>
              </div>

              {node.type === "agent_mention" && (
                <input
                  data-testid="fse-node-owner"
                  value={node.owner ?? ""}
                  placeholder="owner handler"
                  onChange={(e) =>
                    patchNode(node._key, { owner: e.target.value })
                  }
                  className={cn(inputCls, "w-full")}
                />
              )}
              {node.type === "channel_thread" && (
                <input
                  data-testid="fse-node-participants"
                  value={(node.participants ?? []).join(", ")}
                  placeholder="participants (逗号分隔)"
                  onChange={(e) =>
                    patchNode(node._key, {
                      participants: e.target.value
                        .split(",")
                        .map((s) => s.trim())
                        .filter(Boolean),
                    })
                  }
                  className={cn(inputCls, "w-full")}
                />
              )}
              {node.type === "wait_for_signal" && (
                <input
                  data-testid="fse-node-signal"
                  value={node.signal ?? ""}
                  placeholder="signal name"
                  onChange={(e) =>
                    patchNode(node._key, { signal: e.target.value })
                  }
                  className={cn(inputCls, "w-full")}
                />
              )}

              {otherNamed.length > 0 && (
                <div className="space-y-1">
                  <p className="text-xs font-semibold text-muted-foreground">
                    depends on
                  </p>
                  <div className="flex flex-wrap gap-2">
                    {otherNamed.map((other) => (
                      <label
                        key={other._key}
                        className="inline-flex items-center gap-1 text-xs"
                      >
                        <input
                          type="checkbox"
                          checked={(node.needs ?? []).includes(other.id)}
                          onChange={() => toggleNeed(node._key, other.id)}
                        />
                        <span className="font-mono">{other.id}</span>
                      </label>
                    ))}
                  </div>
                </div>
              )}

              <input
                data-testid="fse-node-labels"
                value={(node.required_labels ?? []).join(", ")}
                placeholder="labels (逗号分隔，可选)"
                onChange={(e) =>
                  patchNode(node._key, {
                    required_labels: e.target.value
                      .split(",")
                      .map((s) => s.trim())
                      .filter(Boolean),
                  })
                }
                className={cn(inputCls, "w-full")}
              />

              <textarea
                data-testid="fse-node-prompt"
                value={node.prompt ?? ""}
                placeholder="node prompt (markdown)"
                spellCheck={false}
                onChange={(e) =>
                  patchNode(node._key, { prompt: e.target.value })
                }
                className={cn(
                  inputCls,
                  "min-h-[6rem] w-full resize-y font-mono leading-relaxed",
                )}
              />
            </div>
          );
        })}
      </div>

      <Button
        type="button"
        data-testid="fse-add"
        variant="outline"
        size="sm"
        onClick={addNode}
      >
        + Add node
      </Button>

      {error && (
        <p data-testid="fse-error" className="text-sm text-destructive">
          {error}
        </p>
      )}

      <div className="flex justify-end gap-2 border-t border-border pt-3">
        <Button
          type="button"
          data-testid="fse-cancel"
          variant="ghost"
          size="sm"
          onClick={onCancel}
          disabled={saving}
        >
          Cancel
        </Button>
        <Button
          type="button"
          data-testid="fse-save"
          size="sm"
          onClick={() => void handleSave()}
          disabled={saving}
        >
          {saving ? "Saving…" : "Save structure"}
        </Button>
      </div>
    </div>
  );
}
