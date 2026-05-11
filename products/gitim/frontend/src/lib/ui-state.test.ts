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
      JSON.stringify({ channel: 42, boardHandler: "alice", cardsShowArchived: true }),
    );
    expect(readUiState("runtime:myws")).toEqual({
      channel: null,
      boardHandler: "alice",
      cardsShowArchived: true,
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
    });
  });

  it("write merges patch without overwriting unpatched fields", () => {
    writeUiState("runtime:myws", { channel: "general" });
    writeUiState("runtime:myws", { boardHandler: "alice" });
    expect(readUiState("runtime:myws")).toEqual({
      channel: "general",
      boardHandler: "alice",
      cardsShowArchived: false,
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
});
