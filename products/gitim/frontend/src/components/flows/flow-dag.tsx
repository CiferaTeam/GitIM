import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  lazy,
  Suspense,
} from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/button";
import type { FlowNodeSummary } from "@/lib/types";
import { cn } from "@/lib/utils";

const ReactMarkdown = lazy(() => import("react-markdown"));

export type SavePromptResult = { ok: boolean; error?: string };

/** Characters allowed in a node ID: a-z, 0-9, hyphen, underscore. */
const SAFE_NODE_ID_RE = /^[a-zA-Z0-9_-]+$/;

function isSafeNodeId(id: string): boolean {
  return SAFE_NODE_ID_RE.test(id);
}

function buildMermaidSource(nodes: FlowNodeSummary[]): string {
  const lines: string[] = ["flowchart TD"];
  for (const n of nodes) {
    // Defensive guard: skip nodes whose ID would break mermaid interpolation.
    // The Rust validator rejects these at write-time; this handles external/legacy files.
    if (!isSafeNodeId(n.id)) continue;
    if (!n.needs || n.needs.length === 0) {
      lines.push(`  ${n.id}["${escapeLabel(n.id)}"]`);
    }
    for (const dep of n.needs ?? []) {
      if (!isSafeNodeId(dep)) continue;
      lines.push(`  ${dep} --> ${n.id}`);
    }
  }
  return lines.join("\n");
}

