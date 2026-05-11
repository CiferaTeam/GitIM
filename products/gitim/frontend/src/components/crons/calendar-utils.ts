// Calendar grid maths — kept pure so the day-bucketing logic is testable
// without rendering. All operations live in UTC because the timeline
// endpoint emits RFC 3339 UTC and the cron engine fires on UTC instants.
// Local-timezone rendering would create the classic "DST split a day in
// two" hazard for negligible UX value.

import type { CronTimelineEntry } from "@/lib/types";

export interface CalendarMonth {
  /** UTC year (e.g. 2026). */
  year: number;
  /** UTC month, 1-12. */
  month: number;
}

export interface CalendarDayCell {
  /** Date for this cell, anchored to 00:00:00 UTC. */
  date: Date;
  /** `true` when the cell is part of the focused month
   *  (`false` for the prev/next month bleed-in cells at grid edges). */
  inMonth: boolean;
  /** Stable key formed from the UTC date (`YYYY-MM-DD`). */
  key: string;
}

/** First day-of-week for the grid header. UTC week starts on Sunday to match
 *  the Western convention; agents using gitim are global and we're not
 *  bidi-localizing today. v2 can revisit if a locale switch lands. */
const DAYS_PER_WEEK = 7;
const WEEKS_IN_GRID = 6;

export const WEEKDAY_LABELS = ["S", "M", "T", "W", "T", "F", "S"] as const;

/** Month label for the header, e.g. "May 2026". UTC-anchored. */
export function formatMonthLabel(m: CalendarMonth): string {
  const date = new Date(Date.UTC(m.year, m.month - 1, 1));
  const monthName = date.toLocaleString("en-US", {
    month: "long",
    timeZone: "UTC",
  });
  return `${monthName} ${m.year}`;
}

/** Pad a number to two digits — used for date keys. */
function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** Stable per-day key (`YYYY-MM-DD` in UTC). */
export function dayKey(date: Date): string {
  return `${date.getUTCFullYear()}-${pad2(date.getUTCMonth() + 1)}-${pad2(date.getUTCDate())}`;
}

/** Today's day key in UTC. */
export function todayKey(now: Date = new Date()): string {
  return dayKey(now);
}

/** Returns the next/prev month in the calendar. */
export function shiftMonth(m: CalendarMonth, delta: number): CalendarMonth {
  const total = m.year * 12 + (m.month - 1) + delta;
  const year = Math.floor(total / 12);
  // JS modulo with negative numbers can yield negative results; the +12 %12
  // dance keeps `month` in [1, 12].
  const month = ((total % 12) + 12) % 12 + 1;
  return { year, month };
}

/** Current calendar month in UTC. */
export function currentMonth(now: Date = new Date()): CalendarMonth {
  return { year: now.getUTCFullYear(), month: now.getUTCMonth() + 1 };
}

/** Build the 6×7 day grid for a given month. Prev/next month days fill
 *  the leading and trailing edges so the grid is always 42 cells, matching
 *  every commonly-used month-calendar layout. */
export function buildMonthGrid(m: CalendarMonth): CalendarDayCell[] {
  const firstOfMonth = new Date(Date.UTC(m.year, m.month - 1, 1));
  const startWeekday = firstOfMonth.getUTCDay(); // 0 = Sunday
  // Step back `startWeekday` days so the grid begins on a Sunday containing
  // (or preceding) the first of the month.
  const gridStart = new Date(firstOfMonth);
  gridStart.setUTCDate(firstOfMonth.getUTCDate() - startWeekday);

  const cells: CalendarDayCell[] = [];
  for (let i = 0; i < WEEKS_IN_GRID * DAYS_PER_WEEK; i++) {
    const date = new Date(gridStart);
    date.setUTCDate(gridStart.getUTCDate() + i);
    const inMonth = date.getUTCMonth() === m.month - 1
      && date.getUTCFullYear() === m.year;
    cells.push({ date, inMonth, key: dayKey(date) });
  }
  return cells;
}

/** Boundaries [from, to] (inclusive) for the requested calendar month in
 *  UTC, RFC 3339 format. The runtime accepts only RFC 3339 timestamps. */
export function monthRangeIso(m: CalendarMonth): { from: string; to: string } {
  const fromDate = new Date(Date.UTC(m.year, m.month - 1, 1, 0, 0, 0));
  // Last second of month: first of next month minus one second.
  const nextMonth = shiftMonth(m, 1);
  const nextStart = new Date(Date.UTC(nextMonth.year, nextMonth.month - 1, 1));
  const toDate = new Date(nextStart.getTime() - 1000);
  return { from: fromDate.toISOString(), to: toDate.toISOString() };
}

/** Group timeline entries by `YYYY-MM-DD` UTC. Empty timeline → empty map. */
export function groupEntriesByDay(
  entries: readonly CronTimelineEntry[],
): Map<string, CronTimelineEntry[]> {
  const map = new Map<string, CronTimelineEntry[]>();
  for (const entry of entries) {
    const date = new Date(entry.ts);
    if (Number.isNaN(date.getTime())) continue;
    const key = dayKey(date);
    let bucket = map.get(key);
    if (!bucket) {
      bucket = [];
      map.set(key, bucket);
    }
    bucket.push(entry);
  }
  // Sort each bucket chronologically. Entries arrive sorted from the runtime,
  // but grouping by day preserves overall order rather than per-day order
  // — re-sort to make the day panel rendering deterministic.
  for (const bucket of map.values()) {
    bucket.sort((a, b) => a.ts.localeCompare(b.ts));
  }
  return map;
}

/** Time-of-day display for an entry, e.g. "09:30Z". UTC. */
export function formatEntryTime(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return `${pad2(d.getUTCHours())}:${pad2(d.getUTCMinutes())}Z`;
}
