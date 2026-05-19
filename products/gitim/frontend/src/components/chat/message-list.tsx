import { useRef, useEffect, useLayoutEffect, useMemo, useState, type RefObject } from "react";
import type { Message } from "../../lib/types";
import { MessageItem } from "./message-item";
import { MessageSquare, Hash } from "lucide-react";

interface MessageListProps {
  messages: Message[];
  currentUser?: string | null;
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
  /** Optional external ref to the scroll container. When provided, the parent
   *  can drive scroll behavior (e.g. the jump-to-latest button). When omitted,
   *  an internal ref is used so existing tests stay green. */
  scrollRef?: RefObject<HTMLDivElement | null>;

  onReply: (msg: Message) => void;
  onShowThread: (msg: Message) => void;
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
  onActionSheet?: (msg: Message) => void;
  /** Fired when the user scrolls within `SCROLL_TOP_THRESHOLD_PX` of the top.
   *  Caller is responsible for fetching older messages and prepending them
   *  via the chat store; this component only reports the trigger. Optional —
   *  card/thread views don't paginate. */
  onLoadOlder?: () => void;
}

/** How close to the top counts as "the user is asking for more history."
 *  Anything beyond this is regarded as still browsing the current page. */
const SCROLL_TOP_THRESHOLD_PX = 50;

/** How close to the bottom counts as "user is still pinned to latest" when
 *  deciding whether a new appended message should drag them down. Matches
 *  useScrollAtBottom's default so the jump-button visibility and the
 *  auto-scroll decision agree. */
const SCROLL_BOTTOM_THRESHOLD_PX = 80;

