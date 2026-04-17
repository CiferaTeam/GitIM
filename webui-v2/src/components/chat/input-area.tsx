import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import type { ApiResponse } from "../../lib/types";
import { MentionPopup } from "./mention-popup";

interface InputAreaProps {
  onSend: (body: string, pointTo: number) => Promise<ApiResponse>;
}

const MAX_HEIGHT = 200;

function draftKey(channel: string) {
  return `gitim:draft:${channel}`;
}

export function InputArea({ onSend }: InputAreaProps) {
  const currentChannel = useChatStore((s) => s.currentChannel);
  const replyTo = useChatStore((s) => s.replyTo);
  const setReplyTo = useChatStore((s) => s.setReplyTo);
  const users = useChatStore((s) => s.users);
  const agents = useAgentStore((s) => s.agents);
  const isGuest = useChatStore((s) => s.isGuest);

  // Merge human users + agent handlers for @mention
  const mentionCandidates = useMemo(() => {
    const agentIds = agents.map((a) => a.id);
    const set = new Set([...users, ...agentIds]);
    return [...set];
  }, [users, agents]);

  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Mention popup state
  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionFilter, setMentionFilter] = useState("");
  const [mentionStart, setMentionStart] = useState(0);

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Restore draft when switching channels
  useEffect(() => {
    if (!currentChannel) return;
    setText(localStorage.getItem(draftKey(currentChannel)) ?? "");
  }, [currentChannel]);

  // Auto-resize textarea up to MAX_HEIGHT
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  if (!currentChannel || isGuest) return null;

  function detectMention(value: string, cursorPos: number) {
    const textBeforeCursor = value.slice(0, cursorPos);
    const match = textBeforeCursor.match(/@([\w-]*)$/);
    if (match) {
      setMentionFilter(match[1]);
      setMentionStart(match.index!);
      setMentionOpen(true);
    } else {
      setMentionOpen(false);
    }
  }

  function handleChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    const value = e.target.value;
    setText(value);
    setError(null);
    localStorage.setItem(draftKey(currentChannel), value);
    const cursor = e.target.selectionStart ?? value.length;
    detectMention(value, cursor);
  }

  async function doSend() {
    const trimmed = text.trim();
    if (!trimmed) return;

    const savedText = text;
    const savedReplyTo = replyTo;

    // Optimistic: clear immediately
    setText("");
    setReplyTo(null);
    setMentionOpen(false);
    setSending(true);
    setError(null);

    try {
      const res = await onSend(trimmed, savedReplyTo?.line_number ?? 0);
      if (!res.ok) {
        setText(savedText);
        setReplyTo(savedReplyTo);
        setError(res.error ?? "Send failed");
      } else {
        localStorage.removeItem(draftKey(currentChannel));
      }
    } catch (err) {
      setText(savedText);
      setReplyTo(savedReplyTo);
      setError(err instanceof Error ? err.message : "Send failed");
    } finally {
      setSending(false);
      textareaRef.current?.focus();
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Escape" && !mentionOpen) {
      setReplyTo(null);
      return;
    }

    // When mention popup is open, arrow/enter/tab/escape are handled by the popup's
    // global keydown listener — don't let Enter also send
    if (mentionOpen) return;

    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void doSend();
    }
  }

  function handleMentionSelect(handle: string) {
    const ta = textareaRef.current;
    if (!ta) return;

    const cursor = ta.selectionStart ?? text.length;
    // Replace from mentionStart to cursor with <@handle>
    const before = text.slice(0, mentionStart);
    const after = text.slice(cursor);
    const inserted = `<@${handle}> `;
    const newText = before + inserted + after;
    setText(newText);
    setMentionOpen(false);

    // Restore focus and cursor after render
    requestAnimationFrame(() => {
      if (!ta) return;
      ta.focus();
      const newCursor = before.length + inserted.length;
      ta.setSelectionRange(newCursor, newCursor);
    });
  }

  return (
    <div className="border-t border-border/60 px-4 py-3 shrink-0">
      {/* Reply bar */}
      {replyTo && (
        <div className="mb-2 flex items-center gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground">
          <span className="flex-1 truncate">
            <span className="font-medium">Reply to @{replyTo.author}: </span>
            {replyTo.body.length > 40
              ? replyTo.body.slice(0, 40) + "..."
              : replyTo.body}
          </span>
          <button
            onClick={() => setReplyTo(null)}
            className="ml-1 shrink-0 hover:text-foreground transition-colors text-base leading-none"
            aria-label="Clear reply"
          >
            x
          </button>
        </div>
      )}

      {/* Input wrapper — position relative for popup anchoring */}
      <div className="relative">
        {mentionOpen && (
          <MentionPopup
            users={mentionCandidates}
            filter={mentionFilter}
            onSelect={handleMentionSelect}
            onClose={() => setMentionOpen(false)}
          />
        )}

        <textarea
          ref={textareaRef}
          rows={1}
          value={text}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          disabled={sending}
          placeholder="Type a message... (Enter to send, Shift+Enter for newline)"
          className="w-full resize-none rounded-md border border-border/60 bg-muted/20 px-3 py-2 text-sm placeholder:text-muted-foreground/60 focus:outline-none focus:ring-1 focus:ring-ring/50 focus:border-ring/50 disabled:opacity-50 transition-colors overflow-y-auto"
          style={{ maxHeight: `${MAX_HEIGHT}px` }}
        />

        {sending && (
          <span className="absolute right-3 top-1/2 -translate-y-1/2 text-xs text-muted-foreground pointer-events-none">
            Sending...
          </span>
        )}
      </div>

      {error && (
        <p className="mt-1 text-xs text-destructive">{error}</p>
      )}
    </div>
  );
}
