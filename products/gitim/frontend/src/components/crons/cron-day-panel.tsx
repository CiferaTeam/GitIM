import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { CronTimelineEntry, CronTimelineKind } from "@/lib/types";
import { formatEntryTime, groupEntriesByHour, type HourGroup } from "./calendar-utils";
import { CronRunViewer } from "./cron-run-viewer";
import { CronSpecDetail } from "./cron-spec-detail";
import { kindStyle, KIND_STYLES } from "./kind-styles";

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

const LIST_VIEW: View = { kind: "list" };

// Above this entry count we collapse the day into hour-of-day groups so a
// high-frequency cron (e.g. `*/30 * * * *` = 48/day) doesn't dump a wall
// of rows at the user. The threshold is "a screen of rows", chosen so a
// `*/30` cron only spanning a working block (12 entries) still renders
// flat — grouping below this would be friction, not help.
const HOUR_GROUPING_THRESHOLD = 12;

// Kind label + badge styling are imported from `./kind-styles.ts` so the
// day list, the calendar chips, and any future kind-styled surface stay
// in lock-step on opacities and Chinese labels.

export function CronDayPanel({ slug, dayKey, entries, onClose }: CronDayPanelProps) {
  const panelKey = `${slug ?? ""}\0${dayKey ?? ""}`;
  const [viewState, setViewState] = useState<{ key: string; view: View }>({
    key: panelKey,
    view: LIST_VIEW,
  });
  // Hour-group expansion is panel-local and resets on panel re-open.
  // Storing as a Set keeps the default state (collapsed) cheap: an empty
  // Set means everything is folded.
  const [expandedState, setExpandedState] = useState<{
    key: string;
    expanded: Set<string>;
  }>({ key: panelKey, expanded: new Set() });
  if (viewState.key !== panelKey) {
    setViewState({ key: panelKey, view: LIST_VIEW });
  }
  if (expandedState.key !== panelKey) {
    setExpandedState({ key: panelKey, expanded: new Set() });
  }
  const view = viewState.key === panelKey ? viewState.view : LIST_VIEW;
  const expandedHours =
    expandedState.key === panelKey ? expandedState.expanded : new Set<string>();
  const setPanelView = (nextView: View) => {
    setViewState({ key: panelKey, view: nextView });
  };
  const toggleHour = (hourKey: string) => {
    setExpandedState((prev) => {
      const next = new Set(prev.key === panelKey ? prev.expanded : []);
      if (next.has(hourKey)) {
        next.delete(hourKey);
      } else {
        next.add(hourKey);
      }
      return { key: panelKey, expanded: next };
    });
  };

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
        setViewState({ key: panelKey, view: LIST_VIEW });
      } else {
        onClose();
      }
    }
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [dayKey, panelKey, view.kind, onClose]);

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
        onBack={() => setPanelView(LIST_VIEW)}
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
        onBack={() => setPanelView(LIST_VIEW)}
      />
    );
  }

  const grouped = entries && entries.length > HOUR_GROUPING_THRESHOLD;

  const openEntry = (entry: CronTimelineEntry) => {
    // The filename stem (URL-safe) drops the colons that RFC 3339 uses
    // in `entry.ts`. The runtime accepts both shapes for the path
    // param, but its validator is strict — match the format the daemon
    // writes on disk so canon-path checks pass.
    const stem = entry.ts.replace(/:/g, "-");
    if (entry.kind === "past") {
      setPanelView({ kind: "run", cronName: entry.cron_name, ts: stem });
    } else {
      setPanelView({
        kind: "spec",
        cronName: entry.cron_name,
        ts: entry.ts,
        entryKind: entry.kind,
      });
    }
  };

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
      ) : grouped ? (
        <ol className="flex-1 space-y-1 overflow-y-auto px-3 py-2">
          {groupEntriesByHour(entries).map((group) => (
            <HourGroupSection
              key={group.hourKey}
              group={group}
              expanded={expandedHours.has(group.hourKey)}
              onToggle={() => toggleHour(group.hourKey)}
              onEntryClick={openEntry}
            />
          ))}
        </ol>
      ) : (
        <ol className="flex-1 space-y-1 overflow-y-auto px-3 py-2">
          {entries.map((entry, idx) => (
            <li key={`${entry.cron_name}-${entry.ts}-${idx}`}>
              <EntryRow entry={entry} onClick={() => openEntry(entry)} />
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

/** One row in the day-panel list, used both flat (≤12 entries) and
 *  inside an expanded hour group (>12). */
function EntryRow({
  entry,
  onClick,
}: {
  entry: CronTimelineEntry;
  onClick: () => void;
}) {
  const style = kindStyle(entry.kind);
  return (
    <button
      type="button"
      onClick={onClick}
      data-testid="entry-row"
      className="flex w-full items-center gap-2 rounded-md border border-transparent px-2 py-1.5 text-left transition-colors hover:bg-surface/60 focus:outline-none focus:ring-1 focus:ring-primary/40"
    >
      <span className="shrink-0 font-mono text-[11px] tabular-nums text-text-secondary">
        {formatEntryTime(entry.ts)}
      </span>
      {/* Per-handler hue would need a new palette + colorblind audit, so
          we stay on the existing muted-foreground token. `@target` reads
          as a handler in chat conventions throughout the rest of GitIM,
          so the prefix is doing the legwork of "this is who runs it". */}
      <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
        @{entry.target}
      </span>
      <span className="min-w-0 flex-1 truncate font-mono text-xs">
        {entry.cron_name}
      </span>
      <span
        className={cn(
          "shrink-0 rounded border px-1.5 py-0.5 text-[10px]",
          style.chip,
        )}
      >
        {style.label}
      </span>
    </button>
  );
}

/** Collapsible hour bucket. Header is a button (full keyboard support
 *  via native button semantics) with `aria-expanded` reflecting state. */
function HourGroupSection({
  group,
  expanded,
  onToggle,
  onEntryClick,
}: {
  group: HourGroup;
  expanded: boolean;
  onToggle: () => void;
  onEntryClick: (entry: CronTimelineEntry) => void;
}) {
  // Per-kind counts feed both the header dots and the aria-label so
  // screen readers hear the breakdown without seeing the visual dots.
  const counts: Record<CronTimelineKind, number> = {
    past: 0,
    future: 0,
    missed: 0,
  };
  for (const e of group.entries) {
    if (e.kind === "past" || e.kind === "future" || e.kind === "missed") {
      counts[e.kind] += 1;
    }
  }
  const kindParts: string[] = [];
  for (const k of ["past", "future", "missed"] as const) {
    if (counts[k] > 0) kindParts.push(`${counts[k]} ${KIND_STYLES[k].label}`);
  }
  const ariaLabel = `${group.label} · ${group.entries.length} 个任务${
    kindParts.length > 0 ? `, ${kindParts.join(", ")}` : ""
  }`;
  return (
    <li data-testid="hour-group">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        aria-label={ariaLabel}
        data-testid="hour-group-header"
        className="flex w-full items-center gap-2 rounded-md border border-transparent px-2 py-1.5 text-left transition-colors hover:bg-surface/60 focus:outline-none focus:ring-1 focus:ring-primary/40"
      >
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-text-secondary">
          {group.label}
        </span>
        <span className="flex-1 truncate text-xs text-muted-foreground">
          {group.entries.length} 个任务
        </span>
        <span className="flex shrink-0 items-center gap-1" aria-hidden>
          {(["past", "future", "missed"] as const).map((k) =>
            counts[k] > 0 ? (
              <span
                key={k}
                data-testid="kind-dot"
                className={cn("size-1.5 rounded-full", KIND_STYLES[k].dot)}
              />
            ) : null,
          )}
        </span>
      </button>
      {expanded && (
        <ol className="mt-1 space-y-1 pl-4">
          {group.entries.map((entry, idx) => (
            <li key={`${entry.cron_name}-${entry.ts}-${idx}`}>
              <EntryRow entry={entry} onClick={() => onEntryClick(entry)} />
            </li>
          ))}
        </ol>
      )}
    </li>
  );
}
