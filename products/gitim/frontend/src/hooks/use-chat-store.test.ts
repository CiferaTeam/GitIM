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
