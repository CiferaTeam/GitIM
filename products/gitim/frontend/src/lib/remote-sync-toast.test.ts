import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Message, PollChange } from "./types";

const mocks = vi.hoisted(() => ({
  toast: {
    info: vi.fn(),
    success: vi.fn(),
  },
}));

vi.mock("sonner", () => ({
  toast: mocks.toast,
}));

import {
  recordRemoteSyncPending,
  remoteSyncFailure,
  resetRemoteSyncToastState,
  resolveRemoteSyncFromChanges,
} from "./remote-sync-toast";

function message(author: string, body: string, lineNumber = 3): Message {
  return {
    line_number: lineNumber,
    point_to: 0,
    author,
    timestamp: "20260521T000000Z",
    body,
  };
}

function change(scope: string, entries: Message[]): PollChange {
  return {
    kind: scope.startsWith("card:") ? "card_thread" : "new_messages",
    channel: scope,
    entries,
  };
}

describe("remote sync toast tracker", () => {
  beforeEach(() => {
    resetRemoteSyncToastState();
    vi.clearAllMocks();
  });

  it("extracts commit-only sync failures from daemon and browser responses", () => {
    expect(remoteSyncFailure({
      status: "commit_only",
      error: "push cycle completed without success",
    })).toBe("push cycle completed without success");

    expect(remoteSyncFailure({
      sync_status: "commit_only",
      sync_error: "HTTP Error: 401 Unauthorized",
    })).toBe("HTTP Error: 401 Unauthorized");

    expect(remoteSyncFailure({ status: "pushed" })).toBeNull();
  });

  it("confirms pending local sync only when the matching remote message appears", () => {
    recordRemoteSyncPending(
      "runtime:room",
      {
        scope: "general",
        author: "lewis",
        body: "queued",
        lineNumber: 3,
      },
      "push cycle completed without success",
    );

    expect(resolveRemoteSyncFromChanges("runtime:room", [
      change("general", [message("alice", "queued")]),
      change("random", [message("lewis", "queued")]),
    ])).toBe(0);
    expect(mocks.toast.success).not.toHaveBeenCalled();

    expect(resolveRemoteSyncFromChanges("runtime:room", [
      change("general", [message("lewis", "queued")]),
    ])).toBe(1);
    expect(mocks.toast.success).toHaveBeenCalledWith(
      "Synced to remote",
      expect.objectContaining({
        id: "remote-sync:runtime:room",
        description: "Queued local changes uploaded after retry.",
      }),
    );
  });

  it("keeps the toast pending until every queued local message is confirmed", () => {
    recordRemoteSyncPending(
      "runtime:room",
      { scope: "general", author: "lewis", body: "first", lineNumber: 3 },
      "push cycle completed without success",
    );
    recordRemoteSyncPending(
      "runtime:room",
      { scope: "general", author: "lewis", body: "second", lineNumber: 4 },
      "push cycle completed without success",
    );

    expect(resolveRemoteSyncFromChanges("runtime:room", [
      change("general", [message("lewis", "first", 3)]),
    ])).toBe(1);
    expect(mocks.toast.info).toHaveBeenLastCalledWith(
      "Remote sync progressing…",
      expect.objectContaining({
        id: "remote-sync:runtime:room",
        description: "1 uploaded, 1 still waiting.",
      }),
    );
    expect(mocks.toast.success).not.toHaveBeenCalled();

    expect(resolveRemoteSyncFromChanges("runtime:room", [
      change("general", [message("lewis", "second", 4)]),
    ])).toBe(1);
    expect(mocks.toast.success).toHaveBeenCalledWith(
      "Synced to remote",
      expect.objectContaining({ id: "remote-sync:runtime:room" }),
    );
  });
});
