// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, useRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useScrollAtBottom } from "./use-scroll-at-bottom";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

// jsdom doesn't ship ResizeObserver. The hook treats it as optional, but we
// stub a no-op so the observe/disconnect path is exercised the same way it
// would be in a real browser.
class MockResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
Object.defineProperty(globalThis, "ResizeObserver", {
  configurable: true,
  value: MockResizeObserver,
});

interface Captured {
  atBottom: boolean;
  scrollToBottom: (behavior?: ScrollBehavior) => void;
}

function setScrollGeometry(
  el: HTMLElement,
  { scrollTop, scrollHeight, clientHeight }: {
    scrollTop: number;
    scrollHeight: number;
    clientHeight: number;
  },
) {
  Object.defineProperty(el, "scrollTop", { value: scrollTop, configurable: true, writable: true });
  Object.defineProperty(el, "scrollHeight", { value: scrollHeight, configurable: true });
  Object.defineProperty(el, "clientHeight", { value: clientHeight, configurable: true });
}

function Probe({ onState }: { onState: (s: Captured) => void }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const state = useScrollAtBottom(ref);
  onState(state);
  return <div ref={ref} data-testid="scroll" />;
}

async function mountProbe(): Promise<{
  container: HTMLDivElement;
  root: Root;
  el: HTMLDivElement;
  latest: () => Captured;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  let last: Captured | null = null;
  await act(async () => {
    root.render(<Probe onState={(s) => { last = s; }} />);
    await Promise.resolve();
  });
  const el = container.querySelector<HTMLDivElement>("[data-testid='scroll']")!;
  return {
    container,
    root,
    el,
    latest: () => last as Captured,
  };
}

async function fireScroll(el: HTMLElement) {
  await act(async () => {
    el.dispatchEvent(new Event("scroll"));
    await Promise.resolve();
  });
}

describe("useScrollAtBottom", () => {
  let root: Root | null = null;

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("starts atBottom=true and stays true when scroll position is within threshold of the bottom", async () => {
    const { root: r, el, latest } = await mountProbe();
    root = r;

    // 800 - 730 - 60 = 10  → within 80px → atBottom
    setScrollGeometry(el, { scrollTop: 730, scrollHeight: 800, clientHeight: 60 });
    await fireScroll(el);

    expect(latest().atBottom).toBe(true);
  });

  it("flips to atBottom=false once the user scrolls past the threshold", async () => {
    const { root: r, el, latest } = await mountProbe();
    root = r;

    // 1000 - 500 - 400 = 100  → past 80px → not atBottom
    setScrollGeometry(el, { scrollTop: 500, scrollHeight: 1000, clientHeight: 400 });
    await fireScroll(el);

    expect(latest().atBottom).toBe(false);
  });

  it("scrollToBottom calls scrollTo with the latest scrollHeight", async () => {
    const { root: r, el, latest } = await mountProbe();
    root = r;

    setScrollGeometry(el, { scrollTop: 0, scrollHeight: 1234, clientHeight: 400 });
    const scrollTo = vi.fn();
    el.scrollTo = scrollTo as unknown as typeof el.scrollTo;

    latest().scrollToBottom("auto");

    expect(scrollTo).toHaveBeenCalledWith({ top: 1234, behavior: "auto" });
  });
});
