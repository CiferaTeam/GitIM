import { describe, expect, it } from "vitest";
import { runningLockedFields, runningLockNotice } from "./agent-field-lock";

describe("runningLockedFields", () => {
  it("claude locks both Model and Effort", () => {
    expect(runningLockedFields("claude", true)).toEqual(["Model", "Effort"]);
  });

  it("claude still locks Effort even when model is not editable", () => {
    expect(runningLockedFields("claude", false)).toEqual(["Effort"]);
  });

  it("codex locks both Model and Effort", () => {
    expect(runningLockedFields("codex", true)).toEqual(["Model", "Effort"]);
  });

  it("hermes locks nothing (model is read-only, no effort field)", () => {
    expect(runningLockedFields("hermes", false)).toEqual([]);
  });

  it("an editable model with no provider yet still counts as locked", () => {
    expect(runningLockedFields(undefined, true)).toEqual(["Model"]);
  });
});

describe("runningLockNotice", () => {
  it("is null when nothing is locked", () => {
    expect(runningLockNotice("hermes", false)).toBeNull();
  });

  it("names both fields for claude", () => {
    expect(runningLockNotice("claude", true)).toContain("Model and Effort");
  });

  it("names both fields for codex", () => {
    const notice = runningLockNotice("codex", true);
    expect(notice).toContain("Model and Effort");
  });
});
