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
  | { kind: "anchor"; line: number; offsetPx: number }
  | { kind: "bottom" }
  | { kind: "preserve-prepend-anchor"; heightDelta: number };

export function decideTimelineScroll({
  previous,
  next,
  scrollTop,
  clientHeight,
  pendingScrollLine,
  restoreAnchor,
  suppressAutoBottom,
  lastMessageIsOutbound,
}: {
  previous: TimelineSnapshot | null;
  next: TimelineSnapshot;
  scrollTop: number;
  clientHeight: number;
  pendingScrollLine: number | null;
  restoreAnchor?: { line: number; offsetPx: number } | null;
  suppressAutoBottom?: boolean;
  lastMessageIsOutbound: boolean;
}): ScrollDecision {
  if (pendingScrollLine !== null && next.length > 0) {
    return { kind: "line", line: pendingScrollLine };
  }

  if (next.length === 0) {
    return { kind: "none" };
  }

  const isFreshTimeline =
    !previous ||
    previous.scopeKey !== next.scopeKey ||
    previous.length === 0;

  if (isFreshTimeline) {
    if (restoreAnchor) {
      return {
        kind: "anchor",
        line: restoreAnchor.line,
        offsetPx: restoreAnchor.offsetPx,
      };
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
    if (lastMessageIsOutbound || (wasAtBottom && !suppressAutoBottom)) {
      return { kind: "bottom" };
    }
  }

  return { kind: "none" };
}
