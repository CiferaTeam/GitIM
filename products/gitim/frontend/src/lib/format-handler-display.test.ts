import { describe, expect, it } from "vitest";
import { formatHandlerLabel, resolveDisplayName } from "./format-handler-display";

const dir = new Map<string, string>([
  ["alice", "Alice Chen"],
  ["bob", "bob"], // display_name equals handler — degenerate
]);

describe("resolveDisplayName", () => {
  it("returns the display name when known and distinct", () => {
    expect(resolveDisplayName("alice", dir)).toBe("Alice Chen");
  });

  it("returns undefined for an unknown handler (fall back to bare handler)", () => {
    expect(resolveDisplayName("ghost", dir)).toBeUndefined();
  });

  it("returns undefined when display_name equals handler (avoid 'bob @bob')", () => {
    expect(resolveDisplayName("bob", dir)).toBeUndefined();
  });

  it("returns undefined against an empty directory", () => {
    expect(resolveDisplayName("alice", new Map())).toBeUndefined();
  });
});

describe("formatHandlerLabel", () => {
  it("combines name and handle when known", () => {
    expect(formatHandlerLabel("alice", dir)).toBe("Alice Chen (@alice)");
  });

  it("falls back to bare @handle when unknown", () => {
    expect(formatHandlerLabel("ghost", dir)).toBe("@ghost");
  });

  it("falls back to bare @handle when name equals handle", () => {
    expect(formatHandlerLabel("bob", dir)).toBe("@bob");
  });
});
