import { create } from "zustand";
import type { ArchivedDmEntry } from "../lib/client";
import type { Channel, Message } from "../lib/types";

interface NavEntry {
  channel: string;
  scrollTop: number;
}

/** Paginated, prefix-filtered view of archived DMs.
 *
 *  The previous shape (`Channel[]` fetched eagerly on workspace activation)
 *  doesn't scale — a workspace with thousands of archived DMs would block
 *  bootstrap and balloon memory. This view is lazy: `null` until the user
 *  expands the Archived DMs section, then loaded one page (limit 5) at a
 *  time through `client.listArchivedDms({ prefix, offset, limit })`.
 *
 *  `offset` mirrors the daemon's pagination cursor and always equals the
 *  number of *requested* entries so far (not the dedupped count) — the
 *  daemon owns the cursor semantics. `query` is the *applied* prefix, used
 *  by the fetch helper to drop stale responses when the user is typing.
 */
export interface ArchivedDmsView {
  items: ArchivedDmEntry[];
  offset: number;
  hasMore: boolean;
  query: string;
  loading: boolean;
  error: string | null;
}

export interface ArchivedChannelsView {
  items: Channel[];
  offset: number;
  hasMore: boolean;
  query: string;
  loading: boolean;
  error: string | null;
}

interface ChatState {
  connected: boolean;
  currentUser: string;
  isGuest: boolean;
  users: string[];
  channels: Channel[];
  /** Back-compat snapshot for callers that still need the current archived
   *  channel page. New UI should use `archivedChannelsView` so archive
   *  browsing stays lazy + paginated. */
  archivedChannels: Channel[];
  archivedChannelsView: ArchivedChannelsView | null;
  /** Lazy, paginated, prefix-filterable view of archived DMs. `null` until
   *  the sidebar Archived DMs section is first expanded; refetched (re-init
   *  to null) whenever a DM is archived out-of-band so the next expand
   *  reads a fresh sorted page. */
  archivedDmsView: ArchivedDmsView | null;
  currentChannel: string | null;
  messages: Message[];
  replyTo: Message | null;
  highlightLine: number | null;
  pendingScrollLine: number | null;
  threadRoot: Message | null;
  threadMessages: Message[];
  navHistory: NavEntry[];
  /** True until a paginated read returns fewer entries than requested (i.e.
   *  we've fetched everything older than what's currently in `messages`).
   *  Reset to true on channel switch — until the next fetch lands, we don't
   *  know whether there's more history to load. */
  hasMoreHistory: boolean;

