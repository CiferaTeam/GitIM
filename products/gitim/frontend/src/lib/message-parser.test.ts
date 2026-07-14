import { describe, expect, it } from "vitest";
import { parseAssetRef } from "./asset-ref";
import { parseMessageBody, type Fragment } from "./message-parser";

const ASSET_REF =
  "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=fleet-assets.png&type=image%2Fpng&size=184203&width=1600&height=900>";

function collectTypes(fragments: Fragment[]): string[] {
  return fragments.map((f) => f.type);
}

function findFirst<T extends Fragment["type"]>(
  fragments: Fragment[],
  type: T,
): Extract<Fragment, { type: T }> | undefined {
  return fragments.find((f) => f.type === type) as Extract<Fragment, { type: T }> | undefined;
}

describe("parseMessageBody", () => {
  it("parses plain text", () => {
    const frags = parseMessageBody("hello world");
    expect(frags).toHaveLength(1);
    expect(frags[0]).toEqual({ type: "text", content: "hello world" });
  });

  it("parses mention", () => {
    const frags = parseMessageBody("<@alice>");
    expect(findFirst(frags, "mention")).toEqual({ type: "mention", handler: "alice" });
  });

  it("parses channel link", () => {
    const frags = parseMessageBody("see <#general>");
    expect(findFirst(frags, "channel-link")).toEqual({ type: "channel-link", channel: "general" });
  });

  it("parses message link", () => {
    const frags = parseMessageBody("check <#dev:L000042>");
    expect(findFirst(frags, "message-link")).toEqual({
      type: "message-link",
      channel: "dev",
      line: 42,
    });
  });

  it("parses user profile link", () => {
    const frags = parseMessageBody("contact <~bob>");
    expect(findFirst(frags, "user-profile")).toEqual({
      type: "user-profile",
      handler: "bob",
    });
  });

  it("parses softlink bare", () => {
    const frags = parseMessageBody("visit <!https://example.com>");
    expect(findFirst(frags, "external-link")).toEqual({
      type: "external-link",
      url: "https://example.com",
    });
  });

  it("parses softlink with title", () => {
    const frags = parseMessageBody("see <!https://example.com|Example Site>");
    expect(findFirst(frags, "external-link")).toEqual({
      type: "external-link",
      url: "https://example.com",
      title: "Example Site",
    });
  });

  it("parses inline code", () => {
    const frags = parseMessageBody("use `foo`");
    expect(findFirst(frags, "inline-code")).toEqual({ type: "inline-code", code: "foo" });
  });

  it("parses bold", () => {
    const frags = parseMessageBody("**bold**");
    expect(findFirst(frags, "bold")).toEqual({ type: "bold", content: "bold" });
  });

  it("parses italic", () => {
    const frags = parseMessageBody("*italic*");
    expect(findFirst(frags, "italic")).toEqual({ type: "italic", content: "italic" });
  });

  it("parses multiple links in one message", () => {
    const frags = parseMessageBody("<@alice> <#general> and <!https://x.com>");
    expect(collectTypes(frags)).toEqual(["mention", "text", "channel-link", "text", "external-link"]);
  });

  describe("asset references", () => {
    it("parses a canonical asset as a single asset fragment", () => {
      expect(parseMessageBody(ASSET_REF)).toEqual([
        { type: "asset", asset: parseAssetRef(ASSET_REF) },
      ]);
    });

    it("preserves assets inside inline code", () => {
      expect(parseMessageBody(`\`${ASSET_REF}\``)).toEqual([
        { type: "inline-code", code: ASSET_REF },
      ]);
    });

    it("preserves assets inside fenced code blocks", () => {
      expect(parseMessageBody(`\`\`\`text\n${ASSET_REF}\n\`\`\``)).toEqual([
        { type: "code-block", language: "text", code: `${ASSET_REF}\n` },
      ]);
    });

    it("parses assets among text and existing inline links", () => {
      expect(collectTypes(parseMessageBody(`See <@alice> ${ASSET_REF} <#general/abc123> <!https://gitim.io>.`))).toEqual([
        "text",
        "mention",
        "text",
        "asset",
        "text",
        "card-link",
        "text",
        "external-link",
        "text",
      ]);
    });

    it("emits invalid asset references as exact selectable plain text", () => {
      const invalid = ASSET_REF.replace("size=184203", "size=0184203");
      expect(parseMessageBody(invalid)).toEqual([{ type: "text", content: invalid }]);
    });

    it("parses multiple canonical assets", () => {
      expect(collectTypes(parseMessageBody(`${ASSET_REF} and ${ASSET_REF}`))).toEqual([
        "asset",
        "text",
        "asset",
      ]);
    });

    it.each([
      ["channel", "<#general>", "channel-link"],
      ["user", "<~bob>", "user-profile"],
      ["mention", "<@alice>", "mention"],
      ["soft link", "<!https://gitim.io>", "external-link"],
    ])("does not swallow a nested %s link in a malformed asset opener", (_label, link, type) => {
      const fragments = parseMessageBody(`<^not-an-asset ${link}>`);
      expect(collectTypes(fragments)).toContain(type);
      expect(
        fragments
          .filter((fragment): fragment is Extract<Fragment, { type: "text" }> => fragment.type === "text")
          .map((fragment) => fragment.content)
          .join(""),
      ).toBe("<^not-an-asset >");
    });

    it("recovers valid links after a long malformed asset opener", () => {
      const fragments = parseMessageBody(
        `<^${"x".repeat(2_000)} <#general>> tail <@alice>`,
      );
      expect(collectTypes(fragments)).toEqual([
        "text",
        "channel-link",
        "text",
        "mention",
      ]);
      expect(findFirst(fragments, "channel-link")).toEqual({
        type: "channel-link",
        channel: "general",
      });
    });
  });

  describe("card-link", () => {
    it("parses bare card link", () => {
      const frags = parseMessageBody("open <#general/abc123>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({ type: "card-link", channel: "general", cardId: "abc123" });
    });

    it("parses card link with title", () => {
      const frags = parseMessageBody("see <#general/abc123|Token Rotation>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "abc123",
        label: "Token Rotation",
      });
    });

    it("parses card link with empty title", () => {
      const frags = parseMessageBody("see <#general/abc123|>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "abc123",
        label: "",
      });
    });

    it("parses card discussion line link", () => {
      const frags = parseMessageBody("see <#general/20260520-035646-7cf:L000004>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "20260520-035646-7cf",
        line: 4,
      });
    });

    it("parses legacy short card discussion line link", () => {
      const frags = parseMessageBody("see <#general/20260520-035646-7cf:L22>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "20260520-035646-7cf",
        line: 22,
      });
    });

    it("parses card discussion line link with label", () => {
      const frags = parseMessageBody("see <#general/20260520-035646-7cf:L000004|Token Rotation>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "20260520-035646-7cf",
        line: 4,
        label: "Token Rotation",
      });
    });

    it("parses legacy bare card reference", () => {
      const frags = parseMessageBody("see #general/20260520-035646-7cf");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "20260520-035646-7cf",
      });
    });

    it("parses legacy bare card reference with line", () => {
      const frags = parseMessageBody("see #general/20260520-035646-7cf L4");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "20260520-035646-7cf",
        line: 4,
      });
    });

    it("parses card link with hyphenated channel", () => {
      const frags = parseMessageBody("<#code-skill-track/20260522-033047-522>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "code-skill-track",
        cardId: "20260522-033047-522",
      });
    });

    it("does not parse card link with invalid channel", () => {
      // "a/b/c" -> channel="a/b" which fails isValidChannel
      const frags = parseMessageBody("<#a/b/c>");
      expect(findFirst(frags, "card-link")).toBeUndefined();
      expect(findFirst(frags, "channel-link")).toBeUndefined();
    });

    it("does not parse empty card id", () => {
      const frags = parseMessageBody("<#general/>");
      expect(findFirst(frags, "card-link")).toBeUndefined();
    });

    it("does not parse message link as card link", () => {
      const frags = parseMessageBody("<#dev:L000042>");
      expect(findFirst(frags, "card-link")).toBeUndefined();
      expect(findFirst(frags, "message-link")).toEqual({
        type: "message-link",
        channel: "dev",
        line: 42,
      });
    });

    it("does not parse channel link as card link", () => {
      const frags = parseMessageBody("<#general>");
      expect(findFirst(frags, "card-link")).toBeUndefined();
      expect(findFirst(frags, "channel-link")).toEqual({ type: "channel-link", channel: "general" });
    });

    it("does not parse card link with newline", () => {
      const frags = parseMessageBody("<#general/abc\n123>");
      expect(findFirst(frags, "card-link")).toBeUndefined();
    });
  });

  describe("edge cases", () => {
    it("parses bounded Quick Session refs and optional line targets", () => {
      expect(
        findFirst(
          parseMessageBody("See session:qs-01JZZZZZZZZZZZZZZZZZZZZZZZ:L000007."),
          "session-link",
        ),
      ).toEqual({
        type: "session-link",
        sessionId: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
        line: 7,
      });
      expect(
        findFirst(
          parseMessageBody("xsession:qs-01JZZZZZZZZZZZZZZZZZZZZZZZ"),
          "session-link",
        ),
      ).toBeUndefined();
    });

    it("ignores invalid handler in mention", () => {
      const frags = parseMessageBody("<@System>");
      expect(findFirst(frags, "mention")).toBeUndefined();
    });

    it("ignores empty channel link", () => {
      const frags = parseMessageBody("<#>");
      expect(findFirst(frags, "channel-link")).toBeUndefined();
    });

    it("ignores unclosed marker", () => {
      const frags = parseMessageBody("<#general");
      expect(findFirst(frags, "channel-link")).toBeUndefined();
    });
  });
});
