// Calendar grid maths — kept pure so the day-bucketing logic is testable
// without rendering. Timeline entries stay as UTC instants from the runtime;
// grouping and labels project those instants into the selected display
// timezone.

import type { CronTimelineEntry } from "@/lib/types";
import {
  DEFAULT_DISPLAY_TIMEZONE,
  displayTimezoneOption,
  pad2,
  utcDateFromZonedParts,
  zonedDateParts,
  type DisplayTimezone,
} from "@/lib/timezone";

export interface CalendarMonth {
  /** Display-zone year (e.g. 2026). */
  year: number;
  /** Display-zone month, 1-12. */
  month: number;
}

export interface CalendarDayCell {
  /** Civil date for this cell; UTC fields hold the display-zone date parts. */
  date: Date;
  /** `true` when the cell is part of the focused month
   *  (`false` for the prev/next month bleed-in cells at grid edges). */
  inMonth: boolean;
  /** Stable key formed from the display-zone date (`YYYY-MM-DD`). */
  key: string;
}

/** First day-of-week for the grid header. The week starts on Sunday to match
 *  the Western convention; agents using gitim are global and we're not
 *  bidi-localizing today. v2 can revisit if a locale switch lands. */
const DAYS_PER_WEEK = 7;
const WEEKS_IN_GRID = 6;

export const WEEKDAY_LABELS = ["S", "M", "T", "W", "T", "F", "S"] as const;
const MONTH_NAMES_EN = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
] as const;

/** Month label for the header, e.g. "May 2026". */
export function formatMonthLabel(m: CalendarMonth): string {
  const monthName = MONTH_NAMES_EN[m.month - 1] ?? "";
  return `${monthName} ${m.year}`;
}

/** Stable per-day key (`YYYY-MM-DD` in the display timezone). */
export function dayKey(
  date: Date,
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): string {
  const parts = zonedDateParts(date, timezone);
  return `${parts.year}-${pad2(parts.month)}-${pad2(parts.day)}`;
}

/** Today's day key in the display timezone. */
export function todayKey(
  now: Date = new Date(),
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): string {
  return dayKey(now, timezone);
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

/** Current calendar month in the display timezone. */
export function currentMonth(
  now: Date = new Date(),
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): CalendarMonth {
  const parts = zonedDateParts(now, timezone);
  return { year: parts.year, month: parts.month };
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
    cells.push({ date, inMonth, key: dayKey(date, "utc") });
  }
  return cells;
}

/** Boundaries [from, to] (inclusive) for the requested calendar month in
 *  the display timezone, converted to RFC 3339 UTC for the runtime. */
export function monthRangeIso(
  m: CalendarMonth,
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): { from: string; to: string } {
  const fromDate = utcDateFromZonedParts(
    { year: m.year, month: m.month, day: 1 },
    timezone,
  );
  // Last second of month: first of next month minus one second.
  const nextMonth = shiftMonth(m, 1);
  const nextStart = utcDateFromZonedParts(
    { year: nextMonth.year, month: nextMonth.month, day: 1 },
    timezone,
  );
  const toDate = new Date(nextStart.getTime() - 1000);
  return { from: fromDate.toISOString(), to: toDate.toISOString() };
}

/** Group timeline entries by `YYYY-MM-DD` in the display timezone. */
export function groupEntriesByDay(
  entries: readonly CronTimelineEntry[],
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): Map<string, CronTimelineEntry[]> {
  const map = new Map<string, CronTimelineEntry[]>();
  for (const entry of entries) {
    const date = new Date(entry.ts);
    if (Number.isNaN(date.getTime())) continue;
    const key = dayKey(date, timezone);
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

/** Time-of-day display for an entry, e.g. "17:30+8". */
export function formatEntryTime(
  ts: string,
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const parts = zonedDateParts(d, timezone);
  return `${pad2(parts.hour)}:${pad2(parts.minute)}${displayTimezoneOption(timezone).shortSuffix}`;
}

/** One hour-of-day bucket inside a single day's panel. `hourKey` is the
 *  zero-padded display-zone hour (`"00"` through `"23"`); `label` is the user-
 *  visible header text. Empty hours never appear in the returned list. */
export interface HourGroup {
  hourKey: string;
  label: string;
  entries: CronTimelineEntry[];
}

/** Bucket timeline entries by display-zone hour-of-day, preserving input order
 *  inside each bucket. Hours with no entries are absent (we don't pad a
 *  24-row grid — empty rows are pure visual noise). Entries with an
 *  unparseable `ts` are silently dropped, mirroring `groupEntriesByDay`.
 *  Caller stays responsible for keeping the input scoped to a single
 *  display day; this function only groups by hour-of-day, so a multi-day
 *  input would collapse 13:00 from different days into one bucket. */
export function groupEntriesByHour(
  entries: readonly CronTimelineEntry[],
  timezone: DisplayTimezone = DEFAULT_DISPLAY_TIMEZONE,
): HourGroup[] {
  const buckets = new Map<string, CronTimelineEntry[]>();
  for (const entry of entries) {
    const d = new Date(entry.ts);
    if (Number.isNaN(d.getTime())) continue;
    const hourKey = pad2(zonedDateParts(d, timezone).hour);
    let list = buckets.get(hourKey);
    if (!list) {
      list = [];
      buckets.set(hourKey, list);
    }
    list.push(entry);
  }
  const out: HourGroup[] = [];
  const suffix = displayTimezoneOption(timezone).shortSuffix;
  for (const [hourKey, list] of buckets) {
    out.push({ hourKey, label: `${hourKey}:00${suffix}`, entries: list });
  }
  out.sort((a, b) => a.hourKey.localeCompare(b.hourKey));
  return out;
}

/** Count of distinct `cron_name` values across the entry list. Used by
 *  the day cell's overflow tooltip and aria-label to convey "this stack
 *  of N entries is N repeats of M crons". */
export function distinctCronCount(
  entries: readonly CronTimelineEntry[],
): number {
  const names = new Set<string>();
  for (const e of entries) names.add(e.cron_name);
  return names.size;
}
