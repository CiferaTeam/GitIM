import { useCallback, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useCronTimeline } from "@/hooks/use-cron-timeline";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { displayTimezoneOption, type DisplayTimezone } from "@/lib/timezone";
import type { CronTimelineEntry, CronTimelineKind } from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  WEEKDAY_LABELS,
  buildMonthGrid,
  currentMonth,
  distinctCronCount,
  formatMonthLabel,
  groupEntriesByDay,
  formatEntryTime,
  monthRangeIso,
  shiftMonth,
  todayKey,
  type CalendarMonth,
} from "./calendar-utils";
import { CronDayPanel } from "./cron-day-panel";
import { KIND_STYLES, kindStyle } from "./kind-styles";

// Visible entries per day before collapsing into "+N more". 3 keeps a single
// row of small chips visible inside a comfortably-sized cell at desktop
// breakpoints; more than that and the cell becomes a wall of text the user
// has to ignore to scan the calendar.
const MAX_VISIBLE_ENTRIES_PER_DAY = 3;

// Visual style mapping lives in `./kind-styles.ts` so the day panel
// (and any future surface) renders the same kind → label/color/icon
// mapping. See the comment there for the canonical opacity convention.

// English month names for aria-label — kept inline rather than reusing
// `toLocaleString` so server/SSR-style consistency is guaranteed regardless
// of the jsdom locale. Chinese readers get the kind labels in Chinese, the
// date in numeric form below — this is the same compromise the rest of the
// crons page makes.
const MONTH_NAMES_EN = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

/** Build a screen-reader-friendly label for a day cell. Includes the full
 *  date, a count breakdown by kind, and (when more than one distinct cron
 *  exists on the day) a "N 个 cron" suffix so a user can hear "a day's
 *  48 fires come from 3 different crons" without opening the panel. The
 *  cron-count suffix is omitted for single-cron days where it would just
 *  read as "1 个 cron" — pure noise. */
function dayCellAriaLabel(date: Date, entries: CronTimelineEntry[]): string {
  const monthName = MONTH_NAMES_EN[date.getUTCMonth()] ?? "";
  const dayNum = date.getUTCDate();
  const year = date.getUTCFullYear();
  const datePart = `${monthName} ${dayNum}, ${year}`;
  if (entries.length === 0) return `${datePart}, 无任务`;
  const counts: Record<CronTimelineKind, number> = { past: 0, future: 0, missed: 0 };
  for (const e of entries) counts[e.kind] += 1;
  const parts: string[] = [];
  for (const k of ["past", "future", "missed"] as const) {
    if (counts[k] > 0) parts.push(`${counts[k]} ${KIND_STYLES[k].label}`);
  }
  const cronCount = distinctCronCount(entries);
  if (cronCount > 1) parts.push(`${cronCount} 个 cron`);
  return `${datePart}, ${parts.join(", ")}`;
}

export function CronCalendar() {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const timezone = useTimezoneStore((s) => s.timezone);
  const timezoneLabel = displayTimezoneOption(timezone).label;

  // Month state is local: each navigation triggers a new fetch via the hook.
  // `currentMonth()` is captured once at mount so navigation deltas don't
  // race against the wall clock if the user leaves the page open across
  // midnight UTC.
  const [month, setMonth] = useState<CalendarMonth>(() =>
    currentMonth(new Date(), timezone),
  );

  // Memoize range strings so they're referentially stable per (year, month) —
  // useCronTimeline's useEffect deps include both, and a fresh object each
  // render would refetch on every parent re-render.
  const { from, to } = useMemo(
    () => monthRangeIso(month, timezone),
    [month, timezone],
  );

  const { entries, truncated, loading, error, refetch } = useCronTimeline(
    activeSlug,
    from,
    to,
  );

  const grid = useMemo(() => buildMonthGrid(month), [month]);
  const grouped = useMemo(
    () => groupEntriesByDay(entries, timezone),
    [entries, timezone],
  );

  const [selectedDayKey, setSelectedDayKey] = useState<string | null>(null);
  const selectedEntries = useMemo(() => {
    if (!selectedDayKey) return null;
    return grouped.get(selectedDayKey) ?? null;
  }, [grouped, selectedDayKey]);

  // Track the most recently activated DayCell so we can restore focus to it
  // when the panel closes (Escape or X button). Without this, after Escape
  // the focus lands on <body> — accessibility regression where a sighted
  // keyboard user loses their place in the grid.
  const lastTriggerRef = useRef<HTMLButtonElement | null>(null);
  const handleClose = useCallback(() => {
    setSelectedDayKey(null);
    // queueMicrotask: the close usually unmounts the panel synchronously,
    // and focusing immediately works in practice. We still queue so an
    // edge case where the panel is the active document.activeElement
    // doesn't fight us — the focus call lands after React commits.
    queueMicrotask(() => {
      lastTriggerRef.current?.focus();
    });
  }, []);
  const handleDayClick = useCallback(
    (cellKey: string, _hasEntries: boolean, button: HTMLButtonElement | null) => {
      lastTriggerRef.current = button;
      // Open the panel on every cell click, even days with no entries.
      // Previously empty days were inert clicks — on mobile (no hover
      // tooltip available) this meant tapping an empty day did nothing,
      // hiding the "no scheduled tasks on this day" affordance. The
      // panel renders its own "当天没有计划任务" empty state.
      setSelectedDayKey(cellKey);
    },
    [],
  );

  const today = todayKey(new Date(), timezone);

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <h1 className="truncate text-xl font-semibold">周期任务</h1>
          <p className="truncate text-xs text-muted-foreground">
            cron 历史 · 未来预测 · 错过的任务（{timezoneLabel} 日历视图）
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
            onClick={() => setMonth(currentMonth(new Date(), timezone))}
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
        {/* `relative` anchors the absolutely-positioned loading spinner below
            so it lands at the top-right of the calendar column, not the app
            shell. Previously the spinner used `absolute inset-0` with no
            relative ancestor in this subtree and walked all the way up to
            the document. */}
        <div className="relative flex min-h-0 flex-col overflow-hidden">
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
                    date={cell.date}
                    inMonth={cell.inMonth}
                    isToday={isToday}
                    isSelected={isSelected}
                    entries={dayEntries}
                    timezone={timezone}
                    onActivate={(button) =>
                      handleDayClick(cell.key, dayEntries.length > 0, button)
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
            timezone={timezone}
            onClose={handleClose}
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
              timezone={timezone}
              onClose={handleClose}
            />
          </div>
        )}
      </div>
    </div>
  );
}

