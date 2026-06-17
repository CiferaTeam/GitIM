// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { Message } from "../../lib/types";
import { MessageList } from "./message-list";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function msg(line: number, body: string): Message {
  return {
    line_number: line,
    point_to: 0,
    author: "alice",
    timestamp: "20260511T120000Z",
    body,
  };
}

async function renderList(
  props: Partial<Parameters<typeof MessageList>[0]> = {},
): Promise<{ container: HTMLDivElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <MessageList
        messages={props.messages ?? [msg(1, "a"), msg(2, "b"), msg(3, "c")]}
        scopeKey="general"
        replyTo={null}
        highlightLine={null}
        pendingScrollLine={null}
        onHighlightLineChange={() => {}}
        onPendingScrollClear={() => {}}
        onReply={() => {}}
        onShowThread={() => {}}
        {...props}
      />,
    );
    await Promise.resolve();
  });
  return { container, root };
}

function fireScroll(container: HTMLElement, scrollTop: number) {
  const el = container.querySelector<HTMLDivElement>("[data-message-scroll]");
  if (!el) throw new Error("scroll container not rendered");
  Object.defineProperty(el, "scrollTop", { value: scrollTop, configurable: true });
  el.dispatchEvent(new Event("scroll"));
}

describe("MessageList scroll-to-top history trigger", () => {
  let root: Root | null = null;

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("fires onLoadOlder when scrollTop reaches the near-top threshold", async () => {
    const onLoadOlder = vi.fn();
    const rendered = await renderList({ onLoadOlder });
    root = rendered.root;

    await act(async () => {
      fireScroll(rendered.container, 30);
      await Promise.resolve();
    });

    expect(onLoadOlder).toHaveBeenCalledTimes(1);
  });

  it("does not fire onLoadOlder when scrollTop is past the threshold", async () => {
    const onLoadOlder = vi.fn();
    const rendered = await renderList({ onLoadOlder });
    root = rendered.root;

    await act(async () => {
      fireScroll(rendered.container, 200);
      await Promise.resolve();
    });

    expect(onLoadOlder).not.toHaveBeenCalled();
  });

  it("fires onLoadOlder at scrollTop=0 (true top)", async () => {
    const onLoadOlder = vi.fn();
    const rendered = await renderList({ onLoadOlder });
    root = rendered.root;

    await act(async () => {
      fireScroll(rendered.container, 0);
      await Promise.resolve();
    });

    expect(onLoadOlder).toHaveBeenCalledTimes(1);
  });

  it("does not load older history from scroll events caused by programmatic line positioning", async () => {
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    const originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      value: vi.fn(),
      configurable: true,
    });
    const onLoadOlder = vi.fn();

    try {
      const rendered = await renderList({
        messages: [msg(88, "first unread"), msg(89, "next")],
        pendingScrollLine: 88,
        onLoadOlder,
      });
      root = rendered.root;

      await act(async () => {
        fireScroll(rendered.container, 0);
        await Promise.resolve();
      });
      await act(async () => {
        fireScroll(rendered.container, 0);
        await Promise.resolve();
      });

      expect(onLoadOlder).not.toHaveBeenCalled();
    } finally {
      if (originalScrollIntoView) {
        Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
          value: originalScrollIntoView,
          configurable: true,
        });
      } else {
        delete (HTMLElement.prototype as { scrollIntoView?: unknown })
          .scrollIntoView;
      }
      vi.unstubAllGlobals();
    }
  });

  it("fires onLoadOlder on an upward wheel gesture even when the list cannot scroll", async () => {
    const onLoadOlder = vi.fn();
    const rendered = await renderList({ onLoadOlder });
    root = rendered.root;

    const el = rendered.container.querySelector<HTMLDivElement>(
      "[data-message-scroll]",
    );
    expect(el).not.toBeNull();
    Object.defineProperty(el!, "scrollTop", {
      value: 0,
      configurable: true,
    });
    Object.defineProperty(el!, "scrollHeight", {
      value: 320,
      configurable: true,
    });
    Object.defineProperty(el!, "clientHeight", {
      value: 640,
      configurable: true,
    });

    await act(async () => {
      el!.dispatchEvent(
        new WheelEvent("wheel", { deltaY: -120, bubbles: true }),
      );
      await Promise.resolve();
    });

    expect(onLoadOlder).toHaveBeenCalledTimes(1);
  });

  it("does not fire onLoadOlder on a downward wheel gesture at the top", async () => {
    const onLoadOlder = vi.fn();
    const rendered = await renderList({ onLoadOlder });
    root = rendered.root;

    const el = rendered.container.querySelector<HTMLDivElement>(
      "[data-message-scroll]",
    );
    expect(el).not.toBeNull();
    Object.defineProperty(el!, "scrollTop", {
      value: 0,
      configurable: true,
    });

    await act(async () => {
      el!.dispatchEvent(
        new WheelEvent("wheel", { deltaY: 120, bubbles: true }),
      );
      await Promise.resolve();
    });

    expect(onLoadOlder).not.toHaveBeenCalled();
  });

  it("does not throw when onLoadOlder is not provided and user scrolls", async () => {
    const rendered = await renderList(); // no onLoadOlder prop
    root = rendered.root;

    await act(async () => {
      fireScroll(rendered.container, 10);
      await Promise.resolve();
    });

    // Reaching this line means no throw. Sanity check the list renders.
    expect(rendered.container.querySelector("[data-message-scroll]")).not.toBeNull();
  });

  it("shows an ephemeral acknowledgement box for messages with recipients", async () => {
    const rendered = await renderList({
      messages: [
        {
          ...msg(1, "需要确认一下通知范围"),
          recipients: ["lewis", "flame4"],
        },
      ],
    });
    root = rendered.root;

    const receipt = rendered.container.querySelector("[data-message-receipt]");
    expect(receipt?.textContent).toContain("🫡");
    expect(receipt?.textContent).toContain("@lewis");
    expect(receipt?.textContent).toContain("@flame4");
  });

  it("filters the current human user from acknowledgement recipients", async () => {
    const rendered = await renderList({
      currentUser: "lewis",
      messages: [
        {
          ...msg(1, "需要确认一下通知范围"),
          recipients: ["lewis", "flame4"],
        },
      ],
    });
    root = rendered.root;

    const receipt = rendered.container.querySelector("[data-message-receipt]");
    expect(receipt?.textContent).toContain("🫡");
    expect(receipt?.textContent).not.toContain("@lewis");
    expect(receipt?.textContent).toContain("@flame4");
  });

  it("renders the line number under the avatar instead of before the message text", async () => {
    const rendered = await renderList({
      messages: [msg(42, "first line\nsecond line")],
    });
    root = rendered.root;

    const item = rendered.container.querySelector<HTMLElement>("[data-line='42']");
    const avatarColumn = item?.querySelector("[data-message-avatar-column]");
    const lineBadge = avatarColumn?.querySelector("[data-message-line-badge]");
    const body = item?.querySelector("[data-message-body]");

    expect(lineBadge?.textContent).toBe("L42");
    expect(body?.textContent?.startsWith("L42")).toBe(false);
    expect(body?.textContent).toContain("first line");
  });
});