  setConnected: (v: boolean) => void;
  setCurrentUser: (u: string) => void;
  setIsGuest: (v: boolean) => void;
  setUsers: (u: string[]) => void;
  setChannels: (c: Channel[]) => void;
  setArchivedChannels: (c: Channel[]) => void;
  resetArchivedChannelsView: (query: string) => void;
  appendArchivedChannelsPage: (page: {
    items: Channel[];
    hasMore: boolean;
  }) => void;
  setArchivedChannelsLoading: (v: boolean) => void;
  setArchivedChannelsError: (e: string | null) => void;
  invalidateArchivedChannelsView: () => void;
  /** Re-initialize the archived DMs view for a new prefix query. Clears
   *  items / offset / error and resets `hasMore` to true so the next fetch
   *  can append page 1. Use when the user changes the prefix filter or
   *  re-expands the section after invalidation. */
  resetArchivedDmsView: (query: string) => void;
  /** Append a fetched page to the view. No-op if the view is null —
   *  callers must `resetArchivedDmsView` first. Dedups by `dm_pair_stem`
   *  so a duplicate page (race / retry) doesn't render the same entry
   *  twice, but advances `offset` by the full incoming length to track
   *  the daemon's cursor. */
  appendArchivedDmsPage: (page: {
    items: ArchivedDmEntry[];
    hasMore: boolean;
  }) => void;
  setArchivedDmsLoading: (v: boolean) => void;
  setArchivedDmsError: (e: string | null) => void;
  /** Optimistic: remove a channel from active channels and invalidate the
   *  lazy archive view so the next/sidebar-open page refetch is sorted. */
  markChannelArchived: (name: string) => void;
  /** Optimistic: remove a loaded archive row and seed active channels. */
  markChannelUnarchived: (name: string) => void;
  /** Optimistic: remove a DM from active channels. The archive view is
   *  paginated + sorted, so we can't know where the new entry belongs —
   *  invalidate it (set to null) and let the next expand refetch. */
  markDmArchived: (name: string) => void;
  /** Optimistic: drop the entry from the view (by `dm_pair_stem`) and
   *  synthesize a Channel-shaped record back into `channels` so the
   *  sidebar's name-keyed render picks it up immediately. Idempotent
   *  against a DM that's already in `channels`. */
  markDmUnarchived: (name: string) => void;
  selectChannel: (name: string) => void;
  incrementUnread: (channel: string, mentioned?: boolean) => void;
  clearUnread: (channel: string) => void;
  setMessages: (m: Message[]) => void;
  addMessages: (m: Message[]) => void;
  /** Insert older messages at the head of `messages`, deduping by line_number
   *  against existing entries. Resulting list stays ascending by line_number,
   *  assuming the existing list was already ascending. */
  prependMessages: (m: Message[]) => void;
  setHasMoreHistory: (v: boolean) => void;
  addPendingMessage: (m: Message) => void;
  markPendingSent: (pendingId: string, lineNumber: number) => void;
  markPendingFailed: (pendingId: string, lineNumber?: number) => void;
  removePendingMessage: (pendingId: string) => void;
  setReplyTo: (m: Message | null) => void;
  setHighlightLine: (line: number | null) => void;
  setPendingScrollLine: (line: number | null) => void;
  setThreadRoot: (m: Message | null) => void;
  setThreadMessages: (m: Message[]) => void;
  pushNav: (entry: NavEntry) => void;
  popNav: () => NavEntry | null;
  /** Clear all workspace-scoped chat state. Called on workspace switch so
   *  ws-A's messages / channel selection / thread / nav history don't leak
   *  into ws-B's chat view. */
  resetForWorkspaceSwitch: () => void;
}

// Failed pending sticks around with line_number = -1 even after the real entry
// arrives — the daemon may have written successfully while our HTTP response
// timed out / errored. line_number-only dedup never catches it. Match by
// (author, body) instead: if a real entry with the same content shows up, the
// "Failed" copy is a stale duplicate and should be dropped.
function dropFailedPendingsMatching(
  existing: Message[],
  incoming: Message[]
): Message[] {
  if (incoming.length === 0) return existing;
  const realKeys = new Set<string>();
  for (const m of incoming) {
    if (m._pendingId) continue;
    realKeys.add(`${m.author}\u0000${m.body}`);
  }
  if (realKeys.size === 0) return existing;
  return existing.filter(
    (m) =>
      !(
        m._status === "failed" &&
        m._pendingId &&
        realKeys.has(`${m.author}\u0000${m.body}`)
      )
  );
}

function mergeIncomingByLine(
  existing: Message[],
  incoming: Message[]
): { messages: Message[]; changed: boolean } {
  const incomingByLine = new Map<number, Message>();
  for (const m of incoming) {
    if (m.line_number > 0 && !m._pendingId) {
      incomingByLine.set(m.line_number, m);
    }
  }
  if (incomingByLine.size === 0) {
    return { messages: existing, changed: false };
  }

  let changed = false;
  const messages = existing.map((current) => {
    const authoritative = incomingByLine.get(current.line_number);
    if (!authoritative) return current;

    changed = true;
    const pendingState = current._pendingId
      ? { _pendingId: current._pendingId, _status: current._status }
      : {};
    return { ...current, ...authoritative, ...pendingState };
  });

  return { messages, changed };
}

