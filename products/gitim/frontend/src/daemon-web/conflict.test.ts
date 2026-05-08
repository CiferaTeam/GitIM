import { describe, expect, it } from "vitest";
import { extractThreadAdditions, resolveConflicts } from "./conflict";

const baseThread = "[L000001][P000000][@alice][20260317T120000Z] base\n";
const localThread =
  baseThread +
  "[L000002][P000001][@lewis][20260317T120100Z] local\n";
const remoteThread =
  baseThread +
  "[L000002][P000001][@alice][20260317T120050Z] remote\n";

describe("daemon-web conflict resolution", () => {
  it("extracts only append-only thread additions from local content", () => {
    expect(extractThreadAdditions("channels/general.thread", localThread, baseThread))
      .toBe("[L000002][P000001][@lewis][20260317T120100Z] local\n");
  });

  it("renumbers extracted additions after the remote thread", () => {
    const additions = extractThreadAdditions(
      "channels/general.thread",
      localThread,
      baseThread,
    );
    const resolved = resolveConflicts(
      { "channels/general.thread": additions },
      { "channels/general.thread": remoteThread },
    );

    expect(resolved.files["channels/general.thread"]).toBe(
      remoteThread +
      "[L000003][P000001][@lewis][20260317T120100Z] local\n",
    );
  });

  it("rejects non-thread conflicts instead of dropping local changes", () => {
    expect(() =>
      extractThreadAdditions(
        "channels/general.meta.yaml",
        "display_name: Local\n",
        "display_name: Base\n",
      ),
    ).toThrow("Cannot auto-merge non-thread browser sync conflict");
  });

  it("rejects non-append thread conflicts", () => {
    expect(() =>
      extractThreadAdditions(
        "channels/general.thread",
        "[L000001][P000000][@alice][20260317T120000Z] edited\n",
        baseThread,
      ),
    ).toThrow("Cannot auto-merge non-append thread conflict");
  });
});
