import { create } from 'zustand';
import type { Message, Channel } from '../lib/types.js';

interface Store {
  // 连接状态
  connected: boolean;
  setConnected: (v: boolean) => void;

  // 当前用户
  currentUser: string;
  setCurrentUser: (u: string) => void;

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

  // 回复
  replyTo: Message | null;
  setReplyTo: (m: Message | null) => void;

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
  setMessages: (m) => set({ messages: m }),
  addMessages: (m) =>
    set((s) => {
      // 去重合并
      const existing = new Set(s.messages.map((msg) => msg.line_number));
      const newMsgs = m.filter((msg) => !existing.has(msg.line_number));
      return { messages: [...s.messages, ...newMsgs] };
    }),

  replyTo: null,
  setReplyTo: (m) => set({ replyTo: m }),

  threadRoot: null,
  setThreadRoot: (m) => set({ threadRoot: m }),
  threadMessages: [],
  setThreadMessages: (m) => set({ threadMessages: m }),
}));
