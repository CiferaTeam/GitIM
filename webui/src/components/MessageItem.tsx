import { useRef } from 'react';
import type { Message } from '../lib/types.js';
import { formatTimestamp } from '../lib/types.js';

interface MessageItemProps {
  message: Message;
  replyTarget: Message | null;
  onReply: (m: Message) => void;
  onShowThread: (m: Message) => void;
  isReplying: boolean;
  highlight: boolean;
  onScrollTo: (lineNumber: number) => void;
  onCopy: (body: string, lineNumber: number) => void;
  copied: boolean;
}

const STATUS_LABEL: Record<string, string> = {
  sending: '发送中...',
  sent: '已发送 ✓',
  failed: '发送失败 ✗',
};

export function MessageItem({
  message,
  replyTarget,
  onReply,
  onShowThread,
  isReplying,
  highlight,
  onScrollTo,
  onCopy,
  copied,
}: MessageItemProps) {
  const clickTimerRef = useRef<number | null>(null);

  const isSending = message._status === 'sending';
  const isSent = message._status === 'sent';
  const isFailed = message._status === 'failed';
  const isPending = isSending || isSent;
  const statusText = message._status ? STATUS_LABEL[message._status] : null;

  const handleBodyClick = () => {
    // 用户正在选择文本，不触发回复
    if (window.getSelection()?.toString().length) return;

    clickTimerRef.current = window.setTimeout(() => {
      clickTimerRef.current = null;
      onReply(message);
    }, 250);
  };

  const handleBodyDblClick = () => {
    if (clickTimerRef.current !== null) {
      window.clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
    onShowThread(message);
  };

  const classNames = [
    'message-item',
    isSending ? 'message-pending' : '',
    isSent ? 'message-sent' : '',
    isFailed ? 'message-failed' : '',
    highlight ? 'message-highlight' : '',
    isReplying ? 'message-replying' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div className={classNames} data-line={message.line_number}>
      {!isPending && (
        <div className="message-actions">
          <button
            className="message-action-btn"
            onClick={(e) => {
              e.stopPropagation();
              onReply(message);
            }}
          >
            回复
          </button>
          <button
            className="message-action-btn"
            onClick={(e) => {
              e.stopPropagation();
              onShowThread(message);
            }}
          >
            线程
          </button>
          <button
            className="message-action-btn"
            onClick={(e) => {
              e.stopPropagation();
              onCopy(message.body, message.line_number);
            }}
          >
            {copied ? '已复制' : '复制'}
          </button>
        </div>
      )}
      <div className="message-header">
        <span className="message-author">@{message.author}</span>
        <span className="message-time">{formatTimestamp(message.timestamp)}</span>
        {statusText && (
          <span className={`message-status message-status-${message._status}`}>
            {statusText}
          </span>
        )}
      </div>
      {replyTarget && (
        <div
          className="message-reply-ref reply-ref-clickable"
          onClick={(e) => {
            e.stopPropagation();
            onScrollTo(message.point_to);
          }}
        >
          <span className="reply-author">@{replyTarget.author}:</span>
          {replyTarget.body.length > 60
            ? replyTarget.body.slice(0, 60) + '...'
            : replyTarget.body}
        </div>
      )}
      <div
        className="message-body"
        onClick={handleBodyClick}
        onDoubleClick={handleBodyDblClick}
      >
        {message.body}
      </div>
    </div>
  );
}
