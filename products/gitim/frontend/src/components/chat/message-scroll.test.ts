import { describe, expect, it } from "vitest";
import { decideTimelineScroll } from "./message-scroll";

describe("decideTimelineScroll", () => {
  it("restores the persisted anchor when an empty selected scope receives its loaded page", () => {
    const decision = decideTimelineScroll({
      previous: {
        scopeKey: "leaders",
        firstLine: undefined,
        length: 0,
        scrollHeight: 0,
      },
      next: {
        scopeKey: "leaders",
        firstLine: 37,
        length: 29,
        scrollHeight: 1200,
      },
      scrollTop: 0,
      clientHeight: 640,
      pendingScrollLine: null,
      restoreAnchor: { line: 37, offsetPx: 24 },
      lastMessageIsOutbound: false,
    });

    expect(decision).toEqual({ kind: "anchor", line: 37, offsetPx: 24 });
  });

  it("does not pull a restored historical viewport to the bottom on inbound append", () => {
    const decision = decideTimelineScroll({
      previous: {
        scopeKey: "leaders",
        firstLine: 43,
        length: 23,
        scrollHeight: 500,
      },
      next: {
        scopeKey: "leaders",
        firstLine: 43,
        length: 24,
        scrollHeight: 560,
      },
      scrollTop: 0,
      clientHeight: 640,
      pendingScrollLine: null,
      restoreAnchor: null,
      suppressAutoBottom: true,
      lastMessageIsOutbound: false,
    });

    expect(decision).toEqual({ kind: "none" });
  });
});
