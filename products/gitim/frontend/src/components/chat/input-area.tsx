import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { ApiResponse, Message } from "../../lib/types";
import { useIsMobile } from "../../hooks/use-media-query";
import { MentionPopup } from "./mention-popup";
import { CornerDownLeft, SendHorizontal, X } from "lucide-react";

interface InputAreaProps {
  /** Unique key for this input's scope — used for localStorage draft keying.
   *  Channel scope: the channel display name.
   *  Card scope: "card:<channel>/<card_id>".
   *  Pass null to hide the input (e.g. when no scope is selected). */
  scopeKey: string | null;
  replyTo: Message | null;
  onReplyToChange: (msg: Message | null) => void;
  mentionCandidates: string[];
  disabled?: boolean;
  onSend: (body: string, pointTo: number) => Promise<ApiResponse>;
  placeholder?: string;
}

const MAX_HEIGHT = 200;
const DESKTOP_ENTER_HINT = " (Enter to send, Shift+Enter for newline)";

function draftKey(scopeKey: string) {
  return `gitim:draft:${scopeKey}`;
}

function resolvedPlaceholder(placeholder: string | undefined, isMobile: boolean) {
  if (placeholder) {
    return isMobile ? placeholder.replace(DESKTOP_ENTER_HINT, "") : placeholder;
  }
  return isMobile
    ? "Type a message..."
    : `Type a message...${DESKTOP_ENTER_HINT}`;
}

export function InputArea({
  scopeKey,
  replyTo,
  onReplyToChange,
  mentionCandidates,
  disabled,
  onSend,
  placeholder,
}: InputAreaProps) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [mentionOpen, setMentionOpen] = useState(false);
  const [mentionFilter, setMentionFilter] = useState("");
  const [mentionStart, setMentionStart] = useState(0);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const isMobile = useIsMobile();

  // Restore draft when scope changes
  useEffect(() => {
    if (!scopeKey) {
      setText("");
      return;
    }
    setText(localStorage.getItem(draftKey(scopeKey)) ?? "");
  }, [scopeKey]);

  // Auto-resize textarea up to MAX_HEIGHT
  useEffect(() => {
    const ta = textareaRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  if (disabled || !scopeKey) return null;
  // After the guard above, scopeKey is non-null for the rest of render.
  const activeScopeKey: string = scopeKey;
  const canSend = text.trim().length > 0 && !sending;

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
    localStorage.setItem(draftKey(activeScopeKey), value);
    const cursor = e.target.selectionStart ?? value.length;
    detectMention(value, cursor);
  }

  async function doSend() {
    const trimmed = text.trim();
    if (!trimmed) return;

    const savedText = text;
    const savedReplyTo = replyTo;

    setText("");
    onReplyToChange(null);
    setMentionOpen(false);
    setSending(true);
    setError(null);

    try {
      const res = await onSend(trimmed, savedReplyTo?.line_number ?? 0);
      if (!res.ok) {
        setText(savedText);
        onReplyToChange(savedReplyTo);
        setError(res.error ?? "Send failed");
      } else {
        localStorage.removeItem(draftKey(activeScopeKey));
      }
    } catch (err) {
      setText(savedText);
      onReplyToChange(savedReplyTo);
      setError(err instanceof Error ? err.message : "Send failed");
    } finally {
      setSending(false);
      textareaRef.current?.focus();
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Escape" && !mentionOpen) {
      onReplyToChange(null);
      return;
    }

    if (mentionOpen) return;

    if (!isMobile && e.key === "Enter" && !e.shiftKey) {
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
      {replyTo && (
        <div className="mb-2 flex items-center gap-2 rounded-lg border border-border/60 bg-surface/60 px-3 py-1.5 text-xs text-text-muted">
          <span className="flex-1 truncate">
            <span className="font-medium text-foreground">Reply to @{replyTo.author}: </span>
            {replyTo.body.length > 40
              ? replyTo.body.slice(0, 40) + "..."
              : replyTo.body}
          </span>
          <button
            onClick={() => onReplyToChange(null)}
            className="ml-1 shrink-0 hover:text-foreground transition-colors p-0.5 rounded hover:bg-surface-hover"
            aria-label="Clear reply"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}

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
          placeholder={resolvedPlaceholder(placeholder, isMobile)}
          enterKeyHint={isMobile ? "enter" : "send"}
          className="w-full resize-none rounded-xl border border-border bg-background px-4 py-2.5 text-sm placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 disabled:opacity-50 transition-all overflow-y-auto pr-12 md:pr-10"
          style={{ maxHeight: `${MAX_HEIGHT}px` }}
        />

        {isMobile ? (
          <button
            type="button"
            onClick={() => void doSend()}
            onMouseDown={(e) => e.preventDefault()}
            disabled={!canSend}
            aria-label="Send message"
            title="Send"
            className="absolute right-2 bottom-2 flex size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground shadow-sm transition-colors hover:bg-primary/90 active:scale-95 disabled:bg-surface disabled:text-text-faint disabled:shadow-none"
          >
            <SendHorizontal className="size-4" />
          </button>
        ) : (
          <div className="absolute right-3 top-1/2 -translate-y-1/2 flex items-center gap-1.5 pointer-events-none">
            {sending ? (
              <span className="text-xs text-text-muted">Sending...</span>
            ) : (
              <CornerDownLeft className="size-3.5 text-text-faint" />
            )}
          </div>
        )}
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