export const useChatStore = create<ChatState>((set) => ({
  connected: false,
  currentUser: "",
  isGuest: false,
  users: [],
  channels: [],
  archivedChannels: [],
  archivedChannelsView: null,
  archivedDmsView: null,
  currentChannel: null,
  messages: [],
  replyTo: null,
  highlightLine: null,
  pendingScrollLine: null,
  threadRoot: null,
  threadMessages: [],
  navHistory: [],
  hasMoreHistory: true,

  setConnected: (v) => set({ connected: v }),
  setCurrentUser: (u) => set({ currentUser: u }),
  setIsGuest: (v) => set({ isGuest: v }),
  setUsers: (u) => set({ users: u }),
  setChannels: (newChannels) =>
    set((state) => {
      const prevMap = new Map(
        state.channels.map((c) => [c.name, c])
      );
      return {
        channels: newChannels.map((c) => {
          const prev = prevMap.get(c.name);
          return {
            ...c,
            unreadCount: prev?.unreadCount || 0,
            hasMention: prev?.hasMention || false,
          };
        }),
      };
    }),

  setArchivedChannels: (newChannels) =>
    set({
      // Daemon omits unreadCount / hasMention for archived channels, so
      // synthesize defaults here — the existing Channel type requires them.
      archivedChannels: newChannels.map((c) => ({
        ...c,
        unreadCount: c.unreadCount ?? 0,
        hasMention: c.hasMention ?? false,
      })),
    }),

  resetArchivedChannelsView: (query) =>
    set({
      archivedChannelsView: {
        items: [],
        offset: 0,
        hasMore: true,
        query,
        loading: false,
        error: null,
      },
      archivedChannels: [],
    }),

  appendArchivedChannelsPage: (page) =>
    set((state) => {
      if (!state.archivedChannelsView) return {};
      const seen = new Set(
        state.archivedChannelsView.items.map((c) => c.name),
      );
      const toAppend = page.items
        .filter((c) => !seen.has(c.name))
        .map((c) => ({
          ...c,
          unreadCount: c.unreadCount ?? 0,
          hasMention: c.hasMention ?? false,
        }));
      const items = [...state.archivedChannelsView.items, ...toAppend];
      return {
        archivedChannels: items,
        archivedChannelsView: {
          ...state.archivedChannelsView,
          items,
          offset: state.archivedChannelsView.offset + page.items.length,
          hasMore: page.hasMore,
          error: null,
        },
      };
    }),

  setArchivedChannelsLoading: (v) =>
    set((state) => {
      if (!state.archivedChannelsView) return {};
      return {
        archivedChannelsView: {
          ...state.archivedChannelsView,
          loading: v,
          ...(v ? { error: null } : {}),
        },
      };
    }),

  setArchivedChannelsError: (e) =>
    set((state) => {
      if (!state.archivedChannelsView) return {};
      return {
        archivedChannelsView: {
          ...state.archivedChannelsView,
          error: e,
          loading: false,
        },
      };
    }),

  invalidateArchivedChannelsView: () =>
    set({ archivedChannels: [], archivedChannelsView: null }),

  resetArchivedDmsView: (query) =>
    set({
      archivedDmsView: {
        items: [],
        offset: 0,
        // `true` until the first response: optimism keeps the Load-more
        // button visible during the initial fetch instead of flickering.
        hasMore: true,
        query,
        loading: false,
        error: null,
      },
    }),

  appendArchivedDmsPage: (page) =>
    set((state) => {
      if (!state.archivedDmsView) return {};
      const seen = new Set(
        state.archivedDmsView.items.map((e) => e.dm_pair_stem),
      );
      const toAppend = page.items.filter((e) => !seen.has(e.dm_pair_stem));
      return {
        archivedDmsView: {
          ...state.archivedDmsView,
          items: [...state.archivedDmsView.items, ...toAppend],
          // Advance by full incoming length, *not* the dedupped count. The
          // daemon's offset cursor counts requested rows, not unique ones —
          // dropping the dedup count here would double-fetch the overlap.
          offset: state.archivedDmsView.offset + page.items.length,
          hasMore: page.hasMore,
          error: null,
        },
      };
    }),

  setArchivedDmsLoading: (v) =>
    set((state) => {
      if (!state.archivedDmsView) return {};
      return {
        archivedDmsView: {
          ...state.archivedDmsView,
          loading: v,
          // Entering loading clears any prior error so a retry doesn't
          // render both a spinner and a stale "Failed" message.
          ...(v ? { error: null } : {}),
        },
      };
    }),

  setArchivedDmsError: (e) =>
    set((state) => {
      if (!state.archivedDmsView) return {};
      return {
        archivedDmsView: { ...state.archivedDmsView, error: e, loading: false },
      };
    }),

  markChannelArchived: (name) =>
    set((state) => {
      const nextChannels = state.channels.filter((c) => c.name !== name);
      return {
        channels: nextChannels,
        archivedChannels: [],
        archivedChannelsView: null,
      };
    }),

  markChannelUnarchived: (name) =>
    set((state) => {
      const channel = state.archivedChannelsView?.items.find(
        (c) => c.name === name,
      );
      const nextItems =
        state.archivedChannelsView?.items.filter((c) => c.name !== name) ?? [];
      const nextView = state.archivedChannelsView
        ? {
            ...state.archivedChannelsView,
            items: nextItems,
          }
        : state.archivedChannelsView;
      const alreadyActive = state.channels.some((c) => c.name === name);
      // Normalize kind → "channel". Daemon tags archived items with
      // kind: "archived_channel" (runtime value outside the TS union), so
      // without this rewrite the sidebar's `kind === "channel"` filter would
      // skip the restored channel until a client.channels() refresh lands.
      const nextChannels =
        channel && !alreadyActive
          ? [...state.channels, { ...channel, kind: "channel" as const }]
          : state.channels;
      return {
        channels: nextChannels,
        archivedChannels: nextItems,
        archivedChannelsView: nextView,
      };
    }),

  markDmArchived: (name) =>
    set((state) => {
      // Invalidate the view rather than mutating it: the paginated, sorted
      // page from the server can't be patched client-side without knowing
      // every page's contents (otherwise the new entry would land at the
      // wrong position). Next expand refetches a fresh first page.
      const nextChannels = state.channels.filter((c) => c.name !== name);
      return { channels: nextChannels, archivedDmsView: null };
    }),

  markDmUnarchived: (name) =>
    set((state) => {
      const nextView = state.archivedDmsView
        ? {
            ...state.archivedDmsView,
            items: state.archivedDmsView.items.filter(
              (e) => e.dm_pair_stem !== name,
            ),
          }
        : state.archivedDmsView;
      const alreadyActive = state.channels.some((c) => c.name === name);
      if (alreadyActive) {
        return { archivedDmsView: nextView };
      }
      // Synthesize a Channel-shaped record from the stem. `members` is
      // recoverable because the on-disk filename is `<min>--<max>` of the
      // two participants; we don't have unread metadata for a freshly-
      // restored DM, so defaults are zeros and the next channel poll will
      // refresh authoritative values.
      const synthChannel: Channel = {
        name,
        kind: "dm",
        unreadCount: 0,
        hasMention: false,
        members: name.split("--"),
      };
      return {
        channels: [...state.channels, synthChannel],
        archivedDmsView: nextView,
      };
    }),

  selectChannel: (name) =>
    set({
      currentChannel: name,
      replyTo: null,
      messages: [],
      highlightLine: null,
      pendingScrollLine: null,
      threadRoot: null,
      threadMessages: [],
      hasMoreHistory: true,
    }),

  incrementUnread: (channel, mentioned) =>
    set((state) => ({
      channels: state.channels.map((c) =>
        c.name === channel
          ? {
              ...c,
              unreadCount: (c.unreadCount || 0) + 1,
              hasMention: c.hasMention || !!mentioned,
            }
          : c
      ),
    })),

  clearUnread: (channel) =>
    set((state) => ({
      channels: state.channels.map((c) =>
        c.name === channel ? { ...c, unreadCount: 0, hasMention: false } : c
      ),
    })),

  // Dedup: keep pending messages that haven't been confirmed by the new batch.
  // When newMessages is empty (e.g. channel clear), don't carry over pending —
  // they belong to the previous channel context.
  setMessages: (newMessages) =>
    set((state) => {
      if (newMessages.length === 0) return { messages: [], hasMoreHistory: true };
      const realLineNumbers = new Set(newMessages.map((m) => m.line_number));
      const reconciled = dropFailedPendingsMatching(state.messages, newMessages);
      const pendingToKeep = reconciled.filter(
        (m) => m._pendingId && !realLineNumbers.has(m.line_number)
      );
      return { messages: [...newMessages, ...pendingToKeep] };
    }),

  addMessages: (m) =>
    set((state) => {
      const reconciled = dropFailedPendingsMatching(state.messages, m);
      const merged = mergeIncomingByLine(reconciled, m);
      const existing = new Set(merged.messages.map((msg) => msg.line_number));
      const toAdd = m.filter((msg) => !existing.has(msg.line_number));
      if (toAdd.length === 0 && !merged.changed) return {};
      return { messages: [...merged.messages, ...toAdd] };
    }),

  prependMessages: (older) =>
    set((state) => {
      if (older.length === 0) return {};
      const existing = new Set(state.messages.map((m) => m.line_number));
      const toAdd = older.filter((m) => !existing.has(m.line_number));
      if (toAdd.length === 0) return {};
      // Sort the new entries ascending so the merged list stays in order
      // regardless of input ordering — callers don't have to pre-sort.
      const sorted = [...toAdd].sort((a, b) => a.line_number - b.line_number);
      return { messages: [...sorted, ...state.messages] };
    }),

  setHasMoreHistory: (v) => set({ hasMoreHistory: v }),

  addPendingMessage: (m) =>
    set((state) => ({ messages: [...state.messages, m] })),

  markPendingSent: (pendingId, lineNumber) =>
    set((state) => {
      const realAlreadyArrived = state.messages.some(
        (m) => !m._pendingId && m.line_number === lineNumber
      );
      return {
        messages: realAlreadyArrived
          ? state.messages.filter((m) => m._pendingId !== pendingId)
          : state.messages.map((m) =>
              m._pendingId === pendingId
                ? { ...m, _status: "sent", line_number: lineNumber }
                : m
            ),
      };
    }),

  markPendingFailed: (pendingId, lineNumber) =>
    set((state) => {
      // Symmetric to markPendingSent: if the real entry already showed up
      // (daemon wrote before our HTTP response failed / timed out), the send
      // actually succeeded — drop the pending instead of marking it failed.
      const pending = state.messages.find((m) => m._pendingId === pendingId);
      if (pending) {
        const realArrived = state.messages.some(
          (m) =>
            !m._pendingId &&
            m.author === pending.author &&
            m.body === pending.body
        );
        if (realArrived) {
          return {
            messages: state.messages.filter((m) => m._pendingId !== pendingId),
          };
        }
      }
      return {
        messages: state.messages.map((m) =>
          m._pendingId === pendingId
            ? {
                ...m,
                _status: "failed",
                ...(lineNumber !== undefined && { line_number: lineNumber }),
              }
            : m
        ),
      };
    }),

  removePendingMessage: (pendingId) =>
    set((state) => ({
      messages: state.messages.filter((m) => m._pendingId !== pendingId),
    })),

  setReplyTo: (m) => set({ replyTo: m }),
  setHighlightLine: (line) => set({ highlightLine: line }),
  setPendingScrollLine: (line) => set({ pendingScrollLine: line }),
  setThreadRoot: (m) => set({ threadRoot: m }),
  setThreadMessages: (m) => set({ threadMessages: m }),

  pushNav: (entry) =>
    set((state) => ({ navHistory: [...state.navHistory, entry] })),
  popNav: (): NavEntry | null => {
    const s = useChatStore.getState() as ChatState;
    if (s.navHistory.length === 0) return null;
    const last: NavEntry = s.navHistory[s.navHistory.length - 1];
    useChatStore.setState({ navHistory: s.navHistory.slice(0, -1) });
    return last;
  },

  resetForWorkspaceSwitch: () =>
    set({
      connected: false,
      currentUser: "",
      isGuest: false,
      users: [],
      channels: [],
      archivedChannels: [],
      archivedChannelsView: null,
      archivedDmsView: null,
      currentChannel: null,
      messages: [],
      replyTo: null,
      highlightLine: null,
      pendingScrollLine: null,
      threadRoot: null,
      threadMessages: [],
      navHistory: [],
      hasMoreHistory: true,
    }),
}));
