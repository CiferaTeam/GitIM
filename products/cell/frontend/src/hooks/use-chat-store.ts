import { create } from "zustand";
import type { Channel, Message } from "../lib/types";

interface NavEntry {
  channel: string;
  scrollTop: number;
}

interface ChatState {
  connected: boolean;
  currentUser: string;
  isGuest: boolean;
  users: string[];
  channels: Channel[];
  /** Archived channels — populated by explicit fetch, not by /im/channels poll. */
  archivedChannels: Channel[];
  currentChannel: string | null;
  messages: Message[];
  replyTo: Message | null;
  highlightLine: number | null;
  pendingScrollLine: number | null;
  threadRoot: Message | null;
  threadMessages: Message[];
  navHistory: NavEntry[];

  setConnected: (v: boolean) => void;
  setCurrentUser: (u: string) => void;
  setIsGuest: (v: boolean) => void;
  setUsers: (u: string[]) => void;
  setChannels: (c: Channel[]) => void;
  setArchivedChannels: (c: Channel[]) => void;
  /** Optimistic: move a channel from `channels` → `archivedChannels`. */
  markChannelArchived: (name: string) => void;
  /** Optimistic: move a channel from `archivedChannels` → `channels`. */
  markChannelUnarchived: (name: string) => void;
  selectChannel: (name: string) => void;
  incrementUnread: (channel: string, mentioned?: boolean) => void;
  clearUnread: (channel: string) => void;
  setMessages: (m: Message[]) => void;
  addMessages: (m: Message[]) => void;
  addPendingMessage: (m: Message) => void;
  markPendingSent: (pendingId: string, lineNumber: number) => void;
  markPendingFailed: (pendingId: string) => void;
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

export const useChatStore = create<ChatState>((set) => ({
  connected: false,
  currentUser: "",
  isGuest: false,
  users: [],
  channels: [],
  archivedChannels: [],
  currentChannel: null,
  messages: [],
  replyTo: null,
  highlightLine: null,
  pendingScrollLine: null,
  threadRoot: null,
  threadMessages: [],
  navHistory: [],

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

  markChannelArchived: (name) =>
    set((state) => {
      const channel = state.channels.find((c) => c.name === name);
      const nextChannels = state.channels.filter((c) => c.name !== name);
      const alreadyArchived = state.archivedChannels.some((c) => c.name === name);
      const nextArchived =
        channel && !alreadyArchived
          ? [...state.archivedChannels, channel]
          : state.archivedChannels;
      return { channels: nextChannels, archivedChannels: nextArchived };
    }),

  markChannelUnarchived: (name) =>
    set((state) => {
      const channel = state.archivedChannels.find((c) => c.name === name);
      const nextArchived = state.archivedChannels.filter((c) => c.name !== name);
      const alreadyActive = state.channels.some((c) => c.name === name);
      // Normalize kind → "channel". Daemon tags archived items with
      // kind: "archived_channel" (runtime value outside the TS union), so
      // without this rewrite the sidebar's `kind === "channel"` filter would
      // skip the restored channel until a client.channels() refresh lands.
      const nextChannels =
        channel && !alreadyActive
          ? [...state.channels, { ...channel, kind: "channel" as const }]
          : state.channels;
      return { channels: nextChannels, archivedChannels: nextArchived };
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
      if (newMessages.length === 0) return { messages: [] };
      const realLineNumbers = new Set(newMessages.map((m) => m.line_number));
      const pendingToKeep = state.messages.filter(
        (m) => m._pendingId && !realLineNumbers.has(m.line_number)
      );
      return { messages: [...newMessages, ...pendingToKeep] };
    }),

  addMessages: (m) =>
    set((state) => {
      const existing = new Set(state.messages.map((msg) => msg.line_number));
      const toAdd = m.filter((msg) => !existing.has(msg.line_number));
      return toAdd.length ? { messages: [...state.messages, ...toAdd] } : {};
    }),

  addPendingMessage: (m) =>
    set((state) => ({ messages: [...state.messages, m] })),

  markPendingSent: (pendingId, lineNumber) =>
    set((state) => ({
      messages: state.messages.map((m) =>
        m._pendingId === pendingId
          ? { ...m, _status: "sent", line_number: lineNumber }
          : m
      ),
    })),

  markPendingFailed: (pendingId) =>
    set((state) => ({
      messages: state.messages.map((m) =>
        m._pendingId === pendingId ? { ...m, _status: "failed" } : m
      ),
    })),

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
      currentChannel: null,
      messages: [],
      replyTo: null,
      highlightLine: null,
      pendingScrollLine: null,
      threadRoot: null,
      threadMessages: [],
      navHistory: [],
    }),
}));
