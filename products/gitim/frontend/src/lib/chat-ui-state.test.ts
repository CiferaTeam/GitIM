import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@isomorphic-git/lightning-fs", () => ({
  default: class MockLightningFS {
    promises = { stat: () => Promise.resolve({}) };
  },
}));

import "@/lib/browser-workspaces";
import {
  chatScopeKeyForChannel,
  chatScopeKeyForName,
  chatScopeName,
  clearChatScopeUnread,
  clearChatUiState,
  incrementChatScopeUnread,
  mergeChatUnreadIntoChannels,
  readActiveChatScope,
  readChatScopeState,
  readChatScopeScrollTop,
  writeActiveChatScope,
  writeChatScopeScrollTop,
} from "./chat-ui-state";

describe("chat-ui-state", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("stores active scope separately from per-scope state", () => {
    writeActiveChatScope("runtime:room", "channel:general");
    writeChatScopeScrollTop("runtime:room", "channel:general", 240);

    expect(readActiveChatScope("runtime:room")).toBe("channel:general");
    expect(readChatScopeScrollTop("runtime:room", "channel:general")).toBe(240);
    expect(localStorage.getItem("gitim:ui:v2:runtime%3Aroom:activeScope")).toBe(
      "channel:general",
    );
    expect(
      JSON.parse(
        localStorage.getItem(
          "gitim:ui:v2:runtime%3Aroom:scope:channel%3Ageneral",
        )!,
      ),
    ).toMatchObject({ scrollTop: 240 });
  });

  it("tracks unread count, mention state, and first unread line per scope", () => {
    const scope = "channel:opencode-provider-timeout-0519";

    incrementChatScopeUnread("runtime:room", scope, {
      count: 2,
      hasMention: false,
      firstUnreadLine: 50,
    });
    incrementChatScopeUnread("runtime:room", scope, {
      count: 1,
      hasMention: true,
      firstUnreadLine: 52,
    });

    expect(readChatScopeState("runtime:room", scope)).toMatchObject({
      unreadCount: 3,
      hasMention: true,
      firstUnreadLine: 50,
    });

    clearChatScopeUnread("runtime:room", scope);
    expect(readChatScopeState("runtime:room", scope)).toMatchObject({
      unreadCount: 0,
      hasMention: false,
      firstUnreadLine: null,
      scrollTop: null,
    });
  });

  it("merges persisted unread state into channels for sidebar display", () => {
    incrementChatScopeUnread("runtime:room", "channel:general", {
      count: 1,
      hasMention: false,
      firstUnreadLine: 12,
    });
    incrementChatScopeUnread("runtime:room", "dm:cfo--flame4", {
      count: 2,
      hasMention: true,
      firstUnreadLine: 3,
    });

    const channels = mergeChatUnreadIntoChannels("runtime:room", [
      { name: "general", kind: "channel" as const },
      { name: "cfo--flame4", kind: "dm" as const },
      { name: "leaders", kind: "channel" as const },
    ]);

    expect(channels).toEqual([
      { name: "general", kind: "channel", unreadCount: 1, hasMention: false },
      { name: "cfo--flame4", kind: "dm", unreadCount: 2, hasMention: true },
      { name: "leaders", kind: "channel" },
    ]);
  });

  it("ignores legacy chat fields instead of migrating them into v2 state", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:room",
      JSON.stringify({
        channel: "general",
        unreadByChannel: {
          general: { unreadCount: 2, hasMention: true },
        },
        messageScrollByScope: {
          general: 360,
          "card:general/card-1": 720,
        },
      }),
    );

    expect(readActiveChatScope("runtime:room")).toBeNull();
    expect(readChatScopeState("runtime:room", "channel:general")).toEqual({
      scrollTop: null,
      unreadCount: 0,
      hasMention: false,
      firstUnreadLine: null,
      updatedAt: 0,
    });
    expect(readChatScopeScrollTop("runtime:room", "card:general/card-1")).toBeNull();
  });

  it("clears v2 active and scope keys for one workspace only", () => {
    writeActiveChatScope("runtime:room", "channel:general");
    writeChatScopeScrollTop("runtime:room", "channel:general", 10);
    writeActiveChatScope("runtime:other", "channel:leaders");
    writeChatScopeScrollTop("runtime:other", "channel:leaders", 20);

    clearChatUiState("runtime:room");

    expect(readActiveChatScope("runtime:room")).toBeNull();
    expect(readChatScopeScrollTop("runtime:room", "channel:general")).toBeNull();
    expect(readActiveChatScope("runtime:other")).toBe("channel:leaders");
    expect(readChatScopeScrollTop("runtime:other", "channel:leaders")).toBe(20);
  });

  it("normalizes channel and dm scope keys", () => {
    expect(chatScopeKeyForName("general")).toBe("channel:general");
    expect(chatScopeKeyForName("cfo--flame4")).toBe("dm:cfo--flame4");
    expect(chatScopeKeyForChannel({ name: "cfo--flame4", kind: "dm" })).toBe(
      "dm:cfo--flame4",
    );
    expect(chatScopeName("channel:general")).toBe("general");
    expect(chatScopeName("dm:cfo--flame4")).toBe("cfo--flame4");
  });
});
