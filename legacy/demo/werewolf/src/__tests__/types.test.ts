import { describe, it, expect } from "vitest";
import { Role, dmChannel } from "../types.js";

describe("dmChannel", () => {
  it("sorts handlers lexicographically", () => {
    expect(dmChannel("bob", "alice")).toBe("dm:alice,bob");
    expect(dmChannel("alice", "bob")).toBe("dm:alice,bob");
  });

  it("handles self-DM", () => {
    expect(dmChannel("alice", "alice")).toBe("dm:alice,alice");
  });

  it("handles god DM", () => {
    expect(dmChannel("alice", "god")).toBe("dm:alice,god");
    expect(dmChannel("god", "alice")).toBe("dm:alice,god");
  });
});

describe("Role enum", () => {
  it("has all expected roles", () => {
    expect(Object.values(Role)).toContain("wolf");
    expect(Object.values(Role)).toContain("seer");
    expect(Object.values(Role)).toContain("witch");
    expect(Object.values(Role)).toContain("hunter");
    expect(Object.values(Role)).toContain("villager");
    expect(Object.values(Role)).toContain("god");
  });
});
