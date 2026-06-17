import { describe, expect, it } from "vitest";
import { computeCardDraftRecipients, computeDraftRecipients } from "./recipient-preview";
import type { Channel, Message } from "./types";

const baseChannel: Channel = {
  name: "handoff",
  kind: "channel",
  unreadCount: 0,
  hasMention: false,
  members: ["lewis", "flame4"],
  created_by: "cfo",
};

describe("computeDraftRecipients", () => {
  it("includes the channel creator even when the creator is not a member", () => {
    const recipients = computeDraftRecipients({
      body: "hello",
      channel: baseChannel,
      replyTo: null,
      messages: [],
    });

    expect(recipients).toEqual(["cfo"]);
  });

  it("includes the reply parent chain with the channel creator", () => {
    const root: Message = {
      line_number: 1,
      point_to: 0,
      author: "flame4",
      timestamp: "20260520T120000Z",
      body: "previous",
    };

    const recipients = computeDraftRecipients({
      body: "replying",
      channel: baseChannel,
      replyTo: root,
      messages: [root],
    });

    expect(recipients).toEqual(["cfo", "flame4"]);
  });

  it("does not route protocol mentions to users outside the channel", () => {
    const recipients = computeDraftRecipients({
      body: "member <@flame4>, reference outsider <@robin>",
      channel: baseChannel,
      replyTo: null,
      messages: [],
    });

    expect(recipients).toEqual(["cfo", "flame4"]);
  });

  it("excludes self from the recipients", () => {
    const ownerSelfChannel: Channel = {
      ...baseChannel,
      created_by: "lewis",
    };
    const recipients = computeDraftRecipients({
      body: "hello",
      channel: ownerSelfChannel,
      replyTo: null,
      messages: [],
      excludeSelf: "lewis",
    });

    expect(recipients).toEqual([]);
  });

  it("strips self from @all expansion when computing recipients", () => {
    const recipients = computeDraftRecipients({
      body: "<@all> ping",
      channel: baseChannel,
      replyTo: null,
      messages: [],
      excludeSelf: "lewis",
    });

    expect(recipients).toEqual(["cfo", "flame4"]);
  });

  it("strips self from DM recipients", () => {
    const dmChannel: Channel = {
      name: "alice--lewis",
      kind: "dm",
      unreadCount: 0,
      hasMention: false,
      members: ["alice", "lewis"],
    };
    const recipients = computeDraftRecipients({
      body: "hi",
      channel: dmChannel,
      replyTo: null,
      messages: [],
      excludeSelf: "lewis",
    });

    expect(recipients).toEqual(["alice"]);
  });
});

describe("computeCardDraftRecipients", () => {
  // Mirrors the daemon's `compute_card_thread_recipients`: cards route by task
  // role (reporter + assignee) plus explicit mentions — NOT channel membership.
  const baseCard = { created_by: "leader1", assignee: "leader2" };

  it("routes to the reporter and the assignee", () => {
    expect(
      computeCardDraftRecipients({ body: "status update", card: baseCard }),
    ).toEqual(["leader1", "leader2"]);
  });

  it("routes to the reporter only when unassigned", () => {
    expect(
      computeCardDraftRecipients({
        body: "status update",
        card: { created_by: "leader1", assignee: null },
      }),
    ).toEqual(["leader1"]);
  });

  it("unions explicit mentions with the card roles", () => {
    expect(
      computeCardDraftRecipients({ body: "ping <@composer>", card: baseCard }),
    ).toEqual(["composer", "leader1", "leader2"]);
  });

  it("routes mentions to handlers outside the channel — cards are not member-scoped", () => {
    expect(
      computeCardDraftRecipients({
        body: "<@robin>",
        card: { created_by: "leader1", assignee: null },
      }),
    ).toEqual(["leader1", "robin"]);
  });

  it("does not expand bare @all (unlike channel routing)", () => {
    expect(
      computeCardDraftRecipients({ body: "@all heads up", card: baseCard }),
    ).toEqual(["leader1", "leader2"]);
  });

  it("excludes self from the recipients", () => {
    expect(
      computeCardDraftRecipients({
        body: "status update",
        card: baseCard,
        excludeSelf: "leader2",
      }),
    ).toEqual(["leader1"]);
  });

  it("returns empty for a blank body", () => {
    expect(
      computeCardDraftRecipients({ body: "   ", card: baseCard }),
    ).toEqual([]);
  });

  it("returns empty when the card is missing", () => {
    expect(
      computeCardDraftRecipients({ body: "status update", card: null }),
    ).toEqual([]);
  });
});
