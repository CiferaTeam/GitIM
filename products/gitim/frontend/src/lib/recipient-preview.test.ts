import { describe, expect, it } from "vitest";
import { computeDraftRecipients } from "./recipient-preview";
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