function escapeLabel(s: string): string {
  return s.replace(/"/g, '\\"');
}

const TOOLTIP_OFFSET = 8;
const TOOLTIP_MAX_WIDTH = 448; // max-w-md
const TOOLTIP_MAX_HEIGHT = 384; // max-h-96
// Grace period before closing on mouseleave. Lets the cursor cross the
// TOOLTIP_OFFSET gap between node and tooltip without dismissing.
const TOOLTIP_CLOSE_DELAY_MS = 150;

export function FlowDAG({
  nodes,
  onSavePrompt,
}: {
  nodes: FlowNodeSummary[];
  onSavePrompt?: (
    nodeId: string,
    prompt: string,
  ) => Promise<SavePromptResult>;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const id = useId().replace(/:/g, "_");
  const [hoveredNode, setHoveredNode] = useState<FlowNodeSummary | null>(null);
  const [tooltipStyle, setTooltipStyle] = useState<React.CSSProperties>({});
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const editingRef = useRef(false);
  const cleanupRef = useRef<(() => void) | null>(null);
  const closeTimerRef = useRef<number | null>(null);

  // Mirror `editing` into a ref so handlers bound to native SVG events
  // (which capture state at attach time) can read the current value without
  // tearing down + re-binding on every edit toggle.
  useEffect(() => {
    editingRef.current = editing;
  }, [editing]);

  const cancelClose = useCallback(() => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
  }, []);

  const scheduleClose = useCallback(() => {
    if (editingRef.current) return;
    cancelClose();
    closeTimerRef.current = window.setTimeout(() => {
      setHoveredNode(null);
      closeTimerRef.current = null;
    }, TOOLTIP_CLOSE_DELAY_MS);
  }, [cancelClose]);

  const exitEdit = useCallback(() => {
    setEditing(false);
    setDraft("");
    setSaving(false);
    setSaveError(null);
  }, []);

  // Bind native mouseenter/leave to the tooltip so the cursor can park inside
  // it (e.g. to scroll the prompt) without the close timer firing. Native
  // listeners stay symmetric with the SVG node bindings and are easier to
  // exercise from tests than React's mouseover-derived synthetic events.
  useEffect(() => {
    const el = tooltipRef.current;
    if (!el) return;
    const enter = () => cancelClose();
    const leave = () => scheduleClose();
    el.addEventListener("mouseenter", enter);
    el.addEventListener("mouseleave", leave);
    return () => {
      el.removeEventListener("mouseenter", enter);
      el.removeEventListener("mouseleave", leave);
    };
  }, [hoveredNode, cancelClose, scheduleClose]);

  useEffect(() => {
    if (nodes.length === 0) return;
    const source = buildMermaidSource(nodes);
    let cancelled = false;

    void (async () => {
      const mermaid = (await import("mermaid")).default;
      mermaid.initialize({ startOnLoad: false, theme: "default" });
      try {
        const { svg } = await mermaid.render(`mermaid-${id}`, source);
        if (cancelled || !ref.current) return;
        ref.current.innerHTML = svg;

        // Bind hover/focus events to each rendered node.
        const nodeEls = ref.current.querySelectorAll<SVGGElement>(".node");
        const controllers: AbortController[] = [];

        nodeEls.forEach((el) => {
          // Resolve node ID from label text (primary) or id attribute (fallback).
          const label =
            el.querySelector(".nodeLabel")?.textContent?.trim() ??
            el.id?.replace(/^flowchart-[^-]+-/, "");
          const node = nodes.find((n) => n.id === label);
          if (!node) return;

          // Make focusable for keyboard navigation.
          el.setAttribute("tabindex", "0");
          el.setAttribute("role", "button");
          el.setAttribute("aria-label", `Node ${node.id}`);

          const show = (target: Element) => {
            // While the user is editing a prompt, the tooltip is locked to
            // the editing node — ignore hover/focus on other nodes so a
            // stray mouseover doesn't blow away the unsaved draft.
            if (editingRef.current) return;
            cancelClose();
            const rect = target.getBoundingClientRect();
            const viewportW = window.innerWidth;
            const viewportH = window.innerHeight;

            let left = rect.left + rect.width / 2;
            let top = rect.top - TOOLTIP_OFFSET;
            let flipBelow = false;

            // Flip to below if not enough room above.
            if (top < TOOLTIP_MAX_HEIGHT + TOOLTIP_OFFSET) {
              top = rect.bottom + TOOLTIP_OFFSET;
              flipBelow = true;
            }

            // Clamp horizontally.
            left = Math.max(
              TOOLTIP_MAX_WIDTH / 2 + 8,
              Math.min(viewportW - TOOLTIP_MAX_WIDTH / 2 - 8, left),
            );

            // Clamp vertically.
            top = Math.max(8, Math.min(viewportH - TOOLTIP_MAX_HEIGHT - 8, top));

            setTooltipStyle({
              position: "fixed",
              left,
              top,
              transform: flipBelow
                ? "translate(-50%, 0)"
                : "translate(-50%, -100%)",
              zIndex: 50,
            });
            setHoveredNode(node);
          };

          // Immediate close for keyboard/explicit dismiss; mouse leave goes
          // through scheduleClose so the cursor can reach the tooltip.
          // Locked while editing — Escape/blur on the SVG node must not yank
          // the textarea out from under the user.
          const hideImmediate = () => {
            if (editingRef.current) return;
            cancelClose();
            setHoveredNode(null);
          };

          const handleEnter = (e: Event) => show(e.currentTarget as Element);
          const handleLeave = () => scheduleClose();
          const handleFocus = (e: Event) => show(e.currentTarget as Element);
          const handleBlur = () => hideImmediate();
          const handleKeyDown = (e: KeyboardEvent) => {
            if (e.key === "Escape") hideImmediate();
          };

          el.addEventListener("mouseenter", handleEnter);
          el.addEventListener("mouseleave", handleLeave);
          el.addEventListener("focus", handleFocus);
          el.addEventListener("blur", handleBlur);
          el.addEventListener("keydown", handleKeyDown);

          const ctrl = new AbortController();
          controllers.push(ctrl);
          ctrl.signal.addEventListener("abort", () => {
            el.removeEventListener("mouseenter", handleEnter);
            el.removeEventListener("mouseleave", handleLeave);
            el.removeEventListener("focus", handleFocus);
            el.removeEventListener("blur", handleBlur);
            el.removeEventListener("keydown", handleKeyDown);
            el.removeAttribute("tabindex");
            el.removeAttribute("role");
            el.removeAttribute("aria-label");
          });
        });

        cleanupRef.current = () => {
          controllers.forEach((c) => c.abort());
        };
      } catch (e) {
        if (!cancelled && ref.current) {
          ref.current.textContent = `mermaid render failed: ${String(e)}`;
        }
      }
    })();

    return () => {
      cancelled = true;
      cleanupRef.current?.();
      cleanupRef.current = null;
      cancelClose();
      // Keep the tooltip pinned when nodes re-render mid-edit (e.g. parent
      // refresh from a sibling save) so the user's draft survives.
      if (!editingRef.current) setHoveredNode(null);
    };
  }, [id, nodes, cancelClose, scheduleClose]);

  if (nodes.length === 0) {
    return (
      <div className="italic text-muted-foreground text-sm">(no nodes)</div>
    );
  }

  async function handleSave() {
    if (!hoveredNode || !onSavePrompt) return;
    if (draft === (hoveredNode.prompt ?? "")) {
      exitEdit();
      return;
    }
    setSaving(true);
    setSaveError(null);
    const res = await onSavePrompt(hoveredNode.id, draft);
    if (!res.ok) {
      setSaving(false);
      setSaveError(res.error ?? "Failed to save prompt");
      return;
    }
    // Optimistically swap the displayed prompt to the saved draft so the
    // tooltip stays open in read mode without waiting for the parent's
    // async reload to land.
    setHoveredNode({ ...hoveredNode, prompt: draft });
    exitEdit();
  }

  function startEdit() {
    if (!hoveredNode) return;
    setDraft(hoveredNode.prompt ?? "");
    setSaveError(null);
    setEditing(true);
  }

  return (
    <>
      <div ref={ref} className="mermaid-container" />
      {hoveredNode &&
        createPortal(
          <div
            ref={tooltipRef}
            data-testid="flow-dag-tooltip"
            style={tooltipStyle}
            className="rounded-md border border-border bg-popover p-3 shadow-md outline-hidden"
          >
            <div className="mb-1 flex items-center justify-between gap-2">
              <span className="font-mono text-xs font-medium text-popover-foreground">
                {hoveredNode.id}
              </span>
              {onSavePrompt && !editing && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-6 px-2 text-xs"
                  onClick={startEdit}
                  data-testid="flow-dag-tooltip-edit"
                >
                  Edit
                </Button>
              )}
            </div>
            {editing ? (
              <div
                className="space-y-2"
                style={{ width: TOOLTIP_MAX_WIDTH }}
              >
                <textarea
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") {
                      e.preventDefault();
                      e.stopPropagation();
                      exitEdit();
                    }
                  }}
                  disabled={saving}
                  spellCheck={false}
                  data-testid="flow-dag-tooltip-textarea"
                  className={cn(
                    "w-full min-h-[10rem] resize-y rounded-md border border-input bg-background",
                    "px-2 py-1.5 font-mono text-xs leading-relaxed",
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
                    className="h-7 px-2 text-xs"
                    onClick={exitEdit}
                    disabled={saving}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    className="h-7 px-2 text-xs"
                    onClick={() => void handleSave()}
                    disabled={saving || draft === (hoveredNode.prompt ?? "")}
                    data-testid="flow-dag-tooltip-save"
                  >
                    {saving ? "Saving…" : "Save"}
                  </Button>
                </div>
              </div>
            ) : (
              <div
                className="prose prose-sm max-w-none overflow-y-auto dark:prose-invert"
                style={{ maxWidth: TOOLTIP_MAX_WIDTH, maxHeight: TOOLTIP_MAX_HEIGHT }}
              >
                {hoveredNode.prompt ? (
                  <Suspense
                    fallback={
                      <span className="text-xs text-muted-foreground">Loading…</span>
                    }
                  >
                    <ReactMarkdown>{hoveredNode.prompt}</ReactMarkdown>
                  </Suspense>
                ) : (
                  <span className="text-xs italic text-muted-foreground">
                    (no prompt body)
                  </span>
                )}
              </div>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