export function MessageList({
  messages,
  currentUser,
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
  onActionSheet,
  onLoadOlder,
  scrollRef: externalScrollRef,
}: MessageListProps) {
  const internalScrollRef = useRef<HTMLDivElement>(null);
  const scrollRef = externalScrollRef ?? internalScrollRef;
  // Track first line number, list length, and pre-mutation scrollHeight so we
  // can distinguish prepend (older history arrived at the head) from append
  // (anything growing the list — new live message, pending placeholder, poll
  // delivering newer entries) and adjust scroll position appropriately.
  //
  // Why head-line for prepend but length for append: pending outbound
  // messages live at the tail with line_number = -1, so a line-number-based
  // append detector ("last line grew") would miss them and the user's just-
  // sent message would scroll off-screen. Length is the right append signal
  // because pending always grows the array. Prepend, conversely, is unique
  // in that the head shrinks — length grows too, so we must check the
  // prepend signal first and bail out before the append branch.
  const prevFirstLineRef = useRef<number | undefined>(undefined);
  const prevLengthRef = useRef(0);
  const prevScrollHeightRef = useRef(0);

  const [copiedLine, setCopiedLine] = useState<number | null>(null);

  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const m of messages) {
      if (m.line_number > 0 && m.type !== "event") map.set(m.line_number, m);
    }
    return map;
  }, [messages]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const newFirstLine = messages[0]?.line_number;
    const newLength = messages.length;
    const prevFirstLine = prevFirstLineRef.current;
    const prevLength = prevLengthRef.current;
    const prevScrollHeight = prevScrollHeightRef.current;
    const newScrollHeight = el.scrollHeight;

    // Update refs for the next effect cycle BEFORE any early return so
    // subsequent decisions compare against the most recent state.
    prevFirstLineRef.current = newFirstLine;
    prevLengthRef.current = newLength;
    prevScrollHeightRef.current = newScrollHeight;

    if (pendingScrollLine !== null && newLength > 0) {
      requestAnimationFrame(() => {
        if (!scrollRef.current) return;
        const target = scrollRef.current.querySelector(
          `[data-line="${pendingScrollLine}"]`,
        ) as HTMLElement | null;
        if (target) {
          target.scrollIntoView({ behavior: "smooth", block: "center" });
          onHighlightLineChange(pendingScrollLine);
        }
        onPendingScrollClear();
      });
      return;
    }

    // Prepend: older messages arrived at the head — preserve the visual
    // anchor by adding the height delta to scrollTop so the message the
    // user was looking at stays put. Check this BEFORE the length-based
    // append branch because a prepend also grows the list.
    if (
      prevFirstLine !== undefined &&
      newFirstLine !== undefined &&
      newFirstLine < prevFirstLine
    ) {
      el.scrollTop = el.scrollTop + (newScrollHeight - prevScrollHeight);
      return;
    }

    // Append: anything growing the list at the tail — new live message,
    // pending placeholder (line_number = -1), poll delivering newer entries.
    // Length-based detection catches all three; line-number-based detection
    // would miss the pending case and the user's outbound message would
    // scroll off-screen.
    //
    // Only drag the user to the bottom if they were already pinned there;
    // if they've scrolled up to read history, let the new message accumulate
    // off-screen and surface the jump-to-latest button instead. Outbound
    // pending messages always win — pressing Enter is an explicit "I want
    // to see what I just sent" signal.
    if (newLength > prevLength) {
      const wasAtBottom =
        prevScrollHeight === 0 || // first render with messages → snap to latest
        prevScrollHeight - el.scrollTop - el.clientHeight <= SCROLL_BOTTOM_THRESHOLD_PX;
      const lastMsg = messages[messages.length - 1];
      const isOutbound = !!lastMsg?._pendingId;
      if (wasAtBottom || isOutbound) {
        el.scrollTop = newScrollHeight;
      }
    }
  }, [messages, pendingScrollLine, onHighlightLineChange, onPendingScrollClear]);

  function handleScrollEvent(event: React.UIEvent<HTMLDivElement>) {
    if (!onLoadOlder) return;
    if (event.currentTarget.scrollTop <= SCROLL_TOP_THRESHOLD_PX) {
      onLoadOlder();
    }
  }

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
      <div className="flex-1 overflow-y-auto p-6 flex items-center justify-center">
        <div className="text-center space-y-4 max-w-xs">
          <div className="w-12 h-12 rounded-2xl bg-surface flex items-center justify-center mx-auto border border-border">
            <MessageSquare className="size-6 text-primary" />
          </div>
          <div>
            <p className="text-foreground font-medium">
              {noScopeHint ?? "Select a channel"}
            </p>
            <p className="text-sm text-text-muted mt-1">
              Choose a channel or DM from the sidebar to start chatting
            </p>
          </div>
        </div>
      </div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="flex-1 overflow-y-auto p-6 flex items-center justify-center">
        <div className="text-center space-y-4 max-w-sm">
          <div className="w-12 h-12 rounded-2xl bg-surface flex items-center justify-center mx-auto border border-border">
            <Hash className="size-6 text-primary" />
          </div>
          <div>
            <p className="text-foreground font-medium">
              {scopeKey.startsWith("card:") ? "Card discussion" : `#${scopeKey}`}
            </p>
            <p className="text-sm text-text-muted mt-2">
              {emptyHint ?? "No messages yet. Send the first message to get started."}
            </p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div
      ref={scrollRef}
      data-message-scroll
      className="flex-1 overflow-y-auto px-4 py-3 space-y-1"
      onScroll={handleScrollEvent}
    >
      {messages.map((msg) => {
        const key = msg._pendingId ?? msg.line_number;

        if (msg.type === "event") {
          const targets = msg.meta?.targets ?? [];
          let eventText: string;
          if (msg.event_type === "join") {
            eventText =
              targets.length > 0
                ? `@${msg.author} added ${targets.map((t) => `@${t}`).join(", ")}`
                : `@${msg.author} joined the channel`;
          } else if (msg.event_type === "leave") {
            eventText =
              targets.length > 0
                ? `@${msg.author} removed ${targets.map((t) => `@${t}`).join(", ")}`
                : `@${msg.author} left the channel`;
          } else {
            eventText = msg.body ?? `${msg.author}: ${msg.event_type}`;
          }
          return (
            <div key={key} className="flex justify-center py-2">
              <span className="text-[11px] text-text-muted/70 italic bg-surface/40 px-2 py-0.5 rounded-full">
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
            currentUser={currentUser}
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
            onActionSheet={onActionSheet}
          />
        );
      })}
    </div>
  );
}
