// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FlowDAG } from "./flow-dag";
import type { FlowNodeSummary } from "@/lib/types";

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

  it("hides tooltip on mouseleave", async () => {
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
    expect(tooltip).toBeNull();
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
