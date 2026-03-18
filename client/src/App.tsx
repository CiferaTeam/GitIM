import { useCallback } from 'react';
import { useStore } from './hooks/useStore.js';
import { useConnection } from './hooks/useConnection.js';
import { Header } from './components/Header.js';
import { Sidebar } from './components/Sidebar.js';
import { MessageList } from './components/MessageList.js';
import { InputArea } from './components/InputArea.js';
import { ThreadPanel } from './components/ThreadPanel.js';
import type { Message } from './lib/types.js';

export function App() {
  const { request, loadMessages } = useConnection();
  const {
    currentChannel,
    currentUser,
    selectChannel,
    clearUnread,
    setMessages,
    setReplyTo,
    setThreadRoot,
    setThreadMessages,
    messages,
    addPendingMessage,
    removePendingMessage,
    markPendingFailed,
  } = useStore();

  // 切换频道
  const handleChannelSelect = useCallback(
    async (name: string) => {
      selectChannel(name);
      clearUnread(name);
      setMessages([]);
      setThreadRoot(null);
      await loadMessages(name);
    },
    [selectChannel, clearUnread, setMessages, setThreadRoot, loadMessages],
  );

  // 发送消息（乐观 UI）
  const handleSend = useCallback(
    async (body: string, pointTo: number) => {
      if (!currentChannel) {
        return { id: 0, ok: false, error: '未选择频道' };
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

      // 发送请求
      const params: Record<string, unknown> = {
        channel: currentChannel,
        body,
        author: currentUser,
      };
      if (pointTo > 0) {
        params.reply_to = pointTo;
      }

      const res = await request('send', params);
      if (res.ok) {
        // daemon 接受了，消息将通过 push 事件刷新回来，届时 pending 消息会被替换
        // 先标记为 sent
        removePendingMessage(pendingId);
      } else {
        // 发送失败，标记为 failed
        markPendingFailed(pendingId);
      }
      return res;
    },
    [currentChannel, currentUser, request, addPendingMessage, removePendingMessage, markPendingFailed],
  );

  // 回复消息
  const handleReply = useCallback(
    (msg: Message) => {
      setReplyTo(msg);
    },
    [setReplyTo],
  );

  // 显示线程
  const handleShowThread = useCallback(
    async (msg: Message) => {
      // 找到线程根：如果 point_to > 0 则用 point_to，否则用自身
      const rootLine = msg.point_to > 0 ? msg.point_to : msg.line_number;
      const rootMsg =
        msg.point_to > 0
          ? messages.find((m) => m.line_number === rootLine) ?? msg
          : msg;
      setThreadRoot(rootMsg);

      // 加载线程中所有相关消息（从当前已加载消息中过滤）
      const threadMsgs = messages.filter(
        (m) =>
          m.line_number === rootLine || m.point_to === rootLine,
      );
      setThreadMessages(threadMsgs);
    },
    [messages, setThreadRoot, setThreadMessages],
  );

  return (
    <div className="app-layout">
      <Header />
      <div className="app-body">
        <Sidebar onChannelSelect={handleChannelSelect} />
        <div className="main-content">
          <MessageList onReply={handleReply} onShowThread={handleShowThread} />
          <InputArea onSend={handleSend} />
        </div>
        <ThreadPanel onReplyInThread={handleReply} />
      </div>
    </div>
  );
}