interface DayCellProps {
  date: Date;
  inMonth: boolean;
  isToday: boolean;
  isSelected: boolean;
  entries: CronTimelineEntry[];
  timezone: DisplayTimezone;
  /** Receives the activated button so the parent can park a ref for
   *  focus restoration on panel close. */
  onActivate: (button: HTMLButtonElement | null) => void;
}

function DayCell({
  date,
  inMonth,
  isToday,
  isSelected,
  entries,
  timezone,
  onActivate,
}: DayCellProps) {
  const visible = entries.slice(0, MAX_VISIBLE_ENTRIES_PER_DAY);
  const overflow = entries.length - visible.length;
  const buttonRef = useRef<HTMLButtonElement>(null);
  const ariaLabel = dayCellAriaLabel(date, entries);
  return (
    <button
      ref={buttonRef}
      type="button"
      onClick={() => onActivate(buttonRef.current)}
      className={cn(
        "flex min-h-[88px] flex-col items-stretch gap-1 bg-background px-1.5 py-1 text-left transition-colors",
        "hover:bg-surface/40 focus:outline-none focus:ring-1 focus:ring-primary/60",
        !inMonth && "bg-background/40 text-muted-foreground/60",
        isSelected && "bg-primary/10 ring-1 ring-primary/40",
      )}
      aria-label={ariaLabel}
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
        {date.getUTCDate()}
      </span>
      <div className="flex flex-col gap-0.5">
        {visible.map((entry, idx) => (
          <CalendarEntryChip
            key={`${entry.cron_name}-${entry.ts}-${idx}`}
            entry={entry}
            timezone={timezone}
          />
        ))}
        {overflow > 0 && (
          <span
            // Native `title` attribute is desktop-only (no mobile hover),
            // but on mobile the day cell is a button → tap opens the day
            // panel which surfaces the same info inline. No info is lost.
            title={`${entries.length} 个任务（${distinctCronCount(entries)} 个 cron）`}
            className="px-1 py-0.5 text-[10px] text-muted-foreground"
          >
            +{overflow} more
          </span>
        )}
      </div>
    </button>
  );
}

function CalendarEntryChip({
  entry,
  timezone,
}: {
  entry: CronTimelineEntry;
  timezone: DisplayTimezone;
}) {
  // `kindStyle` returns the `missed` style for unknown kinds — a future
  // `kind: "failed"` arriving from the runtime won't crash the calendar.
  const style = kindStyle(entry.kind);
  const Icon = style.Icon;
  return (
    <span
      title={`${style.label} · ${entry.cron_name} · ${formatEntryTime(entry.ts, timezone)}`}
      // `aria-label` puts the kind into the accessible name even when the
      // visible content is just the cron name. Without it, blind users
      // can't tell past from future from missed.
      aria-label={`${style.label}: ${entry.cron_name}`}
      className={cn(
        "flex min-w-0 items-center gap-1 truncate rounded border px-1 py-0.5 font-mono text-[10px]",
        style.chip,
      )}
    >
      {/* The icon is the visible non-color signal: WCAG 1.4.1 says color
          can't be the only conveyor of information. The colored dot is
          retained for sighted users who scan by hue. */}
      <Icon className="size-3 shrink-0" aria-hidden />
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
