import type { Message } from '../lib/types.js';
import { formatTimestamp } from '../lib/types.js';

interface MessageItemProps {
  message: Message;
  replyTarget: Message | null;
  onReply: (m: Message) => void;
  onShowThread: (m: Message) => void;
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
}: MessageItemProps) {
  const isPending = !!message._pendingId;
  const isFailed = message._status === 'failed';
  const statusText = message._status ? STATUS_LABEL[message._status] : null;

  return (
    <div className={`message-item ${isPending ? 'message-pending' : ''} ${isFailed ? 'message-failed' : ''}`}>
      {!isPending && (
        <div className="message-actions">
          <button className="message-action-btn" onClick={() => onReply(message)}>
            回复
          </button>
          <button className="message-action-btn" onClick={() => onShowThread(message)}>
            线程
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
        <div className="message-reply-ref">
          <span className="reply-author">@{replyTarget.author}:</span>
          {replyTarget.body.length > 60
            ? replyTarget.body.slice(0, 60) + '...'
            : replyTarget.body}
        </div>
      )}
      <div className="message-body">{message.body}</div>
    </div>
  );
}
