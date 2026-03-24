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

  if (!threadRoot) return null;

  // 构建树：根消息 + 直接回复
  const root = threadMessages.find(
    (m) => m.line_number === threadRoot.line_number,
  ) ?? threadRoot;
  const replies = threadMessages.filter(
    (m) => m.point_to === threadRoot.line_number,
  );

  return (
    <div className="thread-panel">
      <div className="thread-header">
        <span>引用链: L{threadRoot.line_number}</span>
        <button className="thread-close-btn" onClick={() => setThreadRoot(null)}>
          ×
        </button>
      </div>
      <div className="thread-messages">
        {/* 根消息 */}
        <div className="thread-msg">
          <div className="message-header">
            <span className="message-author">@{root.author}</span>
            <span className="message-time">{formatTimestamp(root.timestamp)}</span>
          </div>
          <div className="message-body">{root.body}</div>
        </div>
        {/* 回复列表 */}
        {replies.map((msg) => (
          <div key={msg.line_number} className="thread-msg thread-msg-indent">
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
        ))}
      </div>
    </div>
  );
}
