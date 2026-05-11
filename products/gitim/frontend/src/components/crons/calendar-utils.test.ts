import { describe, expect, it } from "vitest";
import {
  buildMonthGrid,
  currentMonth,
  dayKey,
  formatEntryTime,
  formatMonthLabel,
  groupEntriesByDay,
  monthRangeIso,
  shiftMonth,
} from "./calendar-utils";
import type { CronTimelineEntry } from "@/lib/types";

describe("calendar-utils", () => {
  it("currentMonth reads from UTC", () => {
    const m = currentMonth(new Date("2026-05-11T03:00:00Z"));
    expect(m).toEqual({ year: 2026, month: 5 });
  });

  it("formatMonthLabel renders English long month plus year", () => {
    expect(formatMonthLabel({ year: 2026, month: 5 })).toBe("May 2026");
    expect(formatMonthLabel({ year: 2025, month: 12 })).toBe("December 2025");
  });

  it("shiftMonth wraps across year boundaries", () => {
    expect(shiftMonth({ year: 2026, month: 1 }, -1)).toEqual({ year: 2025, month: 12 });
    expect(shiftMonth({ year: 2025, month: 12 }, 1)).toEqual({ year: 2026, month: 1 });
    expect(shiftMonth({ year: 2026, month: 5 }, 7)).toEqual({ year: 2026, month: 12 });
    expect(shiftMonth({ year: 2026, month: 5 }, 8)).toEqual({ year: 2027, month: 1 });
  });

  it("buildMonthGrid emits 42 cells with correct inMonth flags", () => {
    const grid = buildMonthGrid({ year: 2026, month: 5 });
    expect(grid).toHaveLength(42);

    // May 2026 starts on Friday (UTC), so the first row has Sun-Thu in April.
    const firstFridayCell = grid[5];
    expect(firstFridayCell.date.toISOString().slice(0, 10)).toBe("2026-05-01");
    expect(firstFridayCell.inMonth).toBe(true);

    const lastCell = grid[grid.length - 1];
    expect(lastCell.inMonth).toBe(false);
  });

  it("monthRangeIso spans the full UTC month", () => {
    const { from, to } = monthRangeIso({ year: 2026, month: 5 });
    expect(from).toBe("2026-05-01T00:00:00.000Z");
    // End is "1 second before next month start" = 23:59:59 on May 31.
    expect(to).toBe("2026-05-31T23:59:59.000Z");
  });

  it("dayKey is the UTC YYYY-MM-DD form", () => {
    expect(dayKey(new Date("2026-05-11T23:30:00Z"))).toBe("2026-05-11");
  });

  it("groupEntriesByDay buckets entries by UTC day and sorts within day", () => {
    const entries: CronTimelineEntry[] = [
      { ts: "2026-05-11T09:30:00Z", kind: "past", cron_name: "daily" },
      { ts: "2026-05-11T09:00:00Z", kind: "past", cron_name: "daily" },
      { ts: "2026-05-12T09:00:00Z", kind: "future", cron_name: "daily" },
    ];
    const grouped = groupEntriesByDay(entries);
    expect(grouped.size).toBe(2);
    const may11 = grouped.get("2026-05-11")!;
    expect(may11.map((e) => e.ts)).toEqual([
      "2026-05-11T09:00:00Z",
      "2026-05-11T09:30:00Z",
    ]);
    expect(grouped.get("2026-05-12")!.map((e) => e.kind)).toEqual(["future"]);
  });

  it("groupEntriesByDay drops entries with unparseable ts", () => {
    const grouped = groupEntriesByDay([
      { ts: "bogus", kind: "past", cron_name: "x" },
    ]);
    expect(grouped.size).toBe(0);
  });

  it("formatEntryTime returns HH:MMZ", () => {
    expect(formatEntryTime("2026-05-11T09:30:00Z")).toBe("09:30Z");
    expect(formatEntryTime("2026-05-11T23:00:00Z")).toBe("23:00Z");
  });
});
