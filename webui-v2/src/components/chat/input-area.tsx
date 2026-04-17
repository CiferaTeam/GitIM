import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import type { ApiResponse } from "../../lib/types";
import { MentionPopup } from "./mention-popup";
import { CornerDownLeft, X } from "lucide-react";

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

  const mentionCandidates = useMemo(() => {
    const agentIds = agents.map((a) => a.id);
    const set = new Set([...users, ...agentIds]);
    return [...set];
  }, [users, agents]);

  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionFilter, setMentionFilter] = useState("");
  const [mentionStart, setMentionStart] = useState(0);

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!currentChannel) return;
    setText(localStorage.getItem(draftKey(currentChannel!)) ?? "");
  }, [currentChannel]);

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
    localStorage.setItem(draftKey(currentChannel!), value);
    const cursor = e.target.selectionStart ?? value.length;
    detectMention(value, cursor);
  }

  async function doSend() {
    const trimmed = text.trim();
    if (!trimmed) return;

    const savedText = text;
    const savedReplyTo = replyTo;

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
        localStorage.removeItem(draftKey(currentChannel!));
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
    const before = text.slice(0, mentionStart);
    const after = text.slice(cursor);
    const inserted = `<@${handle}> `;
    const newText = before + inserted + after;
    setText(newText);
    setMentionOpen(false);

    requestAnimationFrame(() => {
      if (!ta) return;
      ta.focus();
      const newCursor = before.length + inserted.length;
      ta.setSelectionRange(newCursor, newCursor);
    });
  }

  return (
    <div className="border-t border-border bg-card/60 px-4 py-3 shrink-0">
      {/* Reply bar */}
      {replyTo && (
        <div className="mb-2 flex items-center gap-2 rounded-lg border border-border/60 bg-surface/60 px-3 py-1.5 text-xs text-text-muted">
          <span className="flex-1 truncate">
            <span className="font-medium text-foreground">Reply to @{replyTo.author}: </span>
            {replyTo.body.length > 40
              ? replyTo.body.slice(0, 40) + "..."
              : replyTo.body}
          </span>
          <button
            onClick={() => setReplyTo(null)}
            className="ml-1 shrink-0 hover:text-foreground transition-colors p-0.5 rounded hover:bg-surface-hover"
            aria-label="Clear reply"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}

      {/* Input wrapper */}
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
          className="w-full resize-none rounded-xl border border-border bg-background px-4 py-2.5 text-sm placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 disabled:opacity-50 transition-all overflow-y-auto pr-10"
          style={{ maxHeight: `${MAX_HEIGHT}px` }}
        />

        <div className="absolute right-3 top-1/2 -translate-y-1/2 flex items-center gap-1.5 pointer-events-none">
          {sending ? (
            <span className="text-xs text-text-muted">Sending...</span>
          ) : (
            <CornerDownLeft className="size-3.5 text-text-faint" />
          )}
        </div>
      </div>

      {error && (
        <p className="mt-1.5 text-xs text-destructive flex items-center gap-1">
          <span className="inline-block w-1 h-1 rounded-full bg-destructive" />
          {error}
        </p>
      )}
    </div>
  );
}
