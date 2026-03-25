import { describe, it, expect } from "vitest";
import { getVisibleChannels, Role } from "../types.js";

describe("getVisibleChannels", () => {
  const wolves = ["dave", "eve"];

  it("wolf sees general + wolves + dm(god) + dm(self)", () => {
    const channels = getVisibleChannels("dave", Role.Wolf, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("wolves");
    expect(channels).toContain("dm:dave,god");
    expect(channels).toContain("dm:dave,dave");
    expect(channels).not.toContain("dm:alice,god");
  });

  it("seer sees general + dm(god) + dm(self)", () => {
    const channels = getVisibleChannels("alice", Role.Seer, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("dm:alice,god");
    expect(channels).toContain("dm:alice,alice");
    expect(channels).not.toContain("wolves");
  });

  it("villager sees general + dm(self) only", () => {
    const channels = getVisibleChannels("bob", Role.Villager, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("dm:bob,bob");
    expect(channels).not.toContain("wolves");
    expect(channels).not.toContain("dm:bob,god");
  });

  it("witch sees general + dm(god) + dm(self)", () => {
    const channels = getVisibleChannels("charlie", Role.Witch, wolves);
    expect(channels).toContain("general");
    expect(channels).toContain("dm:charlie,god");
    expect(channels).toContain("dm:charlie,charlie");
    expect(channels).not.toContain("wolves");
  });

  it("god sees everything", () => {
    const channels = getVisibleChannels("god", Role.God, wolves, ["alice", "bob", "charlie", "dave", "eve"]);
    expect(channels).toContain("general");
    expect(channels).toContain("wolves");
    expect(channels.length).toBeGreaterThan(3);
  });
});
