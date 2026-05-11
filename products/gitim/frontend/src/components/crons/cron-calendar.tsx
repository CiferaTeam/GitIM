import { useMemo, useState } from "react";
import { ChevronLeft, ChevronRight, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useCronTimeline } from "@/hooks/use-cron-timeline";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { CronTimelineEntry, CronTimelineKind } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  WEEKDAY_LABELS,
  buildMonthGrid,
  currentMonth,
  formatMonthLabel,
  groupEntriesByDay,
  monthRangeIso,
  shiftMonth,
  todayKey,
  type CalendarMonth,
} from "./calendar-utils";
import { CronDayPanel } from "./cron-day-panel";

// Visible entries per day before collapsing into "+N more". 3 keeps a single
// row of small chips visible inside a comfortably-sized cell at desktop
// breakpoints; more than that and the cell becomes a wall of text the user
// has to ignore to scan the calendar.
const MAX_VISIBLE_ENTRIES_PER_DAY = 3;

// Colour mapping — pulled from index.css design tokens so a future palette
// change propagates. The "kind" semantics here are wire-driven (runtime
// timeline endpoint). Past = success (it ran), future = primary blue
// (scheduled), missed = error (didn't run when it should have).
const KIND_STYLES: Record<
  CronTimelineKind,
  { dot: string; chip: string; label: string }
> = {
  past: {
    dot: "bg-success",
    chip: "bg-success/15 text-success border-success/25",
    label: "已执行",
  },
  future: {
    dot: "bg-primary",
    chip: "bg-primary/15 text-primary border-primary/25",
    label: "未来",
  },
  missed: {
    dot: "bg-error",
    chip: "bg-error/15 text-error border-error/25",
    label: "未执行",
  },
};

export function CronCalendar() {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);

  // Month state is local: each navigation triggers a new fetch via the hook.
  // `currentMonth()` is captured once at mount so navigation deltas don't
  // race against the wall clock if the user leaves the page open across
  // midnight UTC.
  const [month, setMonth] = useState<CalendarMonth>(() => currentMonth());

  // Memoize range strings so they're referentially stable per (year, month) —
  // useCronTimeline's useEffect deps include both, and a fresh object each
  // render would refetch on every parent re-render.
  const { from, to } = useMemo(() => monthRangeIso(month), [month]);

  const { entries, truncated, loading, error, refetch } = useCronTimeline(
    activeSlug,
    from,
    to,
  );

  const grid = useMemo(() => buildMonthGrid(month), [month]);
  const grouped = useMemo(() => groupEntriesByDay(entries), [entries]);

  const [selectedDayKey, setSelectedDayKey] = useState<string | null>(null);
  const selectedEntries = useMemo(() => {
    if (!selectedDayKey) return null;
    return grouped.get(selectedDayKey) ?? null;
  }, [grouped, selectedDayKey]);

  const today = todayKey();

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <h1 className="truncate text-xl font-semibold">周期任务</h1>
          <p className="truncate text-xs text-muted-foreground">
            cron 历史 · 未来预测 · 错过的任务（UTC 日历视图）
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setMonth((m) => shiftMonth(m, -1))}
            aria-label="Previous month"
          >
            <ChevronLeft className="size-4" />
          </Button>
          <span className="min-w-[7rem] text-center text-sm font-medium tabular-nums">
            {formatMonthLabel(month)}
          </span>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setMonth((m) => shiftMonth(m, 1))}
            aria-label="Next month"
          >
            <ChevronRight className="size-4" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => setMonth(currentMonth())}
          >
            今天
          </Button>
        </div>
      </header>

      {error && (
        <div className="flex items-center justify-between gap-3 border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">
          <span className="truncate">{error}</span>
          <button
            type="button"
            className="shrink-0 rounded border border-destructive/40 px-2 py-0.5 text-xs hover:bg-destructive/20"
            onClick={refetch}
          >
            重试
          </button>
        </div>
      )}

      {truncated && (
        <div
          role="status"
          className="border-b border-warning/30 bg-warning/10 px-4 py-2 text-xs text-warning"
        >
          时间窗内任务过多，部分结果被截断。可缩短视图或在 CLI 上 `gitim cron show`
          查看完整历史。
        </div>
      )}

      <div className="grid min-h-0 flex-1 overflow-hidden lg:grid-cols-[1fr_22rem]">
        <div className="flex min-h-0 flex-col overflow-hidden">
          <div className="grid shrink-0 grid-cols-7 border-b border-border bg-surface/30 text-[11px] font-medium text-muted-foreground">
            {WEEKDAY_LABELS.map((label, idx) => (
              <div
                key={idx}
                className="px-2 py-1.5 text-center uppercase tracking-wide"
              >
                {label}
              </div>
            ))}
          </div>

          {entries.length === 0 && !loading && !error ? (
            <EmptyCalendarGrid grid={grid} today={today} />
          ) : (
            <div className="grid min-h-0 flex-1 grid-cols-7 grid-rows-6 gap-px overflow-y-auto bg-border/40">
              {grid.map((cell) => {
                const dayEntries = grouped.get(cell.key) ?? [];
                const isToday = cell.key === today;
                const isSelected = cell.key === selectedDayKey;
                return (
                  <DayCell
                    key={cell.key}
                    dateLabel={cell.date.getUTCDate()}
                    inMonth={cell.inMonth}
                    isToday={isToday}
                    isSelected={isSelected}
                    entries={dayEntries}
                    onClick={() =>
                      dayEntries.length > 0
                        ? setSelectedDayKey(cell.key)
                        : setSelectedDayKey(null)
                    }
                  />
                );
              })}
            </div>
          )}
          {loading && (
            <div className="pointer-events-none absolute inset-0 flex items-start justify-end p-3">
              <Loader2
                aria-label="Loading"
                className="size-4 animate-spin text-muted-foreground"
              />
            </div>
          )}
        </div>

        <aside className="hidden min-w-0 overflow-y-auto border-l border-border bg-surface/20 lg:block">
          <CronDayPanel
            slug={activeSlug}
            dayKey={selectedDayKey}
            entries={selectedEntries}
            onClose={() => setSelectedDayKey(null)}
          />
        </aside>

        {/* Mobile / narrow viewport: show day detail as a stacked panel
            beneath the grid when a day is selected. lg:hidden flips it off
            once the side panel is available. */}
        {selectedDayKey && (
          <div className="border-t border-border bg-surface/30 lg:hidden">
            <CronDayPanel
              slug={activeSlug}
              dayKey={selectedDayKey}
              entries={selectedEntries}
              onClose={() => setSelectedDayKey(null)}
            />
          </div>
        )}
      </div>
    </div>
  );
}

