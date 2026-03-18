import { useState, useRef, useCallback } from 'react';
import { useStore } from '../hooks/useStore.js';
import { MentionPopup } from './MentionPopup.js';
import type { WsResponse } from '../lib/types.js';

interface InputAreaProps {
  onSend: (body: string, pointTo: number) => Promise<WsResponse>;
}

export function InputArea({ onSend }: InputAreaProps) {
  const [text, setText] = useState('');
  const [error, setError] = useState('');
  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionFilter, setMentionFilter] = useState('');
  const [mentionStart, setMentionStart] = useState(-1);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const replyTo = useStore((s) => s.replyTo);
  const setReplyTo = useStore((s) => s.setReplyTo);
  const users = useStore((s) => s.users);
  const currentChannel = useStore((s) => s.currentChannel);

  const handleSend = useCallback(async () => {
    const body = text.trim();
    if (!body || !currentChannel) return;

    setError('');
    const res = await onSend(body, replyTo?.line_number ?? 0);
    if (res.ok) {
      setText('');
      setReplyTo(null);
    } else {
      setError(res.error || '发送失败');
    }
  }, [text, currentChannel, replyTo, onSend, setReplyTo]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // 提及弹窗打开时不处理 Enter
    if (mentionOpen) return;

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void handleSend();
    }
  };

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const val = e.target.value;
    setText(val);

    // 检测 @ 触发提及
    const cursorPos = e.target.selectionStart;
    const textBefore = val.slice(0, cursorPos);
    const atMatch = textBefore.match(/@([\w-]*)$/);

    if (atMatch) {
      setMentionOpen(true);
      setMentionFilter(atMatch[1]);
      setMentionStart(cursorPos - atMatch[0].length);
    } else {
      setMentionOpen(false);
    }
  };

  const handleMentionSelect = (handle: string) => {
    // 替换 @xxx 为 <@handle>
    const before = text.slice(0, mentionStart);
    const cursorPos = textareaRef.current?.selectionStart ?? text.length;
    const after = text.slice(cursorPos);
    const newText = `${before}<@${handle}>${after}`;
    setText(newText);
    setMentionOpen(false);

    // 恢复焦点
    requestAnimationFrame(() => {
      const pos = before.length + handle.length + 3; // <@ + handle + >
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(pos, pos);
    });
  };

  if (!currentChannel) return null;

  return (
    <div className="input-area">
      {replyTo && (
        <div className="input-reply-bar">
          <span>
            回复 @{replyTo.author}:{' '}
            {replyTo.body.length > 40 ? replyTo.body.slice(0, 40) + '...' : replyTo.body}
          </span>
          <button className="input-reply-close" onClick={() => setReplyTo(null)}>
            ×
          </button>
        </div>
      )}
      <div className="input-wrapper">
        {mentionOpen && (
          <MentionPopup
            users={users}
            filter={mentionFilter}
            onSelect={handleMentionSelect}
            onClose={() => setMentionOpen(false)}
          />
        )}
        <textarea
          ref={textareaRef}
          className="input-textarea"
          placeholder="输入消息... (Enter 发送, Shift+Enter 换行)"
          value={text}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          rows={1}
        />
      </div>
      {error && <div className="input-error">{error}</div>}
    </div>
  );
}
