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
  readUiState,
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
      JSON.stringify({ boardHandler: "alice", cardsShowArchived: true }),
    );
    expect(readUiState("runtime:myws")).toEqual({
      boardHandler: "alice",
      cardsShowArchived: true,
      usageBreakdown: "provider",
    });
  });

  it("falls back individual fields that have wrong types, keeps valid ones", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({ cardsShowArchived: "yes" }),
    );
    expect(readUiState("runtime:myws")).toEqual({
      boardHandler: null,
      cardsShowArchived: false,
      usageBreakdown: "provider",
    });
  });

  it("write merges patch without overwriting unpatched fields", () => {
    writeUiState("runtime:myws", { cardsShowArchived: true });
    writeUiState("runtime:myws", { boardHandler: "alice" });
    expect(readUiState("runtime:myws")).toEqual({
      boardHandler: "alice",
      cardsShowArchived: true,
      usageBreakdown: "provider",
    });
  });

  it("clear removes the stored entry", () => {
    writeUiState("runtime:myws", { boardHandler: "alice" });
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
      JSON.stringify({ cardsShowArchived: true }),
    );
    const state = readUiState("runtime:myws");
    expect(state.usageBreakdown).toBe("provider");
    expect(state.cardsShowArchived).toBe(true);
  });

  it("ignores legacy chat fields that now belong to chat-ui-state", () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:myws",
      JSON.stringify({
        channel: "general",
        cardsShowArchived: true,
        unreadByChannel: {
          general: { unreadCount: 2, hasMention: true },
        },
        messageScrollByScope: {
          general: 120,
        },
      }),
    );

    const state = readUiState("runtime:myws");
    expect(state).toEqual({
      boardHandler: null,
      cardsShowArchived: true,
      usageBreakdown: "provider",
    });
  });
});