interface DayCellProps {
  dateLabel: number;
  inMonth: boolean;
  isToday: boolean;
  isSelected: boolean;
  entries: CronTimelineEntry[];
  onClick: () => void;
}

function DayCell({
  dateLabel,
  inMonth,
  isToday,
  isSelected,
  entries,
  onClick,
}: DayCellProps) {
  const visible = entries.slice(0, MAX_VISIBLE_ENTRIES_PER_DAY);
  const overflow = entries.length - visible.length;
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex min-h-[88px] flex-col items-stretch gap-1 bg-background px-1.5 py-1 text-left transition-colors",
        "hover:bg-surface/40 focus:outline-none focus:ring-1 focus:ring-primary/60",
        !inMonth && "bg-background/40 text-muted-foreground/60",
        isSelected && "bg-primary/10 ring-1 ring-primary/40",
      )}
      aria-label={`${dateLabel}, ${entries.length} entries`}
    >
      <span
        className={cn(
          "self-end font-mono text-[11px] tabular-nums",
          isToday
            ? "rounded-full bg-primary px-1.5 py-0.5 text-primary-foreground"
            : "text-text-secondary",
          !inMonth && "text-muted-foreground/50",
        )}
      >
        {dateLabel}
      </span>
      <div className="flex flex-col gap-0.5">
        {visible.map((entry, idx) => (
          <CalendarEntryChip key={`${entry.cron_name}-${entry.ts}-${idx}`} entry={entry} />
        ))}
        {overflow > 0 && (
          <span className="px-1 py-0.5 text-[10px] text-muted-foreground">
            +{overflow} more
          </span>
        )}
      </div>
    </button>
  );
}

function CalendarEntryChip({ entry }: { entry: CronTimelineEntry }) {
  const style = KIND_STYLES[entry.kind];
  return (
    <span
      title={`${style.label} · ${entry.cron_name} · ${entry.ts}`}
      className={cn(
        "flex min-w-0 items-center gap-1 truncate rounded border px-1 py-0.5 font-mono text-[10px]",
        style.chip,
      )}
    >
      <span className={cn("size-1.5 shrink-0 rounded-full", style.dot)} aria-hidden />
      <span className="truncate">{entry.cron_name}</span>
    </span>
  );
}

function EmptyCalendarGrid({
  grid,
  today,
}: {
  grid: ReturnType<typeof buildMonthGrid>;
  today: string;
}) {
  return (
    <div className="relative grid min-h-0 flex-1 grid-cols-7 grid-rows-6 gap-px overflow-y-auto bg-border/40">
      {grid.map((cell) => (
        <div
          key={cell.key}
          className={cn(
            "flex min-h-[88px] flex-col items-end bg-background px-1.5 py-1",
            !cell.inMonth && "bg-background/40 text-muted-foreground/60",
          )}
        >
          <span
            className={cn(
              "font-mono text-[11px] tabular-nums text-text-secondary",
              cell.key === today &&
                "rounded-full bg-primary px-1.5 py-0.5 text-primary-foreground",
              !cell.inMonth && "text-muted-foreground/50",
            )}
          >
            {cell.date.getUTCDate()}
          </span>
        </div>
      ))}
      <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
        <p className="rounded-md border border-border bg-card/90 px-4 py-2 text-sm text-muted-foreground shadow-sm">
          这个时间窗内没有计划任务
        </p>
      </div>
    </div>
  );
}
