import { create } from "zustand";
import type { Channel, Message } from "../lib/types";

interface ChatState {
  connected: boolean;
  currentUser: string;
  isGuest: boolean;
  users: string[];
  channels: Channel[];
  currentChannel: string | null;
  messages: Message[];
  replyTo: Message | null;
  highlightLine: number | null;
  threadRoot: Message | null;
  threadMessages: Message[];

  setConnected: (v: boolean) => void;
  setCurrentUser: (u: string) => void;
  setIsGuest: (v: boolean) => void;
  setUsers: (u: string[]) => void;
  setChannels: (c: Channel[]) => void;
  selectChannel: (name: string) => void;
  incrementUnread: (channel: string) => void;
  clearUnread: (channel: string) => void;
  setMessages: (m: Message[]) => void;
  addMessages: (m: Message[]) => void;
  addPendingMessage: (m: Message) => void;
  markPendingSent: (pendingId: string, lineNumber: number) => void;
  markPendingFailed: (pendingId: string) => void;
  removePendingMessage: (pendingId: string) => void;
  setReplyTo: (m: Message | null) => void;
  setHighlightLine: (line: number | null) => void;
  setThreadRoot: (m: Message | null) => void;
  setThreadMessages: (m: Message[]) => void;
}

export const useChatStore = create<ChatState>((set) => ({
  connected: false,
  currentUser: "",
  isGuest: false,
  users: [],
  channels: [],
  currentChannel: null,
  messages: [],
  replyTo: null,
  highlightLine: null,
  threadRoot: null,
  threadMessages: [],

  setConnected: (v) => set({ connected: v }),
  setCurrentUser: (u) => set({ currentUser: u }),
  setIsGuest: (v) => set({ isGuest: v }),
  setUsers: (u) => set({ users: u }),
  setChannels: (c) => set({ channels: c }),

  selectChannel: (name) =>
    set({ currentChannel: name, replyTo: null }),

  incrementUnread: (channel) =>
    set((state) => ({
      channels: state.channels.map((c) =>
        c.name === channel ? { ...c, unreadCount: c.unreadCount + 1 } : c
      ),
    })),

  clearUnread: (channel) =>
    set((state) => ({
      channels: state.channels.map((c) =>
        c.name === channel ? { ...c, unreadCount: 0 } : c
      ),
    })),

  // Dedup: keep pending messages that haven't been confirmed by the new batch
  setMessages: (newMessages) =>
    set((state) => {
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
  setThreadRoot: (m) => set({ threadRoot: m }),
  setThreadMessages: (m) => set({ threadMessages: m }),
}));
