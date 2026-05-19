import { beforeEach, describe, expect, it } from "vitest";
import type { ArchivedDmEntry } from "../lib/client";
import type { Channel, Message } from "../lib/types";
import { useChatStore } from "./use-chat-store";

function dmEntry(stem: string, peer: string): ArchivedDmEntry {
  return { dm_pair_stem: stem, peer };
}

function channel(name: string, kind: Channel["kind"] = "channel"): Channel {
  return {
    name,
    kind,
    unreadCount: 0,
    hasMention: false,
    members: ["lewis"],
  };
}

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

  it("addMessages drops a failed pending when the matching real entry arrives", () => {
    // Failed-pending leak: HTTP times out, but daemon actually wrote line 17.
    // Without content-based dedup the "Failed ✗" copy sticks around forever
    // because its line_number (-1) never collides with the real line.
    const pending = msg(-1, "你这样昨天那个邮箱测试", {
      _pendingId: "pending-1",
      _status: "failed",
    });
    const real = msg(17, "你这样昨天那个邮箱测试");

    useChatStore.getState().addPendingMessage(pending);
    useChatStore.getState().markPendingFailed("pending-1");
    useChatStore.getState().addMessages([real]);

    expect(useChatStore.getState().messages).toEqual([real]);
  });

  it("addMessages keeps a failed pending when nothing in the incoming batch matches", () => {
    // Defensive: don't accidentally drop a truly-failed message just because
    // *any* new entry arrived. Only matching (author, body) drops it.
    const pending = msg(-1, "totally different", {
      _pendingId: "pending-1",
      _status: "failed",
    });
    const real = msg(17, "unrelated content");

    useChatStore.getState().addPendingMessage(pending);
    useChatStore.getState().markPendingFailed("pending-1");
    useChatStore.getState().addMessages([real]);

    const lines = useChatStore.getState().messages.map((m) => m.line_number);
    expect(lines.sort((a, b) => a - b)).toEqual([-1, 17]);
  });

  it("setMessages drops a failed pending when the new batch contains the matching real entry", () => {
    const pending = msg(-1, "hello", {
      _pendingId: "pending-1",
      _status: "failed",
    });
    const real = msg(17, "hello");

    useChatStore.getState().addPendingMessage(pending);
    useChatStore.getState().markPendingFailed("pending-1");
    useChatStore.getState().setMessages([real]);

    expect(useChatStore.getState().messages).toEqual([real]);
  });

  it("markPendingFailed drops the pending if the real entry already arrived", () => {
    // Symmetric to markPendingSent's realAlreadyArrived check. Ordering can
    // flip when polling beats our HTTP timeout: real lands first, then our
    // send call resolves with an error and tries to mark failed.
    const real = msg(17, "raced through");

    useChatStore.getState().addMessages([real]);
    useChatStore.getState().addPendingMessage(
      msg(-1, "raced through", { _pendingId: "pending-1", _status: "sending" })
    );
    useChatStore.getState().markPendingFailed("pending-1");

    expect(useChatStore.getState().messages).toEqual([real]);
  });

  it("addMessages merges recipients from the real poll echo into a sent pending message", () => {
    const pending = msg(-1, "通知范围需要可见", {
      _pendingId: "pending-1",
      _status: "sending",
    });
    const real = msg(42, "通知范围需要可见", {
      recipients: ["lewis", "flame4"],
    });

    useChatStore.getState().addPendingMessage(pending);
    useChatStore.getState().markPendingSent("pending-1", 42);
    useChatStore.getState().addMessages([real]);

    expect(useChatStore.getState().messages).toEqual([
      {
        ...real,
        _pendingId: "pending-1",
        _status: "sent",
      },
    ]);
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

describe("useChatStore archivedChannelsView", () => {
  beforeEach(() => {
    useChatStore.getState().resetForWorkspaceSwitch();
  });

  it("starts null on a fresh workspace", () => {
    expect(useChatStore.getState().archivedChannelsView).toBeNull();
  });

  it("resetArchivedChannelsView writes a fresh view and clears the snapshot", () => {
    useChatStore.getState().setArchivedChannels([channel("old")]);

    useChatStore.getState().resetArchivedChannelsView("eng");

    expect(useChatStore.getState().archivedChannels).toEqual([]);
    expect(useChatStore.getState().archivedChannelsView).toEqual({
      items: [],
      offset: 0,
      hasMore: true,
      query: "eng",
      loading: false,
      error: null,
    });
  });

  it("appendArchivedChannelsPage deduplicates by name and advances offset by incoming rows", () => {
    useChatStore.getState().resetArchivedChannelsView("");
    useChatStore.getState().appendArchivedChannelsPage({
      items: [channel("alpha"), channel("beta")],
      hasMore: true,
    });
    useChatStore.getState().appendArchivedChannelsPage({
      items: [channel("beta"), channel("gamma")],
      hasMore: false,
    });

    const view = useChatStore.getState().archivedChannelsView!;
    expect(view.items.map((c) => c.name)).toEqual(["alpha", "beta", "gamma"]);
    expect(view.offset).toBe(4);
    expect(view.hasMore).toBe(false);
    expect(useChatStore.getState().archivedChannels.map((c) => c.name)).toEqual([
      "alpha",
      "beta",
      "gamma",
    ]);
  });

  it("loading and error actions are no-ops until the view is initialized", () => {
    useChatStore.getState().setArchivedChannelsLoading(true);
    useChatStore.getState().setArchivedChannelsError("ignored");
    expect(useChatStore.getState().archivedChannelsView).toBeNull();
  });

  it("setArchivedChannelsLoading clears errors when entering loading", () => {
    useChatStore.getState().resetArchivedChannelsView("");
    useChatStore.getState().setArchivedChannelsError("stale");

    useChatStore.getState().setArchivedChannelsLoading(true);

    const view = useChatStore.getState().archivedChannelsView!;
    expect(view.loading).toBe(true);
    expect(view.error).toBeNull();
  });

  it("markChannelArchived removes active channel and invalidates the archive view", () => {
    useChatStore.getState().setChannels([channel("general")]);
    useChatStore.getState().resetArchivedChannelsView("");
    useChatStore.getState().appendArchivedChannelsPage({
      items: [channel("old")],
      hasMore: false,
    });

    useChatStore.getState().markChannelArchived("general");

    expect(useChatStore.getState().channels).toEqual([]);
    expect(useChatStore.getState().archivedChannels).toEqual([]);
    expect(useChatStore.getState().archivedChannelsView).toBeNull();
  });

  it("markChannelUnarchived removes loaded archive item and seeds active channels", () => {
    useChatStore.getState().resetArchivedChannelsView("");
    useChatStore.getState().appendArchivedChannelsPage({
      items: [
        { ...channel("general"), kind: "archived_channel" as Channel["kind"] },
        { ...channel("random"), kind: "archived_channel" as Channel["kind"] },
      ],
      hasMore: false,
    });

    useChatStore.getState().markChannelUnarchived("general");

    expect(
      useChatStore.getState().archivedChannelsView!.items.map((c) => c.name),
    ).toEqual(["random"]);
    expect(useChatStore.getState().channels).toEqual([
      expect.objectContaining({ name: "general", kind: "channel" }),
    ]);
  });

  it("invalidateArchivedChannelsView and resetForWorkspaceSwitch clear the lazy view", () => {
    useChatStore.getState().resetArchivedChannelsView("");
    useChatStore.getState().appendArchivedChannelsPage({
      items: [channel("general")],
      hasMore: false,
    });

    useChatStore.getState().invalidateArchivedChannelsView();
    expect(useChatStore.getState().archivedChannelsView).toBeNull();

    useChatStore.getState().resetArchivedChannelsView("");
    useChatStore.getState().resetForWorkspaceSwitch();
    expect(useChatStore.getState().archivedChannelsView).toBeNull();
  });
});

describe("useChatStore archivedDmsView", () => {
  beforeEach(() => {
    useChatStore.getState().resetForWorkspaceSwitch();
  });

  it("starts null on a fresh workspace (not initialized until first expand)", () => {
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });

  it("resetArchivedDmsView writes a fresh view with the given query", () => {
    useChatStore.getState().resetArchivedDmsView("ali");
    const view = useChatStore.getState().archivedDmsView;
    expect(view).not.toBeNull();
    expect(view).toEqual({
      items: [],
      offset: 0,
      hasMore: true,
      query: "ali",
      loading: false,
      error: null,
    });
  });

  it("resetArchivedDmsView clears any stale items / error / offset from a prior view", () => {
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("alice--bob", "alice"), dmEntry("bob--carol", "carol")],
      hasMore: true,
    });
    useChatStore.getState().setArchivedDmsError("boom");

    useChatStore.getState().resetArchivedDmsView("carol");

    expect(useChatStore.getState().archivedDmsView).toEqual({
      items: [],
      offset: 0,
      hasMore: true,
      query: "carol",
      loading: false,
      error: null,
    });
  });

  it("appendArchivedDmsPage is a no-op when the view is null", () => {
    useChatStore
      .getState()
      .appendArchivedDmsPage({ items: [dmEntry("a--b", "a")], hasMore: false });
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });

  it("appendArchivedDmsPage extends items + advances offset + overwrites hasMore", () => {
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("alice--bob", "alice"), dmEntry("bob--carol", "carol")],
      hasMore: true,
    });

    let view = useChatStore.getState().archivedDmsView!;
    expect(view.items.map((e) => e.dm_pair_stem)).toEqual([
      "alice--bob",
      "bob--carol",
    ]);
    expect(view.offset).toBe(2);
    expect(view.hasMore).toBe(true);

    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("carol--dave", "dave")],
      hasMore: false,
    });

    view = useChatStore.getState().archivedDmsView!;
    expect(view.items.map((e) => e.dm_pair_stem)).toEqual([
      "alice--bob",
      "bob--carol",
      "carol--dave",
    ]);
    expect(view.offset).toBe(3);
    expect(view.hasMore).toBe(false);
  });

  it("appendArchivedDmsPage deduplicates by dm_pair_stem (idempotent retry)", () => {
    // Same page could land twice if a race / retry causes overlap; we must
    // not double-render the same entry.
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("alice--bob", "alice"), dmEntry("bob--carol", "carol")],
      hasMore: true,
    });
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("bob--carol", "carol"), dmEntry("carol--dave", "dave")],
      hasMore: false,
    });

    const view = useChatStore.getState().archivedDmsView!;
    expect(view.items.map((e) => e.dm_pair_stem)).toEqual([
      "alice--bob",
      "bob--carol",
      "carol--dave",
    ]);
    // offset advanced by the count of *incoming* page entries; pagination is
    // server-side so the daemon owns offset semantics, not the client.
    expect(view.offset).toBe(4);
    expect(view.hasMore).toBe(false);
  });

  it("appendArchivedDmsPage clears any prior error", () => {
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().setArchivedDmsError("transient");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("a--b", "a")],
      hasMore: false,
    });
    expect(useChatStore.getState().archivedDmsView!.error).toBeNull();
  });

  it("setArchivedDmsLoading toggles loading and clears error on enter", () => {
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().setArchivedDmsError("oops");

    useChatStore.getState().setArchivedDmsLoading(true);
    let view = useChatStore.getState().archivedDmsView!;
    expect(view.loading).toBe(true);
    expect(view.error).toBeNull();

    useChatStore.getState().setArchivedDmsLoading(false);
    view = useChatStore.getState().archivedDmsView!;
    expect(view.loading).toBe(false);
  });

  it("setArchivedDmsLoading is a no-op when the view is null", () => {
    useChatStore.getState().setArchivedDmsLoading(true);
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });

  it("setArchivedDmsError writes the error and stops loading", () => {
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().setArchivedDmsLoading(true);
    useChatStore.getState().setArchivedDmsError("backend down");
    const view = useChatStore.getState().archivedDmsView!;
    expect(view.error).toBe("backend down");
    expect(view.loading).toBe(false);
  });

  it("setArchivedDmsError is a no-op when the view is null", () => {
    useChatStore.getState().setArchivedDmsError("ignored");
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });

  it("markDmArchived invalidates the view (sets it back to null)", () => {
    // The view is paginated and order-dependent; we can't know where a
    // freshly-archived DM belongs in the sorted result. Force a refetch.
    const dm: Channel = {
      name: "alice--bob",
      kind: "dm",
      unreadCount: 0,
      hasMention: false,
      members: ["alice", "bob"],
    };
    useChatStore.getState().setChannels([dm]);
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("bob--carol", "carol")],
      hasMore: false,
    });

    useChatStore.getState().markDmArchived("alice--bob");

    expect(useChatStore.getState().channels).toEqual([]);
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });

  it("markDmUnarchived removes the entry from the view and synthesizes a Channel back into channels", () => {
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().appendArchivedDmsPage({
      items: [
        dmEntry("alice--bob", "alice"),
        dmEntry("bob--carol", "carol"),
      ],
      hasMore: false,
    });

    useChatStore.getState().markDmUnarchived("alice--bob");

    const view = useChatStore.getState().archivedDmsView!;
    expect(view.items.map((e) => e.dm_pair_stem)).toEqual(["bob--carol"]);
    // Synthesized Channel keyed by the same stem so existing channel-name
    // code paths keep working without special-casing.
    const channels = useChatStore.getState().channels;
    expect(channels).toHaveLength(1);
    expect(channels[0]).toMatchObject({
      name: "alice--bob",
      kind: "dm",
      unreadCount: 0,
      hasMention: false,
      members: ["alice", "bob"],
    });
  });

  it("markDmUnarchived does not double-insert if the DM is already in channels", () => {
    const dm: Channel = {
      name: "alice--bob",
      kind: "dm",
      unreadCount: 0,
      hasMention: false,
      members: ["alice", "bob"],
    };
    useChatStore.getState().setChannels([dm]);
    useChatStore.getState().resetArchivedDmsView("");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("alice--bob", "alice")],
      hasMore: false,
    });

    useChatStore.getState().markDmUnarchived("alice--bob");

    expect(useChatStore.getState().channels).toHaveLength(1);
    expect(
      useChatStore.getState().archivedDmsView!.items.length,
    ).toBe(0);
  });

  it("markDmUnarchived when the view is null still seeds a synthesized Channel (handles SSE-driven unarchive before expand)", () => {
    useChatStore.getState().markDmUnarchived("alice--bob");
    const channels = useChatStore.getState().channels;
    expect(channels).toHaveLength(1);
    expect(channels[0].name).toBe("alice--bob");
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });

  it("resetForWorkspaceSwitch clears archivedDmsView back to null", () => {
    useChatStore.getState().resetArchivedDmsView("foo");
    useChatStore.getState().appendArchivedDmsPage({
      items: [dmEntry("a--b", "a")],
      hasMore: false,
    });
    useChatStore.getState().resetForWorkspaceSwitch();
    expect(useChatStore.getState().archivedDmsView).toBeNull();
  });
});
