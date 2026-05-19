import { describe, expect, it } from "vitest";
import { expandAllMentions } from "./expand-all-mentions";

describe("expandAllMentions", () => {
  it("expands a standalone @all token to protocol mentions", () => {
    expect(expandAllMentions("heads up @all", ["lewis", "alice", "bob"])).toBe(
      "heads up <@lewis> <@alice> <@bob>",
    );
  });

  it("expands the protocol-shaped <@all> token", () => {
    expect(expandAllMentions("<@all> please review", ["alice", "bob"])).toBe(
      "<@alice> <@bob> please review",
    );
  });

  it("deduplicates recipients and ignores non-standalone @all text", () => {
    expect(
      expandAllMentions("mail me at ops@all.example or ping @all", [
        "alice",
        "bob",
        "alice",
      ]),
    ).toBe("mail me at ops@all.example or ping <@alice> <@bob>");
  });
});
