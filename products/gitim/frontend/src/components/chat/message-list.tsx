import {
  useCallback,
  useRef,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  type RefObject,
} from "react";
import type { Message } from "../../lib/types";
import type { ChatViewportAnchor } from "../../lib/chat-ui-state";
import { MessageItem } from "./message-item";
import { MessageSquare, Hash } from "lucide-react";
import {
  SCROLL_BOTTOM_THRESHOLD_PX,
  decideTimelineScroll,
  type TimelineSnapshot,
} from "./message-scroll";

interface MessageListProps {
  messages: Message[];
  currentUser?: string | null;
  /** Identifier for the current scope (channel name, card path, etc.).
   *  null = no scope selected — show "select a channel" empty state. */
  scopeKey: string | null;
  replyTo: Message | null;
  highlightLine: number | null;
  pendingScrollLine: number | null;
  restoreAnchor?: ChatViewportAnchor | null;
  onHighlightLineChange: (line: number | null) => void;
  onPendingScrollClear: () => void;
  onViewportAnchorChange?: (anchor: ChatViewportAnchor) => void;
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

function viewportAnchorFromElement(
  scrollEl: HTMLDivElement,
): ChatViewportAnchor | null {
  const containerRect = scrollEl.getBoundingClientRect();
  const items = Array.from(
    scrollEl.querySelectorAll<HTMLElement>("[data-line]"),
  );
  for (const item of items) {
    const line = Number(item.dataset.line);
    if (!Number.isFinite(line) || line <= 0) continue;
    const rect = item.getBoundingClientRect();
    if (rect.bottom <= containerRect.top) continue;
    return {
      line: Math.floor(line),
      offsetPx: Math.max(0, Math.floor(containerRect.top - rect.top)),
    };
  }
  return null;
}

export function MessageList({
  messages,
  currentUser,
  scopeKey,
  replyTo,
  highlightLine,
  pendingScrollLine,
  restoreAnchor,
  onHighlightLineChange,
  onPendingScrollClear,
  onViewportAnchorChange,
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
  const previousTimelineRef = useRef<TimelineSnapshot | null>(null);
  const suppressAutoBottomRef = useRef(false);

  const [copiedLine, setCopiedLine] = useState<number | null>(null);

  const reportViewportAnchor = useCallback((el: HTMLDivElement | null) => {
    if (!el) return;
    const anchor = viewportAnchorFromElement(el);
    if (anchor) onViewportAnchorChange?.(anchor);
  }, [onViewportAnchorChange]);

  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const m of messages) {
      if (m.line_number > 0 && m.type !== "event") map.set(m.line_number, m);
    }
    return map;
  }, [messages]);

  const maybeLoadOlderFromTop = useCallback(
    (el: HTMLDivElement) => {
      if (!onLoadOlder) return;
      if (el.scrollTop <= SCROLL_TOP_THRESHOLD_PX) {
        onLoadOlder();
      }
    },
    [onLoadOlder],
  );

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const nextTimeline: TimelineSnapshot = {
      scopeKey,
      firstLine: messages[0]?.line_number,
      length: messages.length,
      scrollHeight: el.scrollHeight,
    };
    const previousTimeline = previousTimelineRef.current;

    // Capture the timeline before applying scroll changes so the next render
    // compares against the latest committed message set, not the latest scroll.
    previousTimelineRef.current = nextTimeline;

    const decision = decideTimelineScroll({
      previous: previousTimeline,
      next: nextTimeline,
      scrollTop: el.scrollTop,
      clientHeight: el.clientHeight,
      pendingScrollLine,
      restoreAnchor,
      suppressAutoBottom: suppressAutoBottomRef.current,
      lastMessageIsOutbound: !!messages[messages.length - 1]?._pendingId,
    });

    if (decision.kind === "line") {
      requestAnimationFrame(() => {
        if (!scrollRef.current) return;
        const target = scrollRef.current.querySelector(
          `[data-line="${decision.line}"]`,
        ) as HTMLElement | null;
        if (target) {
          target.scrollIntoView({ behavior: "smooth", block: "center" });
          reportViewportAnchor(scrollRef.current);
          onHighlightLineChange(decision.line);
        }
        onPendingScrollClear();
      });
    } else if (decision.kind === "anchor") {
      requestAnimationFrame(() => {
        if (!scrollRef.current) return;
        const target = scrollRef.current.querySelector(
          `[data-line="${decision.line}"]`,
        ) as HTMLElement | null;
        if (target) {
          target.scrollIntoView({ behavior: "auto", block: "start" });
          scrollRef.current.scrollTop += decision.offsetPx;
          suppressAutoBottomRef.current = true;
          reportViewportAnchor(scrollRef.current);
        }
      });
    } else if (decision.kind === "preserve-prepend-anchor") {
      el.scrollTop = el.scrollTop + decision.heightDelta;
      reportViewportAnchor(el);
    } else if (decision.kind === "bottom") {
      suppressAutoBottomRef.current = false;
      el.scrollTop = nextTimeline.scrollHeight;
      reportViewportAnchor(el);
    }
  }, [
    messages,
    pendingScrollLine,
    restoreAnchor,
    scopeKey,
    scrollRef,
    reportViewportAnchor,
    onHighlightLineChange,
    onPendingScrollClear,
  ]);

  function handleScrollEvent(event: React.UIEvent<HTMLDivElement>) {
    const anchor = viewportAnchorFromElement(event.currentTarget);
    if (anchor) onViewportAnchorChange?.(anchor);
    const distanceFromBottom =
      event.currentTarget.scrollHeight -
      event.currentTarget.scrollTop -
      event.currentTarget.clientHeight;
    if (distanceFromBottom <= SCROLL_BOTTOM_THRESHOLD_PX) {
      suppressAutoBottomRef.current = false;
    }
    maybeLoadOlderFromTop(event.currentTarget);
  }

  function handleWheelEvent(event: React.WheelEvent<HTMLDivElement>) {
    if (event.deltaY >= 0) return;
    maybeLoadOlderFromTop(event.currentTarget);
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
      onWheel={handleWheelEvent}
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
