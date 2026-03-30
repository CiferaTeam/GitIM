import { useMemo } from 'react';
import { useStore } from '../hooks/useStore.js';
import { formatTimestamp } from '../lib/types.js';
import type { Message } from '../lib/types.js';

interface ThreadPanelProps {
  onReplyInThread: (m: Message) => void;
}

export function ThreadPanel({ onReplyInThread }: ThreadPanelProps) {
  const threadRoot = useStore((s) => s.threadRoot);
  const setThreadRoot = useStore((s) => s.setThreadRoot);
  const threadMessages = useStore((s) => s.threadMessages);

  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const m of threadMessages) {
      map.set(m.line_number, m);
    }
    return map;
  }, [threadMessages]);

  if (!threadRoot) return null;

  return (
    <div className="thread-panel">
      <div className="thread-header">
        <span>线程: L{threadRoot.line_number}</span>
        <button className="thread-close-btn" onClick={() => setThreadRoot(null)}>
          ×
        </button>
      </div>
      <div className="thread-messages">
        {threadMessages.map((msg) => {
          const isRoot = msg.line_number === threadRoot.line_number;
          const parentMsg = msg.point_to > 0 ? msgByLine.get(msg.point_to) : undefined;

          return (
            <div
              key={msg.line_number}
              className={`thread-msg${isRoot ? ' thread-msg-root' : ''}`}
            >
              {parentMsg && (
                <div className="message-reply-ref">
                  <span className="reply-author">@{parentMsg.author}:</span>
                  {parentMsg.body.length > 60
                    ? parentMsg.body.slice(0, 60) + '...'
                    : parentMsg.body}
                </div>
              )}
              <div className="message-header">
                <span className="message-author">@{msg.author}</span>
                <span className="message-time">{formatTimestamp(msg.timestamp)}</span>
              </div>
              <div className="message-body">{msg.body}</div>
              <button
                className="message-action-btn"
                style={{ marginTop: 4 }}
                onClick={() => onReplyInThread(msg)}
              >
                回复
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
