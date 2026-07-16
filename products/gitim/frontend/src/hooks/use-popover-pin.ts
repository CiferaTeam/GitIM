import { useCallback, useEffect, useRef, useState } from "react";

type PopoverOpenSource = "hover" | "focus" | "click";
type PopoverCloseSource = "hover" | "dismiss" | "trigger";

/** Minimal event shape needed from React/Radix callbacks. */
interface PreventableEvent {
  preventDefault(): void;
}

const DEFAULT_HOVER_CLOSE_DELAY_MS = 180;

export interface PopoverPin {
  open: boolean;
  pinned: boolean;
  /** Cancel a pending hover close (content pointer-enter). */
  clearCloseTimer: () => void;
  /** Open from trigger wrapper pointer-enter. */
  openFromHover: () => void;
  /** Open from trigger keyboard focus, ignoring restored dismiss focus. */
  openFromFocus: () => void;
  /** Schedule a delayed close on pointer-leave unless pinned. */
  scheduleHoverClose: () => void;
  /** Radix onOpenChange funnel; programmatic closes also unpin. */
  handleOpenChange: (next: boolean) => void;
  /** Trigger click toggles the pin: pinning opens, unpinning closes. */
  handleTriggerClick: (event: PreventableEvent) => void;
  /** Suppress content autofocus for hover opens (trigger keeps focus). */
  handleOpenAutoFocus: (event: PreventableEvent) => void;
  /** Suppress focus restore for hover closes; re-arm focus-open otherwise. */
  handleCloseAutoFocus: (event: PreventableEvent) => void;
  /** Escape counts as a dismiss: unpin and close immediately. */
  handleEscapeKeyDown: () => void;
}

/**
 * Owns the open/close/pin/dismiss-restore state machine for a popover that
 * opens on hover, focus, and click (pin), and closes on hover-leave (unless
 * pinned), outside dismiss, Escape, or trigger re-click (unpin).
 *
 * Semantics:
 * - Hover/focus opens cancel any pending hover close.
 * - Hover-leave while unpinned schedules a close after the delay.
 * - Radix outside-dismiss routes through handleOpenChange; a close with no
 *   recorded source is stamped "dismiss" so focus restore behaves like Escape.
 * - Focus restore on close is suppressed only for hover closes; keyboard
 *   dismisses return focus to the trigger but must not re-open from the
 *   restored focus event (ignored for one microtask).
 */
export function usePopoverPin(
  hoverCloseDelayMs = DEFAULT_HOVER_CLOSE_DELAY_MS,
): PopoverPin {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const openSource = useRef<PopoverOpenSource | null>(null);
  const closeSource = useRef<PopoverCloseSource | null>(null);
  const ignoreRestoredTriggerFocus = useRef(false);

  const clearCloseTimer = useCallback(() => {
    if (closeTimer.current) clearTimeout(closeTimer.current);
    closeTimer.current = null;
  }, []);

  const openFromHover = useCallback(() => {
    clearCloseTimer();
    closeSource.current = null;
    openSource.current = "hover";
    setOpen(true);
  }, [clearCloseTimer]);

  const openFromFocus = useCallback(() => {
    clearCloseTimer();
    if (ignoreRestoredTriggerFocus.current) {
      ignoreRestoredTriggerFocus.current = false;
      return;
    }
    closeSource.current = null;
    if (!open) openSource.current = "focus";
    setOpen(true);
  }, [clearCloseTimer, open]);

  const scheduleHoverClose = useCallback(() => {
    clearCloseTimer();
    if (pinned) return;
    closeTimer.current = setTimeout(() => {
      closeSource.current = "hover";
      openSource.current = null;
      setOpen(false);
    }, hoverCloseDelayMs);
  }, [clearCloseTimer, pinned, hoverCloseDelayMs]);

  useEffect(() => () => clearCloseTimer(), [clearCloseTimer]);

  const handleOpenChange = useCallback(
    (next: boolean) => {
      clearCloseTimer();
      if (!next && closeSource.current === null) {
        closeSource.current = "dismiss";
      }
      setOpen(next);
      if (!next) {
        openSource.current = null;
        setPinned(false);
      }
    },
    [clearCloseTimer],
  );

  const handleTriggerClick = useCallback(
    (event: PreventableEvent) => {
      event.preventDefault();
      clearCloseTimer();
      if (pinned) {
        closeSource.current = "trigger";
        openSource.current = null;
        setPinned(false);
        setOpen(false);
      } else {
        closeSource.current = null;
        openSource.current = "click";
        setPinned(true);
        setOpen(true);
      }
    },
    [clearCloseTimer, pinned],
  );

  const handleOpenAutoFocus = useCallback((event: PreventableEvent) => {
    if (openSource.current === "hover") event.preventDefault();
  }, []);

  const handleCloseAutoFocus = useCallback((event: PreventableEvent) => {
    if (closeSource.current === "hover") {
      event.preventDefault();
    } else {
      ignoreRestoredTriggerFocus.current = true;
      queueMicrotask(() => {
        ignoreRestoredTriggerFocus.current = false;
      });
    }
    closeSource.current = null;
  }, []);

  const handleEscapeKeyDown = useCallback(() => {
    closeSource.current = "dismiss";
    openSource.current = null;
    setPinned(false);
    setOpen(false);
  }, []);

  return {
    open,
    pinned,
    clearCloseTimer,
    openFromHover,
    openFromFocus,
    scheduleHoverClose,
    handleOpenChange,
    handleTriggerClick,
    handleOpenAutoFocus,
    handleCloseAutoFocus,
    handleEscapeKeyDown,
  };
}
