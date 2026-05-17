import { useEffect, useId, useRef } from "react";
import type { FlowNodeSummary } from "@/lib/types";

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

export function FlowDAG({ nodes }: { nodes: FlowNodeSummary[] }) {
  const ref = useRef<HTMLDivElement>(null);
  const id = useId().replace(/:/g, "_");

  useEffect(() => {
    if (nodes.length === 0) return;
    const source = buildMermaidSource(nodes);
    let cancelled = false;
    void (async () => {
      const mermaid = (await import("mermaid")).default;
      mermaid.initialize({ startOnLoad: false, theme: "default" });
      try {
        const { svg } = await mermaid.render(`mermaid-${id}`, source);
        if (!cancelled && ref.current) ref.current.innerHTML = svg;
      } catch (e) {
        if (!cancelled && ref.current) {
          ref.current.textContent = `mermaid render failed: ${String(e)}`;
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [id, nodes]);

  if (nodes.length === 0) {
    return (
      <div className="italic text-muted-foreground text-sm">(no nodes)</div>
    );
  }

  return <div ref={ref} className="mermaid-container" />;
}
