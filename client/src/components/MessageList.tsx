import { useEffect, useRef } from 'react';
import { useStore } from '../hooks/useStore.js';
import { MessageItem } from './MessageItem.js';
import type { Message } from '../lib/types.js';

interface MessageListProps {
  onReply: (m: Message) => void;
  onShowThread: (m: Message) => void;
}

export function MessageList({ onReply, onShowThread }: MessageListProps) {
  const messages = useStore((s) => s.messages);
  const currentChannel = useStore((s) => s.currentChannel);
  const listRef = useRef<HTMLDivElement>(null);
  const prevLengthRef = useRef(0);

  // 新消息自动滚动到底部
  useEffect(() => {
    if (messages.length > prevLengthRef.current && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
    prevLengthRef.current = messages.length;
  }, [messages]);

  // 切换频道时滚动到底部
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [currentChannel]);

  // 构建行号到消息的映射，用于查找回复目标
  const msgByLine = new Map<number, Message>();
  for (const m of messages) {
    msgByLine.set(m.line_number, m);
  }

  if (!currentChannel) {
    return (
      <div className="message-list-empty">选择一个频道开始聊天</div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="message-list" ref={listRef}>
        <div className="message-list-empty">暂无消息</div>
      </div>
    );
  }

  return (
    <div className="message-list" ref={listRef}>
      {messages.map((msg) => (
        <MessageItem
          key={msg.line_number}
          message={msg}
          replyTarget={msg.point_to > 0 ? msgByLine.get(msg.point_to) ?? null : null}
          onReply={onReply}
          onShowThread={onShowThread}
        />
      ))}
    </div>
  );
}
