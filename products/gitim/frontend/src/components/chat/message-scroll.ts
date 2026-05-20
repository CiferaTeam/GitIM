export const SCROLL_BOTTOM_THRESHOLD_PX = 80;

export interface TimelineSnapshot {
  scopeKey: string | null;
  firstLine: number | undefined;
  length: number;
  scrollHeight: number;
}

export type ScrollDecision =
  | { kind: "none" }
  | { kind: "line"; line: number }
  | { kind: "bottom" }
  | { kind: "scroll-top"; top: number }
  | { kind: "preserve-prepend-anchor"; heightDelta: number };

export function decideTimelineScroll({
  previous,
  next,
  scrollTop,
  clientHeight,
  pendingScrollLine,
  restoreScrollTop,
  lastMessageIsOutbound,
}: {
  previous: TimelineSnapshot | null;
  next: TimelineSnapshot;
  scrollTop: number;
  clientHeight: number;
  pendingScrollLine: number | null;
  restoreScrollTop?: number | null;
  lastMessageIsOutbound: boolean;
}): ScrollDecision {
  if (pendingScrollLine !== null && next.length > 0) {
    return { kind: "line", line: pendingScrollLine };
  }

  if (next.length === 0) {
    return { kind: "none" };
  }

  if (!previous || previous.scopeKey !== next.scopeKey) {
    if (restoreScrollTop !== null && restoreScrollTop !== undefined) {
      return { kind: "scroll-top", top: restoreScrollTop };
    }
    return { kind: "bottom" };
  }

  if (
    previous.firstLine !== undefined &&
    next.firstLine !== undefined &&
    next.firstLine < previous.firstLine
  ) {
    return {
      kind: "preserve-prepend-anchor",
      heightDelta: next.scrollHeight - previous.scrollHeight,
    };
  }

  if (next.length > previous.length) {
    const wasAtBottom =
      previous.scrollHeight === 0 ||
      previous.scrollHeight - scrollTop - clientHeight <= SCROLL_BOTTOM_THRESHOLD_PX;
    if (wasAtBottom || lastMessageIsOutbound) {
      return { kind: "bottom" };
    }
  }

  return { kind: "none" };
}
