import { beforeEach, describe, expect, it } from "vitest";

import { useComposerOperationStore } from "./use-composer-operation-store";

function store() {
  return useComposerOperationStore.getState();
}

beforeEach(() => {
  store().invalidateAll();
  // completionSequence/completions are append-only by design (they must
  // outlive scope disposal); tests assert relative ordering only.
});

describe("composer operation tokens", () => {
  it("mints monotonic tokens and holds at most one active operation per scope", () => {
    const first = store().beginOperation("scope-a", "text");
    expect(first).not.toBeNull();
    expect(store().isOperationCurrent("scope-a", first!.token)).toBe(true);

    expect(store().beginOperation("scope-a", "attachments")).toBeNull();
    expect(store().beginOperation("scope-a", "text")).toBeNull();

    const other = store().beginOperation("scope-b", "attachments");
    expect(other).not.toBeNull();
    expect(other!.token).toBeGreaterThan(first!.token);
    expect(store().isOperationCurrent("scope-b", other!.token)).toBe(true);
    expect(store().isOperationCurrent("scope-a", other!.token)).toBe(false);
    expect(store().isOperationCurrent("scope-b", first!.token)).toBe(false);
  });

  it("settles only the matching token and never reuses tokens after settlement", () => {
    const first = store().beginOperation("scope", "attachments")!;

    expect(store().endOperation("scope", first.token + 1)).toBe(false);
    expect(store().isOperationCurrent("scope", first.token)).toBe(true);

    expect(store().endOperation("scope", first.token)).toBe(true);
    expect(store().isOperationCurrent("scope", first.token)).toBe(false);
    expect(store().endOperation("scope", first.token)).toBe(false);

    const second = store().beginOperation("scope", "attachments")!;
    expect(second.token).toBeGreaterThan(first.token);
    expect(store().isOperationCurrent("scope", first.token)).toBe(false);
  });

  it("invalidates the given scopes and ignores unknown or idle ones", () => {
    const a = store().beginOperation("a", "text")!;
    const b = store().beginOperation("b", "attachments")!;

    store().invalidateScopes(["a", "never-begun"]);

    expect(store().isOperationCurrent("a", a.token)).toBe(false);
    expect(store().isOperationCurrent("b", b.token)).toBe(true);

    store().invalidateAll();
    expect(store().isOperationCurrent("b", b.token)).toBe(false);
    expect(store().operations).toEqual({});
  });

  it("keeps the completion stream append-only across invalidation", () => {
    const key = "scope";
    const operation = store().beginOperation(key, "text")!;
    const base = store().completionSequence;
    store().publishCompletion(key, operation.token, "sent text", 7);
    store().invalidateScopes([key]);

    const completion = store().completions.at(-1);
    expect(store().completionSequence).toBe(base + 1);
    expect(completion).toMatchObject({
      sequence: base + 1,
      key,
      token: operation.token,
      text: "sent text",
      replyLine: 7,
    });
  });

  it("sequences completions and caps the ring at 32 events", () => {
    const base = store().completionSequence;
    for (let index = 0; index < 40; index += 1) {
      store().publishCompletion("scope", index + 1, `text-${index}`, null);
    }

    const state = store();
    expect(state.completionSequence).toBe(base + 40);
    expect(state.completions).toHaveLength(32);
    expect(state.completions[0].sequence).toBe(base + 9);
    expect(state.completions.at(-1)).toMatchObject({
      sequence: base + 40,
      key: "scope",
      token: 40,
      text: "text-39",
      replyLine: null,
    });
    expect(Object.isFrozen(state.completions.at(-1))).toBe(true);
  });
});
