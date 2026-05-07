import { beforeEach, describe, expect, it } from "vitest";
import type { Message } from "../lib/types";
import { useCardStore } from "./use-card-store";

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

describe("useCardStore pending messages", () => {
  beforeEach(() => {
    useCardStore.getState().resetForWorkspaceSwitch();
  });

  it("removes the pending copy when the real card message arrived before send confirmation", () => {
    const pending = msg(-1, "我能看到", {
      _pendingId: "pending-1",
      _status: "sending",
    });
    const real = msg(42, "我能看到");

    useCardStore.getState().addPendingCardMessage("general/card-1", pending);
    useCardStore.getState().addCardMessages("general/card-1", [real]);
    useCardStore.getState().markPendingCardSent("general/card-1", "pending-1", 42);

    expect(useCardStore.getState().cardMessagesByPath["general/card-1"]).toEqual([real]);
  });
});
