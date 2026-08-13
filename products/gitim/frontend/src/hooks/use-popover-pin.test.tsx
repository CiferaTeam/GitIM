// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { usePopoverPin, type PopoverPin } from "./use-popover-pin";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function Probe({
  delayMs,
  onState,
}: {
  delayMs: number;
  onState: (pin: PopoverPin) => void;
}) {
  const pin = usePopoverPin(delayMs);
  onState(pin);
  return null;
}

describe("usePopoverPin", () => {
  let container: HTMLDivElement;
  let root: Root;
  let latest: PopoverPin | null;

  beforeEach(() => {
    vi.useFakeTimers();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    latest = null;
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
    vi.useRealTimers();
  });

  async function mount(delayMs = 50) {
    await act(async () => {
      root.render(
        <Probe
          delayMs={delayMs}
          onState={(pin) => {
            latest = pin;
          }}
        />,
      );
    });
  }

  it("closes an unpinned hover open after the leave delay", async () => {
    await mount();
    await act(async () => {
      latest?.openFromHover();
    });
    expect(latest?.open).toBe(true);

    await act(async () => {
      latest?.scheduleHoverClose();
      vi.advanceTimersByTime(50);
    });
    expect(latest?.open).toBe(false);
  });

  it("keeps the popover open after a content interaction even if the pointer leaves", async () => {
    await mount();
    await act(async () => {
      latest?.openFromHover();
    });
    await act(async () => {
      latest?.pinFromInteraction();
    });
    expect(latest?.pinned).toBe(true);

    await act(async () => {
      latest?.scheduleHoverClose();
      vi.advanceTimersByTime(200);
    });
    expect(latest?.open).toBe(true);
    expect(latest?.pinned).toBe(true);
  });
});