// ---------------------------------------------------------------------------
// Scroll position adjustments on message list mutations.
//
// jsdom doesn't compute real layout, so scrollHeight is stubbed via
// Object.defineProperty and we override it between renders to simulate the
// height change a prepend or append would cause in a real browser.
// ---------------------------------------------------------------------------

function pendingMsg(): Message {
  return {
    line_number: -1,
    point_to: 0,
    author: "alice",
    timestamp: "20260511T120000Z",
    body: "outbound",
    _pendingId: "pending-1",
    _status: "sending",
  };
}

function stubScrollHeight(el: HTMLElement, value: number) {
  Object.defineProperty(el, "scrollHeight", {
    value,
    configurable: true,
  });
}

function stubClientHeight(el: HTMLElement, value: number) {
  Object.defineProperty(el, "clientHeight", {
    value,
    configurable: true,
  });
}

function stubScrollTop(el: HTMLElement, value: number) {
  Object.defineProperty(el, "scrollTop", {
    value,
    writable: true,
    configurable: true,
  });
}

function stubRect(el: HTMLElement, top: number, bottom: number) {
  Object.defineProperty(el, "getBoundingClientRect", {
    value: () => ({
      top,
      bottom,
      left: 0,
      right: 0,
      width: 0,
      height: bottom - top,
      x: 0,
      y: top,
      toJSON: () => ({}),
    }),
    configurable: true,
  });
}

