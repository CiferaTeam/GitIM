export type DisplayTimezone = "utc+8" | "utc";

export interface DisplayTimezoneOption {
  value: DisplayTimezone;
  label: string;
  description: string;
  offsetMinutes: number;
  shortSuffix: string;
}

export const DEFAULT_DISPLAY_TIMEZONE: DisplayTimezone = "utc+8";

export const DISPLAY_TIMEZONES: DisplayTimezoneOption[] = [
  {
    value: "utc+8",
    label: "UTC+8",
    description: "Beijing / Singapore",
    offsetMinutes: 8 * 60,
    shortSuffix: "+8",
  },
  {
    value: "utc",
    label: "UTC",
    description: "Coordinated Universal Time",
    offsetMinutes: 0,
    shortSuffix: "Z",
  },
];

export interface ZonedDateParts {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
}

const MS_PER_MINUTE = 60 * 1000;

export function displayTimezoneOption(
  timezone: DisplayTimezone,
): DisplayTimezoneOption {
  return (
    DISPLAY_TIMEZONES.find((option) => option.value === timezone) ??
    DISPLAY_TIMEZONES[0]
  );
}

export function normalizeDisplayTimezone(value: unknown): DisplayTimezone {
  return value === "utc" || value === "utc+8" ? value : DEFAULT_DISPLAY_TIMEZONE;
}

export function parseUtcTimestamp(ts: string): Date | null {
  const compactOrStem =
    ts.match(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/) ??
    ts.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2})-(\d{2})-(\d{2})Z$/);
  if (compactOrStem) {
    const [, y, mo, d, h, mi, s] = compactOrStem;
    return new Date(
      Date.UTC(
        Number(y),
        Number(mo) - 1,
        Number(d),
        Number(h),
        Number(mi),
        Number(s),
      ),
    );
  }

  const parsed = new Date(ts);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

export function zonedDateParts(
  date: Date,
  timezone: DisplayTimezone,
): ZonedDateParts {
  const option = displayTimezoneOption(timezone);
  const shifted = new Date(date.getTime() + option.offsetMinutes * MS_PER_MINUTE);
  return {
    year: shifted.getUTCFullYear(),
    month: shifted.getUTCMonth() + 1,
    day: shifted.getUTCDate(),
    hour: shifted.getUTCHours(),
    minute: shifted.getUTCMinutes(),
    second: shifted.getUTCSeconds(),
  };
}

export function utcDateFromZonedParts(
  parts: Pick<ZonedDateParts, "year" | "month" | "day"> &
    Partial<Pick<ZonedDateParts, "hour" | "minute" | "second">>,
  timezone: DisplayTimezone,
): Date {
  const option = displayTimezoneOption(timezone);
  return new Date(
    Date.UTC(
      parts.year,
      parts.month - 1,
      parts.day,
      parts.hour ?? 0,
      parts.minute ?? 0,
      parts.second ?? 0,
    ) -
      option.offsetMinutes * MS_PER_MINUTE,
  );
}

export function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

export function formatTimeOfDay(
  ts: string,
  timezone: DisplayTimezone,
  options: {
    seconds?: boolean;
    suffix?: boolean;
    fallback?: string;
  } = {},
): string {
  const date = parseUtcTimestamp(ts);
  if (!date) return options.fallback ?? ts;
  const parts = zonedDateParts(date, timezone);
  const time = options.seconds
    ? `${pad2(parts.hour)}:${pad2(parts.minute)}:${pad2(parts.second)}`
    : `${pad2(parts.hour)}:${pad2(parts.minute)}`;
  return options.suffix
    ? `${time}${displayTimezoneOption(timezone).shortSuffix}`
    : time;
}

export function formatDateOnly(
  ts: string,
  timezone: DisplayTimezone,
  fallback = ts,
): string {
  const date = parseUtcTimestamp(ts);
  if (!date) return fallback;
  const parts = zonedDateParts(date, timezone);
  return `${parts.year}-${pad2(parts.month)}-${pad2(parts.day)}`;
}

export function formatDateTime(
  ts: string,
  timezone: DisplayTimezone,
  options: {
    seconds?: boolean;
    suffix?: boolean;
    fallback?: string;
  } = {},
): string {
  const date = parseUtcTimestamp(ts);
  if (!date) return options.fallback ?? ts;
  const parts = zonedDateParts(date, timezone);
  const time = options.seconds
    ? `${pad2(parts.hour)}:${pad2(parts.minute)}:${pad2(parts.second)}`
    : `${pad2(parts.hour)}:${pad2(parts.minute)}`;
  const suffix = options.suffix
    ? ` ${displayTimezoneOption(timezone).label}`
    : "";
  return `${parts.year}-${pad2(parts.month)}-${pad2(parts.day)} ${time}${suffix}`;
}

export function formatShortDate(
  ts: string,
  timezone: DisplayTimezone,
  fallback = ts,
): string {
  const date = parseUtcTimestamp(ts);
  if (!date) return fallback;
  const parts = zonedDateParts(date, timezone);
  return `${pad2(parts.month)}/${pad2(parts.day)}`;
}

export function formatRelativeTimestamp(
  ts: string,
  timezone: DisplayTimezone,
): string {
  const date = parseUtcTimestamp(ts);
  if (!date) return ts;
  const diffSec = Math.floor((Date.now() - date.getTime()) / 1000);
  if (diffSec < 60) return "just now";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
  if (diffSec < 30 * 86400) return `${Math.floor(diffSec / 86400)}d ago`;
  return formatShortDate(ts, timezone);
}
