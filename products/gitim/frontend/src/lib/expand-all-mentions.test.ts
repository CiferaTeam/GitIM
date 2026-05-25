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

  it("converts protocol mentions outside the channel members to profile links", () => {
    expect(
      expandAllMentions(
        "cc <@alice> <@carol> and @all",
        ["alice", "bob"],
        { referenceNonRecipients: true },
      ),
    ).toBe("cc <@alice> <~carol> and <@alice> <@bob>");
  });

  it("excludes self from @all expansion", () => {
    expect(
      expandAllMentions("@all heads up", ["lewis", "alice", "bob"], {
        excludeSelf: "lewis",
      }),
    ).toBe("<@alice> <@bob> heads up");
  });

  it("excludes self from <@all> protocol expansion", () => {
    expect(
      expandAllMentions("<@all> please review", ["lewis", "alice", "bob"], {
        excludeSelf: "lewis",
      }),
    ).toBe("<@alice> <@bob> please review");
  });

  it("leaves body unchanged when @all expands to only self", () => {
    expect(
      expandAllMentions("@all hi", ["lewis"], { excludeSelf: "lewis" }),
    ).toBe("@all hi");
  });
});
