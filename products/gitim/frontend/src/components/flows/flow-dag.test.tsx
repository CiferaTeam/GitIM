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

// React tracks the controlled input value through its own setter — writing
// directly to `.value` is invisible to React. Use the native prototype
// setter so the input event fires through React's onChange.
function setTextareaValue(el: HTMLTextAreaElement, next: string) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  setter?.call(el, next);
  el.dispatchEvent(new Event("input", { bubbles: true }));
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

  it("does not render Edit button when onSavePrompt is omitted", async () => {
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip-edit']"),
    ).toBeNull();
  });

  it("clicking Edit enters edit mode and locks the tooltip", async () => {
    const onSavePrompt = vi.fn();
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} onSavePrompt={onSavePrompt} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    const editBtn = document.body.querySelector<HTMLButtonElement>(
      "[data-testid='flow-dag-tooltip-edit']",
    );
    expect(editBtn).not.toBeNull();

    await act(async () => {
      editBtn!.click();
      await flushPromises();
    });

    // Textarea is mounted with the existing prompt as initial value.
    const textarea = document.body.querySelector<HTMLTextAreaElement>(
      "[data-testid='flow-dag-tooltip-textarea']",
    );
    expect(textarea).not.toBeNull();
    expect(textarea!.value).toBe(
      "Validate scope before starting implementation.",
    );

    // Hovering away no longer closes the tooltip while editing.
    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      vi.advanceTimersByTime(CLOSE_DELAY_MS + 50);
      await flushPromises();
    });
    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip']"),
    ).not.toBeNull();
  });

  it("Save calls onSavePrompt with edited prompt and closes tooltip", async () => {
    const onSavePrompt = vi.fn().mockResolvedValue({ ok: true });
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} onSavePrompt={onSavePrompt} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    await act(async () => {
      document.body
        .querySelector<HTMLButtonElement>(
          "[data-testid='flow-dag-tooltip-edit']",
        )!
        .click();
      await flushPromises();
    });

    const textarea = document.body.querySelector<HTMLTextAreaElement>(
      "[data-testid='flow-dag-tooltip-textarea']",
    )!;
    await act(async () => {
      setTextareaValue(textarea, "Revised scope prompt.");
      await flushPromises();
    });

    await act(async () => {
      document.body
        .querySelector<HTMLButtonElement>(
          "[data-testid='flow-dag-tooltip-save']",
        )!
        .click();
      await flushPromises();
    });

    expect(onSavePrompt).toHaveBeenCalledWith(
      "scope-gate",
      "Revised scope prompt.",
    );
    // Successful save keeps the tooltip open in read mode showing the new
    // prompt — the user gets immediate visual confirmation.
    const tooltip = document.body.querySelector(
      "[data-testid='flow-dag-tooltip']",
    );
    expect(tooltip).not.toBeNull();
    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip-textarea']"),
    ).toBeNull();
    expect(tooltip!.textContent).toContain("Revised scope prompt.");
    // Edit button comes back since we're back in read mode.
    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip-edit']"),
    ).not.toBeNull();
  });

  it("Save surfaces error and keeps edit mode when onSavePrompt fails", async () => {
    const onSavePrompt = vi
      .fn()
      .mockResolvedValue({ ok: false, error: "patch failed" });
    await act(async () => {
      root!.render(<FlowDAG nodes={nodes} onSavePrompt={onSavePrompt} />);
      await flushPromises();
    });

    const target = Array.from(document.querySelectorAll(".node")).find(
      (n) => n.querySelector(".nodeLabel")?.textContent === "scope-gate",
    );
    await act(async () => {
      target!.dispatchEvent(new MouseEvent("mouseenter", { bubbles: true }));
      await flushPromises();
    });

    await act(async () => {
      document.body
        .querySelector<HTMLButtonElement>(
          "[data-testid='flow-dag-tooltip-edit']",
        )!
        .click();
      await flushPromises();
    });

    const textarea = document.body.querySelector<HTMLTextAreaElement>(
      "[data-testid='flow-dag-tooltip-textarea']",
    )!;
    await act(async () => {
      setTextareaValue(textarea, "x");
      await flushPromises();
    });

    await act(async () => {
      document.body
        .querySelector<HTMLButtonElement>(
          "[data-testid='flow-dag-tooltip-save']",
        )!
        .click();
      await flushPromises();
    });

    const tooltip = document.body.querySelector(
      "[data-testid='flow-dag-tooltip']",
    );
    expect(tooltip).not.toBeNull();
    expect(tooltip!.textContent).toContain("patch failed");
    // Still in edit mode — textarea remains.
    expect(
      document.body.querySelector("[data-testid='flow-dag-tooltip-textarea']"),
    ).not.toBeNull();
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
