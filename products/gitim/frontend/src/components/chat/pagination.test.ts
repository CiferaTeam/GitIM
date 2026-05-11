import { describe, expect, it } from "vitest";
import { MESSAGES_PAGE_SIZE, computeLoadOlderSince } from "./pagination";

describe("computeLoadOlderSince", () => {
  it("returns skip when there are no messages on screen", () => {
    expect(computeLoadOlderSince(undefined, MESSAGES_PAGE_SIZE)).toEqual({
      kind: "skip",
      reason: "no_messages",
    });
  });

  it("returns skip when the oldest visible line is already line 1 (top of channel)", () => {
    expect(computeLoadOlderSince(1, MESSAGES_PAGE_SIZE)).toEqual({
      kind: "skip",
      reason: "at_top",
    });
  });

  it("returns since = oldest - pageSize - 1 in the standard paging case", () => {
    // oldest=100, pageSize=50 → want [50..99]
    // since = 100 - 50 - 1 = 49 → daemon retains line>49 → [50..],
    // head-truncate(50) → [50..99]. ✓
    expect(computeLoadOlderSince(100, 50)).toEqual({
      kind: "fetch",
      since: 49,
    });
  });

  it("clamps since at 0 when the page would extend past line 1", () => {
    // oldest=51, pageSize=50 → since would be 0 (covers [1..50] cleanly).
    expect(computeLoadOlderSince(51, 50)).toEqual({
      kind: "fetch",
      since: 0,
    });
    // oldest=2, pageSize=50 → since clamped to 0, daemon returns just line 1
    // (single entry < pageSize, caller sets hasMoreHistory=false).
    expect(computeLoadOlderSince(2, 50)).toEqual({
      kind: "fetch",
      since: 0,
    });
  });

  it("exposes MESSAGES_PAGE_SIZE as 50 (single source of truth)", () => {
    expect(MESSAGES_PAGE_SIZE).toBe(50);
  });
});
