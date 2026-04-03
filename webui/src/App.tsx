import { useCallback } from 'react';
import { useStore } from './hooks/useStore.js';
import { useConnection } from './hooks/useConnection.js';
import { Header } from './components/Header.js';
import { Sidebar } from './components/Sidebar.js';
import { MessageList } from './components/MessageList.js';
import { InputArea } from './components/InputArea.js';
import { ThreadPanel } from './components/ThreadPanel.js';
import type { Message } from './lib/types.js';

/** 将 sidebar 显示名转为 API channel 名：DM "alice--bob" → "dm:alice,bob" */
function toApiChannel(name: string): string {
  if (name.includes('--')) {
    const parts = name.split('--');
    return `dm:${parts[0]},${parts[1]}`;
  }
  return name;
}

export function App() {
  const { request, loadMessages } = useConnection();
  const {
    currentChannel,
    currentUser,
    channels,
    selectChannel,
    clearUnread,
    setMessages,
    setReplyTo,
    setThreadRoot,
    setThreadMessages,
    messages,
    addPendingMessage,
    markPendingSent,
    markPendingFailed,
  } = useStore();

  const isGuest = useStore((s) => s.isGuest);

  // 切换频道
  const handleChannelSelect = useCallback(
    async (name: string) => {
      selectChannel(name);
      clearUnread(name);
      setMessages([]);
      setThreadRoot(null);
      await loadMessages(toApiChannel(name));
    },
    [selectChannel, clearUnread, setMessages, setThreadRoot, loadMessages],
  );

  // 发起私信
  // DM 在 sidebar 显示为 "alice--bob" 格式，但 API 调用需要 "dm:alice,bob" 格式
  const handleStartDm = useCallback(
    async (targetUser: string) => {
      const sorted = [currentUser, targetUser].sort();
      const dmDisplayName = `${sorted[0]}--${sorted[1]}`; // sidebar 显示用
      const dmApiChannel = `dm:${sorted[0]},${sorted[1]}`; // API 调用用

      const existing = useStore.getState().channels.find((c) => c.name === dmDisplayName);
      if (existing) {
        // 已有 DM，跳转并加载（使用 API 格式）
        selectChannel(dmDisplayName);
        clearUnread(dmDisplayName);
        setMessages([]);
        setThreadRoot(null);
        await loadMessages(toApiChannel(dmDisplayName));
      } else {
        // 不存在的 DM — 选中 display name，发消息时 handleSend 会转换格式
        selectChannel(dmDisplayName);
        setMessages([]);
        setThreadRoot(null);
      }
    },
    [currentUser, selectChannel, clearUnread, setMessages, setThreadRoot, loadMessages],
  );

  // 发送消息（乐观 UI）
  const handleSend = useCallback(
    async (body: string, pointTo: number) => {
      if (!currentChannel) {
        return { ok: false, error: '未选择频道' };
      }

      // 生成临时 ID 和时间戳
      const pendingId = `pending-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const now = new Date();
      const ts = now.toISOString().replace(/[-:]/g, '').replace(/\.\d+/, '').replace('T', 'T').slice(0, 15) + 'Z';

      // 乐观插入 pending 消息
      const pendingMsg: Message = {
        line_number: -1, // 临时行号
        point_to: pointTo,
        author: currentUser,
        timestamp: ts,
        body,
        _status: 'sending',
        _pendingId: pendingId,
      };
      addPendingMessage(pendingMsg);

      // 发送请求（DM channel 需要转为 dm:h1,h2 格式）
      const params: Record<string, unknown> = {
        channel: toApiChannel(currentChannel),
        body,
        author: currentUser,
      };
      if (pointTo > 0) {
        params.reply_to = pointTo;
      }

      const res = await request('send', params);
      if (res.ok) {
        const data = res.data as Record<string, unknown>;
        const status = data?.status as string | undefined;
        const lineNumber = data?.line_number as number;

        if (status === 'commit_only') {
          // 本地提交成功但 push 失败 — 消息未到达远端
          markPendingFailed(pendingId);
          return { ok: false, error: (data?.error as string) || 'push 失败，消息仅保存在本地' };
        }

        // "pushed" 或 "committed"（无远端）均视为成功
        markPendingSent(pendingId, lineNumber);
      } else {
        markPendingFailed(pendingId);
      }
      return res;
    },
    [currentChannel, currentUser, request, addPendingMessage, markPendingSent, markPendingFailed],
  );

  // 回复消息（点击同一条消息时取消回复）
  const handleReply = useCallback(
    (msg: Message) => {
      const current = useStore.getState().replyTo;
      if (current && current.line_number === msg.line_number) {
        setReplyTo(null);
      } else {
        setReplyTo(msg);
      }
    },
    [setReplyTo],
  );

  // 显示线程
  const handleShowThread = useCallback(
    (msg: Message) => {
      // 向上追溯 point_to 链，找到真正的根消息
      let rootLine = msg.line_number;
      let current = msg;
      while (current.point_to > 0) {
        const parent = messages.find((m) => m.line_number === current.point_to);
        if (!parent) break;
        rootLine = parent.line_number;
        current = parent;
      }

      const rootMsg = messages.find((m) => m.line_number === rootLine) ?? msg;
      setThreadRoot(rootMsg);

      // BFS 收集整棵线程树
      const treeLines = new Set([rootLine]);
      let changed = true;
      while (changed) {
        changed = false;
        for (const m of messages) {
          if (!treeLines.has(m.line_number) && treeLines.has(m.point_to)) {
            treeLines.add(m.line_number);
            changed = true;
          }
        }
      }

      // 按行号排序（时间序）
      const threadMsgs = messages
        .filter((m) => treeLines.has(m.line_number))
        .sort((a, b) => a.line_number - b.line_number);
      setThreadMessages(threadMsgs);
    },
    [messages, setThreadRoot, setThreadMessages],
  );

  return (
    <div className="app-layout">
      <Header onStartDm={isGuest ? undefined : handleStartDm} />
      <div className="app-body">
        <Sidebar onChannelSelect={handleChannelSelect} onStartDm={isGuest ? undefined : handleStartDm} />
        <div className="main-content">
          <MessageList onReply={handleReply} onShowThread={handleShowThread} />
          {!isGuest && <InputArea onSend={handleSend} />}
        </div>
        <ThreadPanel onReplyInThread={handleReply} />
      </div>
    </div>
  );
}
