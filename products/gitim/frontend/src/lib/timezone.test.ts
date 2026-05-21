import { describe, expect, it, vi } from "vitest";
import {
  formatDateTime,
  formatRelativeTimestamp,
  formatTimeOfDay,
  parseUtcTimestamp,
} from "./timezone";

describe("timezone formatting", () => {
  it("formats compact gitim timestamps in UTC+8 by default helpers", () => {
    expect(formatTimeOfDay("20260511T120000Z", "utc+8")).toBe("20:00");
    expect(formatTimeOfDay("20260511T120000Z", "utc")).toBe("12:00");
  });

  it("formats ISO timestamps across a UTC day boundary", () => {
    expect(formatDateTime("2026-05-11T23:30:00Z", "utc+8")).toBe(
      "2026-05-12 07:30",
    );
    expect(formatDateTime("2026-05-11T23:30:00Z", "utc")).toBe(
      "2026-05-11 23:30",
    );
  });

  it("parses compact timestamps as UTC instants", () => {
    expect(parseUtcTimestamp("20260511T120000Z")?.toISOString()).toBe(
      "2026-05-11T12:00:00.000Z",
    );
    expect(parseUtcTimestamp("2026-05-11T12-00-00Z")?.toISOString()).toBe(
      "2026-05-11T12:00:00.000Z",
    );
  });

  it("keeps relative durations based on the original instant", () => {
    const now = vi
      .spyOn(Date, "now")
      .mockReturnValue(Date.parse("2026-05-11T12:05:00Z"));
    try {
      expect(formatRelativeTimestamp("20260511T120000Z", "utc+8")).toBe(
        "5m ago",
      );
    } finally {
      now.mockRestore();
    }
  });
});
