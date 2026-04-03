import { create } from 'zustand';
import type { Message, Channel } from '../lib/types.js';

interface Store {
  // 连接状态
  connected: boolean;
  setConnected: (v: boolean) => void;

  // 当前用户
  currentUser: string;
  setCurrentUser: (u: string) => void;
  isGuest: boolean;
  setIsGuest: (v: boolean) => void;

  // 用户列表
  users: string[];
  setUsers: (u: string[]) => void;

  // 频道
  channels: Channel[];
  setChannels: (c: Channel[]) => void;
  currentChannel: string | null;
  selectChannel: (name: string) => void;
  incrementUnread: (channel: string) => void;
  clearUnread: (channel: string) => void;

  // 消息
  messages: Message[];
  setMessages: (m: Message[]) => void;
  addMessages: (m: Message[]) => void;
  addPendingMessage: (m: Message) => void;
  markPendingSent: (pendingId: string, lineNumber: number) => void;
  removePendingMessage: (pendingId: string) => void;
  markPendingFailed: (pendingId: string) => void;

  // 回复
  replyTo: Message | null;
  setReplyTo: (m: Message | null) => void;

  // 高亮
  highlightLine: number | null;
  setHighlightLine: (line: number | null) => void;

  // 线程面板
  threadRoot: Message | null;
  setThreadRoot: (m: Message | null) => void;
  threadMessages: Message[];
  setThreadMessages: (m: Message[]) => void;
}

export const useStore = create<Store>((set) => ({
  connected: false,
  setConnected: (v) => set({ connected: v }),

  currentUser: '',
  setCurrentUser: (u) => set({ currentUser: u }),
  isGuest: false,
  setIsGuest: (v) => set({ isGuest: v }),

  users: [],
  setUsers: (u) => set({ users: u }),

  channels: [],
  setChannels: (c) => set({ channels: c }),
  currentChannel: null,
  selectChannel: (name) => set({ currentChannel: name }),
  incrementUnread: (channel) =>
    set((s) => ({
      channels: s.channels.map((c) =>
        c.name === channel ? { ...c, unreadCount: c.unreadCount + 1 } : c,
      ),
    })),
  clearUnread: (channel) =>
    set((s) => ({
      channels: s.channels.map((c) =>
        c.name === channel ? { ...c, unreadCount: 0 } : c,
      ),
    })),

  messages: [],
  setMessages: (m) =>
    set((s) => {
      // 保留尚未匹配到真实消息的 pending 消息
      const realLineNumbers = new Set(m.map((msg) => msg.line_number));
      const pending = s.messages.filter(
        (msg) => msg._pendingId && !realLineNumbers.has(msg.line_number),
      );
      return { messages: [...m, ...pending] };
    }),
  addMessages: (m) =>
    set((s) => {
      const existing = new Set(s.messages.map((msg) => msg.line_number));
      const newMsgs = m.filter((msg) => !existing.has(msg.line_number));
      return { messages: [...s.messages, ...newMsgs] };
    }),
  addPendingMessage: (m) =>
    set((s) => ({ messages: [...s.messages, m] })),
  markPendingSent: (pendingId, lineNumber) =>
    set((s) => ({
      messages: s.messages.map((msg) =>
        msg._pendingId === pendingId
          ? { ...msg, _status: 'sent' as const, line_number: lineNumber }
          : msg,
      ),
    })),
  removePendingMessage: (pendingId) =>
    set((s) => ({
      messages: s.messages.filter((msg) => msg._pendingId !== pendingId),
    })),
  markPendingFailed: (pendingId) =>
    set((s) => ({
      messages: s.messages.map((msg) =>
        msg._pendingId === pendingId ? { ...msg, _status: 'failed' as const } : msg,
      ),
    })),

  replyTo: null,
  setReplyTo: (m) => set({ replyTo: m }),

  highlightLine: null,
  setHighlightLine: (line) => set({ highlightLine: line }),

  threadRoot: null,
  setThreadRoot: (m) => set({ threadRoot: m }),
  threadMessages: [],
  setThreadMessages: (m) => set({ threadMessages: m }),
}));
