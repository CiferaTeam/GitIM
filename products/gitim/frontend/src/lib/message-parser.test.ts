import { describe, expect, it } from "vitest";
import { parseMessageBody, type Fragment } from "./message-parser";

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
        title: "Token Rotation",
      });
    });

    it("parses card link with empty title", () => {
      const frags = parseMessageBody("see <#general/abc123|>");
      const link = findFirst(frags, "card-link");
      expect(link).toEqual({
        type: "card-link",
        channel: "general",
        cardId: "abc123",
        title: "",
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
