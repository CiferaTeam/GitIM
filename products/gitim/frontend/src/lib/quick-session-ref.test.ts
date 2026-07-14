import { afterEach, describe, expect, it, vi } from "vitest";

import {
  extractQuickSessionRefs,
  formatQuickSessionRef,
  generateQuickSessionId,
  generateQuickSessionRequestId,
  parseQuickSessionRef,
} from "./quick-session-ref";

const SESSION_ID = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";

describe("Quick Session refs", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("formats and parses the stable ref with an optional line", () => {
    expect(formatQuickSessionRef(SESSION_ID)).toBe(`session:${SESSION_ID}`);
    expect(formatQuickSessionRef(SESSION_ID, 7)).toBe(
      `session:${SESSION_ID}:L000007`,
    );
    expect(parseQuickSessionRef(`session:${SESSION_ID}:L000007`)).toEqual({
      raw: `session:${SESSION_ID}:L000007`,
      sessionId: SESSION_ID,
      line: 7,
    });
  });

  it("recognizes refs only at protocol text boundaries", () => {
    expect(extractQuickSessionRefs(`See session:${SESSION_ID} now`)).toEqual([
      {
        raw: `session:${SESSION_ID}`,
        sessionId: SESSION_ID,
      },
    ]);
    expect(
      extractQuickSessionRefs(`(session:${SESSION_ID}:L000001)`),
    ).toEqual([
      {
        raw: `session:${SESSION_ID}:L000001`,
        sessionId: SESSION_ID,
        line: 1,
      },
    ]);

    for (const invalid of [
      `xsession:${SESSION_ID}`,
      `session:${SESSION_ID}x`,
      `session:${SESSION_ID}:L1`,
      `session:${SESSION_ID}:L000001x`,
      `界session:${SESSION_ID}`,
      `session:${SESSION_ID}界`,
      `/session:${SESSION_ID}`,
      `session:${SESSION_ID}/discussion.thread`,
      `session:${SESSION_ID}:L9007199254740992`,
    ]) {
      expect(extractQuickSessionRefs(invalid), invalid).toEqual([]);
    }
  });

  it("rejects invalid ids and line numbers before formatting", () => {
    expect(() => formatQuickSessionRef("qs-bad")).toThrow(
      "Invalid Quick Session id",
    );
    expect(() => formatQuickSessionRef(SESSION_ID, 0)).toThrow(
      "Invalid Quick Session line",
    );
    expect(parseQuickSessionRef(`session:${SESSION_ID}:L000000`)).toBeNull();
  });

  it("generates crypto-backed Crockford ULIDs for sessions and requests", () => {
    const getRandomValues = vi.fn(<T extends ArrayBufferView>(buffer: T): T => {
      new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength).fill(31);
      return buffer;
    });
    vi.stubGlobal("crypto", { getRandomValues });

    expect(generateQuickSessionId(1_720_000_000_000)).toMatch(
      /^qs-[0-9A-HJKMNP-TV-Z]{26}$/,
    );
    expect(generateQuickSessionRequestId(1_720_000_000_001)).toMatch(
      /^[0-9A-HJKMNP-TV-Z]{26}$/,
    );
    expect(getRandomValues).toHaveBeenCalledTimes(2);
  });
});
