import { describe, expect, it } from "vitest";
import { buildMentionCandidates } from "./mention-candidates";

describe("buildMentionCandidates", () => {
  it("puts the @all pseudo-candidate first when it is available", () => {
    expect(
      buildMentionCandidates({
        users: ["flame4", "all", "cfo"],
        agents: ["robin", "cfo"],
        includeAll: true,
      }),
    ).toEqual(["all", "flame4", "cfo", "robin"]);
  });

  it("does not synthesize @all outside channel scopes", () => {
    expect(
      buildMentionCandidates({
        users: ["flame4"],
        agents: ["robin"],
        includeAll: false,
      }),
    ).toEqual(["flame4", "robin"]);
  });
});
