import { beforeEach, describe, expect, it } from "vitest";
import type { Message } from "../lib/types";
import { useChatStore } from "./use-chat-store";

function msg(line: number, body: string, extra: Partial<Message> = {}): Message {
  return {
    line_number: line,
    point_to: 0,
    author: "flame4",
    timestamp: "20260507T151500Z",
    body,
    ...extra,
  };
}

describe("useChatStore pending messages", () => {
  beforeEach(() => {
    useChatStore.getState().resetForWorkspaceSwitch();
  });

  it("removes the pending copy when the real message arrived before send confirmation", () => {
    const pending = msg(-1, "我能看到", {
      _pendingId: "pending-1",
      _status: "sending",
    });
    const real = msg(42, "我能看到");

    useChatStore.getState().addPendingMessage(pending);
    useChatStore.getState().addMessages([real]);
    useChatStore.getState().markPendingSent("pending-1", 42);

    expect(useChatStore.getState().messages).toEqual([real]);
  });
});

describe("useChatStore history pagination", () => {
  beforeEach(() => {
    useChatStore.getState().resetForWorkspaceSwitch();
  });

  it("prependMessages on empty messages stores them as the initial set", () => {
    useChatStore.getState().prependMessages([msg(10, "old"), msg(11, "older")]);
    expect(useChatStore.getState().messages.map((m) => m.line_number)).toEqual([
      10, 11,
    ]);
  });

  it("prependMessages places older entries before existing ones and keeps line_number ascending", () => {
    useChatStore.getState().setMessages([msg(50, "current"), msg(51, "current+1")]);
    useChatStore.getState().prependMessages([msg(48, "older"), msg(49, "older+1")]);
    expect(useChatStore.getState().messages.map((m) => m.line_number)).toEqual([
      48, 49, 50, 51,
    ]);
  });

  it("prependMessages skips entries whose line_number already exists", () => {
    useChatStore.getState().setMessages([msg(50, "current")]);
    useChatStore
      .getState()
      .prependMessages([msg(48, "older"), msg(50, "duplicate")]);
    expect(useChatStore.getState().messages.map((m) => m.line_number)).toEqual([
      48, 50,
    ]);
    // Existing entry's body must not be clobbered by the duplicate.
    expect(useChatStore.getState().messages[1].body).toBe("current");
  });

  it("prependMessages with an empty array is a no-op", () => {
    const before = [msg(50, "a"), msg(51, "b")];
    useChatStore.getState().setMessages(before);
    useChatStore.getState().prependMessages([]);
    expect(useChatStore.getState().messages.map((m) => m.line_number)).toEqual([
      50, 51,
    ]);
  });

  it("setMessages([]) resets hasMoreHistory to true (re-arming on channel switch)", () => {
    useChatStore.getState().setHasMoreHistory(false);
    expect(useChatStore.getState().hasMoreHistory).toBe(false);
    useChatStore.getState().setMessages([]);
    expect(useChatStore.getState().hasMoreHistory).toBe(true);
  });

  it("selectChannel resets hasMoreHistory to true (channel switch via selectChannel path)", () => {
    useChatStore.getState().setHasMoreHistory(false);
    useChatStore.getState().selectChannel("other");
    expect(useChatStore.getState().hasMoreHistory).toBe(true);
  });

  it("hasMoreHistory defaults to true on a fresh workspace", () => {
    expect(useChatStore.getState().hasMoreHistory).toBe(true);
  });

  it("prependMessages preserves trailing pending entries instead of pulling them to the head", () => {
    // Defensive: a pending outbound message lives at the tail with
    // line_number = -1 (smallest in the list). If prependMessages ever sorted
    // the full merged list instead of just the new batch, the pending entry
    // would jump to the head and the user's just-sent message would visually
    // disappear under newly-loaded history. This test pins the contract.
    useChatStore.getState().setMessages([msg(50, "real")]);
    useChatStore.getState().addPendingMessage(
      msg(-1, "outbound", { _pendingId: "p1", _status: "sending" }),
    );
    useChatStore.getState().prependMessages([msg(48, "older-a"), msg(49, "older-b")]);

    const lines = useChatStore.getState().messages.map((m) => m.line_number);
    expect(lines).toEqual([48, 49, 50, -1]);
  });
});
