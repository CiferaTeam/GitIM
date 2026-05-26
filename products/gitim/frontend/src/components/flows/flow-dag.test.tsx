// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FlowDAG } from "./flow-dag";
import type { FlowNodeSummary } from "@/lib/types";

// Matches TOOLTIP_CLOSE_DELAY_MS in flow-dag.tsx. Kept loose so tests are
// robust to small bumps in the constant.
const CLOSE_DELAY_MS = 150;

const mocks = vi.hoisted(() => ({
  mermaidRender: vi.fn(),
}));

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: mocks.mermaidRender,
  },
}));

vi.mock("react-markdown", () => ({
  default: ({ children }: { children: string }) => (
    <span data-testid="markdown">{children}</span>
  ),
}));

function buildMockSvg(nodeIds: string[]): string {
  const nodes = nodeIds
    .map(
      (id, idx) =>
        `<g class="node" id="flowchart-TD-${id}-${idx}"><rect class="nodeRect"></rect><text class="nodeLabel">${id}</text></g>`,
    )
    .join("");
  return `<svg>${nodes}</svg>`;
}

async function flushPromises(times = 4) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

describe("FlowDAG node hover tooltip", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  beforeEach(() => {
    vi.clearAllMocks();
    // Only fake setTimeout/clearTimeout so microtasks (mermaid render promises)
    // resolve normally while tests can drive the close-delay clock.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    mocks.mermaidRender.mockImplementation(() =>
      Promise.resolve({ svg: buildMockSvg(["scope-gate", "requirements"]) }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
    }
    root = null;
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  const nodes: FlowNodeSummary[] = [
    {
      id: "scope-gate",
      type: "human_review",
      prompt: "Validate scope before starting implementation.",
    },
    {
      id: "requirements",
      type: "human_review",
      needs: ["scope-gate"],
      prompt: "",
    },
  ];

  it("shows tooltip on mouseenter with the node's prompt", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );

    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    const tooltip = document.body.querySelector(
      "[data-testid='flow-dag-tooltip']",
    );
    expect(tooltip).not.toBeNull();
    expect(tooltip!.textContent).toContain("scope-gate");
    expect(tooltip!.textContent).toContain(
      "Validate scope before starting implementation.",
    );
  });

  it("hides tooltip after mouseleave grace period", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      await flushPromises();
    });

    // Still visible immediately after leave — grace period lets the cursor
    // travel to the tooltip itself.
    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).not.toBeNull();

    await act(async () => {
      vi.advanceTimersByTime(CLOSE_DELAY_MS + 50);
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).toBeNull();
  });

  it("keeps tooltip visible when cursor enters tooltip during grace period", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      await flushPromises();
    });

    const tooltip = document.body.querySelector(
      "[data-testid='flow-dag-tooltip']",
    );
    expect(tooltip).not.toBeNull();

    // Cursor reaches the tooltip before the close timer fires — this cancels
    // the close so the user can scroll the prompt.
    await act(async () => {
      tooltip!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      vi.advanceTimersByTime(CLOSE_DELAY_MS + 50);
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).not.toBeNull();
  });

  it("closes tooltip after grace period when cursor leaves the tooltip", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    const tooltip = document.body.querySelector(
      "[data-testid='flow-dag-tooltip']",
    );
    expect(tooltip).not.toBeNull();

    // Cursor moves onto tooltip then leaves it entirely.
    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      tooltip!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      tooltip!.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      vi.advanceTimersByTime(CLOSE_DELAY_MS + 50);
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).toBeNull();
  });

  it("shows tooltip on focus and hides on blur", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    ) as SVGGElement | undefined;
    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.focus();
      target!.dispatchEvent(new FocusEvent("focus", { bubbles: true }));
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).not.toBeNull();

    await act(async () => {
      target!.blur();
      target!.dispatchEvent(new FocusEvent("blur", { bubbles: true }));
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).toBeNull();
  });

  it("renders placeholder for empty prompt and does not crash", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "requirements",
    );
    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    const tooltip = document.body.querySelector(
      "[data-testid='flow-dag-tooltip']",
    );
    expect(tooltip).not.toBeNull();
    expect(tooltip!.textContent).toContain("requirements");
    expect(tooltip!.textContent).toContain("(no prompt body)");
  });

  it("hides tooltip on Escape keydown", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    ) as SVGGElement | undefined;
    expect(target).not.toBeUndefined();

    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).not.toBeNull();

    await act(async () => {
      target!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).toBeNull();
  });
});
