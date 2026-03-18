import type { Message } from '../lib/types.js';
import { formatTimestamp } from '../lib/types.js';

interface MessageItemProps {
  message: Message;
  replyTarget: Message | null;
  onReply: (m: Message) => void;
  onShowThread: (m: Message) => void;
}

export function MessageItem({
  message,
  replyTarget,
  onReply,
  onShowThread,
}: MessageItemProps) {
  return (
    <div className="message-item">
      <div className="message-actions">
        <button className="message-action-btn" onClick={() => onReply(message)}>
          回复
        </button>
        <button className="message-action-btn" onClick={() => onShowThread(message)}>
          线程
        </button>
      </div>
      <div className="message-header">
        <span className="message-author">@{message.author}</span>
        <span className="message-time">{formatTimestamp(message.timestamp)}</span>
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
