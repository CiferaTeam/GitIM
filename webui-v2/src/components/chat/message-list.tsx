import { useRef, useEffect, useMemo, useState } from "react";
import type { Message } from "../../lib/types";
import { MessageItem } from "./message-item";

interface MessageListProps {
  messages: Message[];
  /** Identifier for the current scope (channel name, card path, etc.).
   *  null = no scope selected — show "select a channel" empty state. */
  scopeKey: string | null;
  replyTo: Message | null;
  highlightLine: number | null;
  pendingScrollLine: number | null;
  onHighlightLineChange: (line: number | null) => void;
  onPendingScrollClear: () => void;
  /** Custom empty-state hint when scope is selected but has no messages. */
  emptyHint?: string;
  /** Custom empty-state hint when scope is null. */
  noScopeHint?: string;

  onReply: (msg: Message) => void;
  onShowThread: (msg: Message) => void;
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
}

export function MessageList({
  messages,
  scopeKey,
  replyTo,
  highlightLine,
  pendingScrollLine,
  onHighlightLineChange,
  onPendingScrollClear,
  emptyHint,
  noScopeHint,
  onReply,
  onShowThread,
  onMentionClick,
  onChannelClick,
  onMessageLinkClick,
  onUserProfileClick,
}: MessageListProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const prevLengthRef = useRef(messages.length);

  const [copiedLine, setCopiedLine] = useState<number | null>(null);

  // O(1) reply target lookup — only real messages, never events (events have no body)
  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const m of messages) {
      if (m.line_number > 0 && m.type !== "event") map.set(m.line_number, m);
    }
    return map;
  }, [messages]);

  // Auto-scroll to bottom when new messages arrive, UNLESS we have a pending scroll target
  useEffect(() => {
    const prev = prevLengthRef.current;
    prevLengthRef.current = messages.length;

    if (pendingScrollLine !== null && messages.length > 0) {
      requestAnimationFrame(() => {
        if (!scrollRef.current) return;
        const el = scrollRef.current.querySelector(
          `[data-line="${pendingScrollLine}"]`
        ) as HTMLElement | null;
        if (el) {
          el.scrollIntoView({ behavior: "smooth", block: "center" });
          onHighlightLineChange(pendingScrollLine);
        }
        onPendingScrollClear();
      });
      return;
    }

    if (messages.length > prev && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages, pendingScrollLine, onHighlightLineChange, onPendingScrollClear]);

  // Clear highlight after 1500ms
  useEffect(() => {
    if (highlightLine === null) return;
    const t = setTimeout(() => onHighlightLineChange(null), 1500);
    return () => clearTimeout(t);
  }, [highlightLine, onHighlightLineChange]);

  function handleScrollTo(lineNumber: number) {
    if (!scrollRef.current) return;
    const el = scrollRef.current.querySelector(
      `[data-line="${lineNumber}"]`
    ) as HTMLElement | null;
    if (el) {
      el.scrollIntoView({ behavior: "smooth", block: "center" });
      onHighlightLineChange(lineNumber);
    }
  }

  function handleCopy(body: string, lineNumber: number) {
    navigator.clipboard.writeText(body).catch(() => {});
    setCopiedLine(lineNumber);
    setTimeout(() => setCopiedLine(null), 1500);
  }

  if (!scopeKey) {
    return (
      <div className="flex-1 overflow-y-auto p-4 flex items-center justify-center">
        <p className="text-muted-foreground/60 text-sm">
          {noScopeHint ?? "Select a channel to start chatting"}
        </p>
      </div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto p-4 flex items-center justify-center">
        <p className="text-muted-foreground/60 text-sm">
          {emptyHint ?? "No messages yet"}
        </p>
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      data-message-scroll
      className="flex-1 overflow-y-auto px-4 py-2 space-y-0.5"
    >
      {messages.map((msg) => {
        const key = msg._pendingId ?? msg.line_number;

        if (msg.type === "event") {
          const eventText =
            msg.event_type === "join"
              ? `@${msg.author} joined the channel`
              : msg.event_type === "leave"
                ? `@${msg.author} left the channel`
                : msg.body ?? `${msg.author}: ${msg.event_type}`;
          return (
            <div key={key} className="flex justify-center py-2">
              <span className="text-[11px] text-muted-foreground/60 italic">
                {eventText}
              </span>
            </div>
          );
        }

        const replyTarget = msg.point_to > 0 ? (msgByLine.get(msg.point_to) ?? null) : null;

        return (
          <MessageItem
            key={key}
            message={msg}
            replyTarget={replyTarget}
            onReply={onReply}
            onShowThread={onShowThread}
            isReplying={replyTo?.line_number === msg.line_number}
            highlight={highlightLine === msg.line_number}
            onScrollTo={handleScrollTo}
            onCopy={handleCopy}
            copied={copiedLine === msg.line_number}
            onMentionClick={onMentionClick}
            onChannelClick={onChannelClick}
            onMessageLinkClick={onMessageLinkClick}
            onUserProfileClick={onUserProfileClick}
          />
        );
      })}
    </div>
  );
}
