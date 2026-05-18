// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ScrollToBottomButton } from "./scroll-to-bottom-button";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

async function mount(node: React.ReactNode): Promise<{ container: HTMLDivElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(node);
    await Promise.resolve();
  });
  return { container, root };
}

function findButton(container: HTMLElement): HTMLButtonElement {
  const btn = container.querySelector("button");
  if (!btn) throw new Error("button not rendered");
  return btn as HTMLButtonElement;
}

describe("ScrollToBottomButton", () => {
  let root: Root | null = null;

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("is hidden via opacity + pointer-events when visible=false", async () => {
    const { container, root: r } = await mount(
      <ScrollToBottomButton visible={false} onClick={() => {}} />,
    );
    root = r;
    const btn = findButton(container);

    expect(btn.className).toMatch(/opacity-0/);
    expect(btn.className).toMatch(/pointer-events-none/);
    expect(btn.getAttribute("aria-hidden")).toBe("true");
    expect(btn.getAttribute("tabindex")).toBe("-1");
  });

  it("is visible and focusable when visible=true", async () => {
    const { container, root: r } = await mount(
      <ScrollToBottomButton visible={true} onClick={() => {}} />,
    );
    root = r;
    const btn = findButton(container);

    expect(btn.className).toMatch(/opacity-100/);
    expect(btn.className).toMatch(/pointer-events-auto/);
    expect(btn.getAttribute("aria-hidden")).toBe("false");
    expect(btn.getAttribute("tabindex")).toBe("0");
  });

  it("fires onClick when clicked", async () => {
    const onClick = vi.fn();
    const { container, root: r } = await mount(
      <ScrollToBottomButton visible={true} onClick={onClick} />,
    );
    root = r;
    const btn = findButton(container);

    await act(async () => {
      btn.click();
      await Promise.resolve();
    });

    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
