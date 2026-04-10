import { useRef, useEffect, useMemo, useState } from "react";
import { useChatStore } from "../../hooks/use-chat-store";
import type { Message } from "../../lib/types";
import { MessageItem } from "./message-item";

interface MessageListProps {
  onReply: (msg: Message) => void;
  onShowThread: (msg: Message) => void;
}

export function MessageList({ onReply, onShowThread }: MessageListProps) {
  const messages = useChatStore((s) => s.messages);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const replyTo = useChatStore((s) => s.replyTo);
  const highlightLine = useChatStore((s) => s.highlightLine);
  const setHighlightLine = useChatStore((s) => s.setHighlightLine);

  const scrollRef = useRef<HTMLDivElement>(null);
  const prevLengthRef = useRef(messages.length);

  const [copiedLine, setCopiedLine] = useState<number | null>(null);

  // O(1) reply target lookup
  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const m of messages) {
      if (m.line_number > 0) map.set(m.line_number, m);
    }
    return map;
  }, [messages]);

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    const prev = prevLengthRef.current;
    prevLengthRef.current = messages.length;
    if (messages.length > prev && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  // Clear highlight after 1500ms
  useEffect(() => {
    if (highlightLine === null) return;
    const t = setTimeout(() => setHighlightLine(null), 1500);
    return () => clearTimeout(t);
  }, [highlightLine, setHighlightLine]);

  function handleScrollTo(lineNumber: number) {
    if (!scrollRef.current) return;
    const el = scrollRef.current.querySelector(
      `[data-line="${lineNumber}"]`
    ) as HTMLElement | null;
    if (el) {
      el.scrollIntoView({ behavior: "smooth", block: "center" });
      setHighlightLine(lineNumber);
    }
  }

  function handleCopy(body: string, lineNumber: number) {
    navigator.clipboard.writeText(body).catch(() => {});
    setCopiedLine(lineNumber);
    setTimeout(() => setCopiedLine(null), 1500);
  }

  // Empty states
  if (!currentChannel) {
    return (
      <div className="flex-1 overflow-y-auto p-4 flex items-center justify-center">
        <p className="text-muted-foreground/60 text-sm">
          Select a channel to start chatting
        </p>
      </div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto p-4 flex items-center justify-center">
        <p className="text-muted-foreground/60 text-sm">No messages yet</p>
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      className="flex-1 overflow-y-auto px-4 py-2 space-y-0.5"
    >
      {messages.map((msg) => {
        const key = msg._pendingId ?? msg.line_number;

        if (msg.type === "event") {
          return (
            <div key={key} className="flex justify-center py-2">
              <span className="text-[11px] text-muted-foreground/60 italic">
                {msg.body}
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
          />
        );
      })}
    </div>
  );
}
