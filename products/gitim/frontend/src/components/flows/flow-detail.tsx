import type { FlowDocument } from "@/lib/types";

// Stub — full implementation (mermaid DAG + markdown) comes in Task 16.
export function FlowDetail({ doc }: { doc: FlowDocument }) {
  return (
    <section className="min-h-0 overflow-y-auto px-4 py-4 md:px-6">
      <div className="mx-auto flex max-w-4xl flex-col gap-5">
        <header className="border-b border-border pb-4">
          <h2 className="break-all text-xl font-semibold">{doc.name}</h2>
          {doc.description && (
            <p className="mt-2 max-w-3xl break-words text-sm text-muted-foreground">
              {doc.description}
            </p>
          )}
          <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>{doc.slug}</span>
            {doc.updated_at && <span>{doc.updated_at}</span>}
            <span>{doc.nodes.length} nodes</span>
          </div>
        </header>
        <pre className="whitespace-pre-wrap break-words font-sans text-sm leading-6 text-foreground/90">
          {doc.raw_markdown}
        </pre>
      </div>
    </section>
  );
}
