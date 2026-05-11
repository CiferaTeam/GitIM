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
});
