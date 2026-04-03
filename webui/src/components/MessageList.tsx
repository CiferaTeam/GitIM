import { useEffect, useRef, useMemo, useState } from 'react';
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
  const replyTo = useStore((s) => s.replyTo);
  const highlightLine = useStore((s) => s.highlightLine);
  const setHighlightLine = useStore((s) => s.setHighlightLine);
  const listRef = useRef<HTMLDivElement>(null);
  const prevLengthRef = useRef(0);

  const [copiedLine, setCopiedLine] = useState<number | null>(null);

  // 高亮自动清除
  useEffect(() => {
    if (highlightLine === null) return;
    const timer = window.setTimeout(() => {
      setHighlightLine(null);
    }, 1500);
    return () => window.clearTimeout(timer);
  }, [highlightLine, setHighlightLine]);

  // 复制状态自动清除
  useEffect(() => {
    if (copiedLine === null) return;
    const timer = window.setTimeout(() => {
      setCopiedLine(null);
    }, 1500);
    return () => window.clearTimeout(timer);
  }, [copiedLine]);

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
  const msgByLine = useMemo(
    () => new Map(messages.map((m) => [m.line_number, m])),
    [messages],
  );

  const handleScrollTo = (lineNumber: number) => {
    const el = listRef.current?.querySelector(`[data-line="${lineNumber}"]`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      setHighlightLine(lineNumber);
    }
  };

  const handleCopy = (body: string, lineNumber: number) => {
    void navigator.clipboard.writeText(body);
    setCopiedLine(lineNumber);
  };

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
      {messages.map((msg) =>
        msg.type === 'event' ? (
          <div key={msg.line_number} className="message-event" data-line={msg.line_number}>
            <span className="message-author">@{msg.author}</span>
            {' '}
            {msg.event_type === 'join' ? '加入了频道' : msg.event_type === 'leave' ? '离开了频道' : msg.event_type}
          </div>
        ) : (
          <MessageItem
            key={msg._pendingId ?? msg.line_number}
            message={msg}
            replyTarget={msg.point_to > 0 ? msgByLine.get(msg.point_to) ?? null : null}
            onReply={onReply}
            onShowThread={onShowThread}
            isReplying={replyTo?.line_number === msg.line_number}
            highlight={highlightLine === msg.line_number}
            onScrollTo={handleScrollTo}
            onCopy={handleCopy}
            copied={copiedLine === msg.line_number}
          />
        ),
      )}
    </div>
  );
}
