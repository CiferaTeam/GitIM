import { useCallback, useEffect, useState, type RefObject } from "react";

/** How close to the bottom counts as "still at the bottom".
 *  80px tolerates a fresh message's render-time height jitter plus
 *  a partial line of padding without spuriously surfacing the jump button. */
const DEFAULT_THRESHOLD_PX = 80;

function isAtBottom(el: HTMLElement, threshold: number): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= threshold;
}

/** Reports whether `ref`'s scroll container is pinned to the bottom and
 *  exposes a `scrollToBottom` helper for the jump button. Re-checks on
 *  scroll AND on content resize — without the ResizeObserver, a fresh
 *  message growing scrollHeight would leave atBottom stuck on the user's
 *  last manual scroll position. */
export function useScrollAtBottom(
  ref: RefObject<HTMLElement | null>,
  threshold: number = DEFAULT_THRESHOLD_PX,
) {
  // Start true — chats mount scrolled to the latest message.
  const [atBottom, setAtBottom] = useState(true);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    function recheck() {
      if (!el) return;
      setAtBottom(isAtBottom(el, threshold));
    }

    recheck();
    el.addEventListener("scroll", recheck, { passive: true });

    // scrollHeight is driven by the first child's layout; observing both
    // covers viewport resize AND content growth.
    let ro: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(recheck);
      ro.observe(el);
      if (el.firstElementChild) ro.observe(el.firstElementChild);
    }

    return () => {
      el.removeEventListener("scroll", recheck);
      ro?.disconnect();
    };
  }, [ref, threshold]);

  const scrollToBottom = useCallback(
    (behavior: ScrollBehavior = "smooth") => {
      const el = ref.current;
      if (!el) return;
      el.scrollTo({ top: el.scrollHeight, behavior });
    },
    [ref],
  );

  return { atBottom, scrollToBottom };
}
