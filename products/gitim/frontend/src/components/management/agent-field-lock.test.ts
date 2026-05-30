import { describe, expect, it } from "vitest";
import { runningLockedFields, runningLockNotice } from "./agent-field-lock";

describe("runningLockedFields", () => {
  it("claude locks both Model and Effort", () => {
    expect(runningLockedFields("claude", true)).toEqual(["Model", "Effort"]);
  });

  it("claude still locks Effort even when model is not editable", () => {
    expect(runningLockedFields("claude", false)).toEqual(["Effort"]);
  });

  it("a non-claude provider locks only Model", () => {
    expect(runningLockedFields("codex", true)).toEqual(["Model"]);
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

  it("names only Model for non-claude providers", () => {
    const notice = runningLockNotice("codex", true);
    expect(notice).toContain("Model");
    expect(notice).not.toContain("Effort");
  });
});