async function rerender(
  root: Root,
  props: Parameters<typeof MessageList>[0],
) {
  await act(async () => {
    root.render(<MessageList {...props} />);
    await Promise.resolve();
  });
}

describe("MessageList scroll position on message mutations", () => {
  let root: Root | null = null;

  const baseProps = {
    scopeKey: "general" as const,
    replyTo: null,
    highlightLine: null,
    pendingScrollLine: null,
    onHighlightLineChange: () => {},
    onPendingScrollClear: () => {},
    onReply: () => {},
    onShowThread: () => {},
  };

  /** Render once with a single placeholder so the scroll div exists, stub
   *  its scrollHeight to `initialHeight`, then rerender with `messages`. The
   *  layout effect's append branch fires on the second render and reads the
   *  stubbed height, so prevScrollHeightRef captures `initialHeight` for any
   *  subsequent rerender that drives the prepend / append math.
   *  (We can't use messages=[] for the priming render because the empty
   *  branch in MessageList renders a different DOM tree without
   *  [data-message-scroll].) */
  async function mountWithHeight(
    r: Root,
    messages: Message[],
    initialHeight: number,
  ): Promise<HTMLDivElement> {
    await act(async () => {
      r.render(
        <MessageList {...baseProps} messages={[msg(0, "__placeholder__")]} />,
      );
      await Promise.resolve();
    });
    const scroll = document.querySelector<HTMLDivElement>(
      "[data-message-scroll]",
    )!;
    stubScrollHeight(scroll, initialHeight);
    await act(async () => {
      r.render(<MessageList {...baseProps} messages={messages} />);
      await Promise.resolve();
    });
    return scroll;
  }

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("scrolls to the bottom on first load with messages", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    const scroll = await mountWithHeight(
      r,
      [msg(1, "a"), msg(2, "b"), msg(3, "c")],
      600,
    );

    expect(scroll.scrollTop).toBe(600);
  });

  it("reports the first visible message as a line anchor instead of a raw scrollTop", async () => {
    const onViewportAnchorChange = vi.fn();
    const rendered = await renderList({
      messages: [msg(10, "a"), msg(11, "b"), msg(12, "c")],
      onViewportAnchorChange,
    });
    root = rendered.root;

    const scroll = rendered.container.querySelector<HTMLDivElement>(
      "[data-message-scroll]",
    )!;
    stubRect(scroll, 100, 500);
    const line10 = scroll.querySelector<HTMLElement>("[data-line='10']")!;
    const line11 = scroll.querySelector<HTMLElement>("[data-line='11']")!;
    const line12 = scroll.querySelector<HTMLElement>("[data-line='12']")!;
    stubRect(line10, 40, 90);
    stubRect(line11, 80, 130);
    stubRect(line12, 140, 190);

    await act(async () => {
      fireScroll(rendered.container, 120);
      await Promise.resolve();
    });

    expect(onViewportAnchorChange).toHaveBeenLastCalledWith({
      line: 11,
      offsetPx: 20,
    });
  });

  it("scrolls to the bottom when a pending outbound message is appended (line_number = -1)", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    const scroll = await mountWithHeight(
      r,
      [msg(10, "a"), msg(11, "b"), msg(12, "c")],
      600,
    );
    // After first load: scrollTop = 600. Simulate the user pinned at bottom.
    stubScrollHeight(scroll, 800);

    await rerender(r, {
      ...baseProps,
      messages: [msg(10, "a"), msg(11, "b"), msg(12, "c"), pendingMsg()],
    });

    // Pending append MUST auto-scroll to the bottom, otherwise the user's
    // outbound message vanishes off-screen until the round-trip lands.
    expect(scroll.scrollTop).toBe(800);
  });

  it("does NOT pull the user down when they've scrolled up and a non-outbound message arrives", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    const scroll = await mountWithHeight(
      r,
      [msg(10, "a"), msg(11, "b"), msg(12, "c")],
      600,
    );
    // Simulate the user has scrolled up to read history. clientHeight is set
    // so the wasAtBottom calc reads as "not at bottom": 600 - 100 - 400 = 100 > 80.
    stubScrollTop(scroll, 100);
    stubClientHeight(scroll, 400);
    stubScrollHeight(scroll, 800);

    await rerender(r, {
      ...baseProps,
      messages: [msg(10, "a"), msg(11, "b"), msg(12, "c"), msg(13, "d")],
    });

    // scrollTop must stay where the user left it — the jump-to-latest button
    // will surface and they can opt in to seeing the new message.
    expect(scroll.scrollTop).toBe(100);
  });

  it("pulls the user down even when scrolled up if the new message is outbound (pending)", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    const scroll = await mountWithHeight(
      r,
      [msg(10, "a"), msg(11, "b"), msg(12, "c")],
      600,
    );
    // User scrolled up, same "not at bottom" geometry as the case above.
    stubScrollTop(scroll, 100);
    stubClientHeight(scroll, 400);
    stubScrollHeight(scroll, 800);

    await rerender(r, {
      ...baseProps,
      messages: [msg(10, "a"), msg(11, "b"), msg(12, "c"), pendingMsg()],
    });

    // Pressing Send is an explicit "I want to see what I just sent" signal,
    // so the outbound branch wins over the scrolled-up state.
    expect(scroll.scrollTop).toBe(800);
  });

  it("does NOT pull a restored historical viewport down when an inbound message arrives", async () => {
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
    const originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      value: vi.fn(),
      configurable: true,
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    try {
      await rerender(r, {
        ...baseProps,
        messages: [msg(43, "a"), msg(44, "b")],
        restoreAnchor: { line: 43, offsetPx: 0 },
      });
      const scroll = container.querySelector<HTMLDivElement>(
        "[data-message-scroll]",
      )!;
      stubScrollTop(scroll, 0);
      stubClientHeight(scroll, 640);
      stubScrollHeight(scroll, 560);

      await rerender(r, {
        ...baseProps,
        messages: [msg(43, "a"), msg(44, "b"), msg(45, "new inbound")],
        restoreAnchor: { line: 43, offsetPx: 0 },
      });

      expect(scroll.scrollTop).toBe(0);
    } finally {
      if (originalScrollIntoView) {
        Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
          value: originalScrollIntoView,
          configurable: true,
        });
      } else {
        delete (HTMLElement.prototype as { scrollIntoView?: unknown })
          .scrollIntoView;
      }
      vi.unstubAllGlobals();
    }
  });

  it("preserves the visual anchor when older messages are prepended", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    // Mount with 5 messages, height 500. After mount the layout effect has
    // captured prevScrollHeight=500, prevFirstLine=50, prevLength=5.
    const scroll = await mountWithHeight(
      r,
      [msg(50, "a"), msg(51, "b"), msg(52, "c"), msg(53, "d"), msg(54, "e")],
      500,
    );
    // Simulate the user has scrolled to the near-top trigger zone.
    Object.defineProperty(scroll, "scrollTop", {
      value: 20,
      writable: true,
      configurable: true,
    });
    // Older history arrives, height grows to 1000 (delta = 500).
    stubScrollHeight(scroll, 1000);

    await rerender(r, {
      ...baseProps,
      messages: [
        msg(45, "old-a"),
        msg(46, "old-b"),
        msg(47, "old-c"),
        msg(48, "old-d"),
        msg(49, "old-e"),
        msg(50, "a"),
        msg(51, "b"),
        msg(52, "c"),
        msg(53, "d"),
        msg(54, "e"),
      ],
    });

    // Expected: scrollTop = 20 + (1000 - 500) = 520 (visual anchor held).
    expect(scroll.scrollTop).toBe(520);
  });

  it("treats a scope change as a fresh timeline instead of preserving the previous channel anchor", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const r = createRoot(container);
    root = r;

    const scroll = await mountWithHeight(
      r,
      [msg(50, "a"), msg(51, "b"), msg(52, "c"), msg(53, "d"), msg(54, "e")],
      500,
    );
    stubScrollTop(scroll, 20);
    stubClientHeight(scroll, 400);
    stubScrollHeight(scroll, 700);

    await rerender(r, {
      ...baseProps,
      scopeKey: "random",
      messages: [msg(1, "new-a"), msg(2, "new-b"), msg(3, "new-c")],
    });

    expect(scroll.scrollTop).toBe(700);
  });

});
