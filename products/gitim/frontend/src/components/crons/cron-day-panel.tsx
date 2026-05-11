import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { CronTimelineEntry, CronTimelineKind } from "@/lib/types";
import { formatEntryTime } from "./calendar-utils";
import { CronRunViewer } from "./cron-run-viewer";
import { CronSpecDetail } from "./cron-spec-detail";

interface CronDayPanelProps {
  slug: string | null;
  dayKey: string | null;
  entries: CronTimelineEntry[] | null;
  onClose: () => void;
}

// Local view state for the panel. The day list is the default; clicking an
// entry pushes a per-kind detail view onto the panel. The panel doesn't
// route through the URL because (a) it's contextual to the calendar grid
// and (b) deep-linking a cron run is a v2 nice-to-have, not v1 essential.
type View =
  | { kind: "list" }
  | { kind: "run"; cronName: string; ts: string }
  | { kind: "spec"; cronName: string; ts: string; entryKind: "future" | "missed" };

const KIND_LABEL: Record<CronTimelineKind, string> = {
  past: "已执行",
  future: "未来",
  missed: "未执行",
};

const KIND_BADGE: Record<CronTimelineKind, string> = {
  past: "bg-success/15 text-success border-success/30",
  future: "bg-primary/15 text-primary border-primary/30",
  missed: "bg-error/15 text-error border-error/30",
};

export function CronDayPanel({ slug, dayKey, entries, onClose }: CronDayPanelProps) {
  const [view, setView] = useState<View>({ kind: "list" });

  // Reset to list whenever the selected day or workspace changes — a
  // detail view pinned to "yesterday's run" stops making sense once
  // the user navigates to a different day or switches workspaces.
  useEffect(() => {
    setView({ kind: "list" });
  }, [dayKey, slug]);

  // Escape closes the panel. We guard on `dayKey` so the listener is a no-op
  // when no panel is showing (component still rendered as the empty
  // placeholder). When inside a detail subview (`run` / `spec`), Escape
  // first pops back to the list — falling all the way to `onClose` on a
  // single keystroke would skip the day overview the user opened the
  // panel for.
  useEffect(() => {
    if (!dayKey) return;
    function handleKey(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      if (view.kind !== "list") {
        setView({ kind: "list" });
      } else {
        onClose();
      }
    }
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [dayKey, view.kind, onClose]);

  if (!dayKey) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-center text-sm text-muted-foreground">
        选中日历中的一天来查看详情
      </div>
    );
  }

  if (view.kind === "run") {
    return (
      <CronRunViewer
        slug={slug}
        cronName={view.cronName}
        ts={view.ts}
        onBack={() => setView({ kind: "list" })}
      />
    );
  }

  if (view.kind === "spec") {
    return (
      <CronSpecDetail
        slug={slug}
        cronName={view.cronName}
        missedTs={view.entryKind === "missed" ? view.ts : undefined}
        futureTs={view.entryKind === "future" ? view.ts : undefined}
        onBack={() => setView({ kind: "list" })}
      />
    );
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-4 py-3">
        <div className="min-w-0">
          <h2 className="text-sm font-semibold font-mono">{dayKey}</h2>
          <p className="text-xs text-muted-foreground">
            {entries?.length ?? 0} 个任务（UTC）
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onClose}
          aria-label="Close day panel"
        >
          <X className="size-4" />
        </Button>
      </header>

      {!entries || entries.length === 0 ? (
        <div className="flex flex-1 items-center justify-center p-4 text-sm text-muted-foreground">
          当天没有计划任务
        </div>
      ) : (
        <ol className="flex-1 space-y-1 overflow-y-auto px-3 py-2">
          {entries.map((entry, idx) => {
            // The filename stem (URL-safe) drops the colons that RFC 3339
            // uses in `entry.ts`. The runtime accepts both shapes for the
            // path param, but its validator is strict — match the format
            // the daemon writes on disk so canon-path checks pass.
            const stem = entry.ts.replace(/:/g, "-");
            const onClick = () => {
              if (entry.kind === "past") {
                setView({ kind: "run", cronName: entry.cron_name, ts: stem });
              } else {
                setView({
                  kind: "spec",
                  cronName: entry.cron_name,
                  ts: entry.ts,
                  entryKind: entry.kind,
                });
              }
            };
            return (
              <li key={`${entry.cron_name}-${entry.ts}-${idx}`}>
                <button
                  type="button"
                  onClick={onClick}
                  className="flex w-full items-center gap-2 rounded-md border border-transparent px-2 py-1.5 text-left transition-colors hover:bg-surface/60 focus:outline-none focus:ring-1 focus:ring-primary/40"
                >
                  <span className="shrink-0 font-mono text-[11px] tabular-nums text-text-secondary">
                    {formatEntryTime(entry.ts)}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">
                    {entry.cron_name}
                  </span>
                  <span
                    className={cn(
                      "shrink-0 rounded border px-1.5 py-0.5 text-[10px]",
                      KIND_BADGE[entry.kind],
                    )}
                  >
                    {KIND_LABEL[entry.kind]}
                  </span>
                </button>
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}
