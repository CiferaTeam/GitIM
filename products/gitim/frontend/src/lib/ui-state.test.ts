import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@isomorphic-git/lightning-fs", () => ({
  default: class MockLightningFS {
    promises = { stat: () => Promise.resolve({}) };
  },
}));

import "@/lib/browser-workspaces";
import {
  DEFAULT_UI_STATE,
  clearUiState,
  clearStoredUnread,
  incrementStoredUnread,
  readMessageScrollTop,
  readUiState,
  writeMessageScrollTop,
  writeUiState,
} from "./ui-state";

describe("ui-state", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("returns default values when no entry exists", () => {
    expect(readUiState("runtime:myws")).toEqual(DEFAULT_UI_STATE);
  });

  it("returns default values when workspaceKey is null", () => {
    expect(readUiState(null)).toEqual(DEFAULT_UI_STATE);
  });

  it("returns default values for corrupted JSON", () => {
    localStorage.setItem("gitim-ui-state:runtime:myws", "{not valid json");
    expect(readUiState("runtime:myws")).toEqual(DEFAULT_UI_STATE);
  });

  it("falls back field-by-field when some fields have wrong types", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({ channel: 42, boardHandler: "alice", cardsShowArchived: true }),
    );
    expect(readUiState("runtime:myws")).toEqual({
      channel: null,
      boardHandler: "alice",
      cardsShowArchived: true,
      usageBreakdown: "provider",
      unreadByChannel: {},
      messageScrollByScope: {},
    });
  });

  it("falls back individual fields that have wrong types, keeps valid ones", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({ channel: "general", cardsShowArchived: "yes" }),
    );
    expect(readUiState("runtime:myws")).toEqual({
      channel: "general",
      boardHandler: null,
      cardsShowArchived: false,
      usageBreakdown: "provider",
      unreadByChannel: {},
      messageScrollByScope: {},
    });
  });

  it("write merges patch without overwriting unpatched fields", () => {
    writeUiState("runtime:myws", { channel: "general" });
    writeUiState("runtime:myws", { boardHandler: "alice" });
    expect(readUiState("runtime:myws")).toEqual({
      channel: "general",
      boardHandler: "alice",
      cardsShowArchived: false,
      usageBreakdown: "provider",
      unreadByChannel: {},
      messageScrollByScope: {},
    });
  });

  it("clear removes the stored entry", () => {
    writeUiState("runtime:myws", { channel: "general" });
    clearUiState("runtime:myws");
    expect(readUiState("runtime:myws")).toEqual(DEFAULT_UI_STATE);
  });

  it("null workspaceKey does not write to storage", () => {
    const before = localStorage.length;
    readUiState(null);
    expect(localStorage.length).toBe(before);
  });

  it("defaults usageBreakdown to 'provider'", () => {
    expect(readUiState("runtime:myws").usageBreakdown).toBe("provider");
  });

  it("round-trips a valid usageBreakdown value", () => {
    writeUiState("runtime:myws", { usageBreakdown: "handler" });
    expect(readUiState("runtime:myws").usageBreakdown).toBe("handler");
  });

  it("falls back to default when persisted usageBreakdown is invalid", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({ usageBreakdown: "bogus" }),
    );
    expect(readUiState("runtime:myws").usageBreakdown).toBe("provider");
  });

  it("falls back to default when persisted state lacks usageBreakdown", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({ channel: "general", cardsShowArchived: true }),
    );
    const state = readUiState("runtime:myws");
    expect(state.usageBreakdown).toBe("provider");
    expect(state.channel).toBe("general");
    expect(state.cardsShowArchived).toBe(true);
  });

  it("persists unread state per workspace and clears individual channels", () => {
    incrementStoredUnread("runtime:myws", "general", false);
    incrementStoredUnread("runtime:myws", "general", true);
    incrementStoredUnread("runtime:myws", "random", false);

    expect(readUiState("runtime:myws").unreadByChannel).toEqual({
      general: { unreadCount: 2, hasMention: true },
      random: { unreadCount: 1, hasMention: false },
    });

    clearStoredUnread("runtime:myws", "general");
    expect(readUiState("runtime:myws").unreadByChannel).toEqual({
      random: { unreadCount: 1, hasMention: false },
    });
  });

  it("ignores malformed unread and message scroll entries", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({
        unreadByChannel: {
          general: { unreadCount: 2, hasMention: true },
          empty: { unreadCount: 0, hasMention: true },
          bad: { unreadCount: "many", hasMention: true },
        },
        messageScrollByScope: {
          general: 120,
          negative: -10,
          bad: "top",
        },
      }),
    );

    const state = readUiState("runtime:myws");
    expect(state.unreadByChannel).toEqual({
      general: { unreadCount: 2, hasMention: true },
    });
    expect(state.messageScrollByScope).toEqual({
      general: 120,
      negative: 0,
    });
  });

  it("persists message scrollTop per workspace scope", () => {
    writeMessageScrollTop("runtime:myws", "general", 180);
    writeMessageScrollTop("runtime:myws", "card:general/card-1", 360);

    expect(readMessageScrollTop("runtime:myws", "general")).toBe(180);
    expect(readMessageScrollTop("runtime:myws", "card:general/card-1")).toBe(360);
    expect(readMessageScrollTop("runtime:other", "general")).toBeNull();
  });
});
