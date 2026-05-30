// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { FlowStructureEditor } from "./flow-structure-editor";
import type { FlowDocument, FlowNodeSummary } from "@/lib/types";

// FlowDAG pulls in mermaid; stub it so these tests stay focused on the form.
vi.mock("./flow-dag", () => ({
  FlowDAG: () => null,
}));
vi.mock("react-markdown", () => ({
  default: ({ children }: { children: string }) => <span>{children}</span>,
}));

async function flushPromises(times = 4) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

// React tracks controlled values through its own setter; write through the
// native prototype setter so onChange fires.
function setNativeValue(
  el: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
  next: string,
  event: "input" | "change",
) {
  const proto = Object.getPrototypeOf(el);
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, next);
  el.dispatchEvent(new Event(event, { bubbles: true }));
}

function makeDoc(): FlowDocument {
  return {
    slug: "release",
    name: "Release",
    description: "",
    created_by: "lewis",
    created_at: "2026-05-30T00:00:00Z",
    nodes: [
      {
        id: "changelog",
        type: "agent_mention",
        owner: "alice",
        prompt: "gen changelog",
      },
      {
        id: "e2e",
        type: "agent_mention",
        owner: "bob",
        needs: ["changelog"],
        prompt: "run tests",
      },
    ],
    raw_markdown: "",
  };
}

function rows(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>("[data-testid='fse-node-row']"),
  );
}

function q<T extends Element>(sel: string): T | null {
  return document.querySelector<T>(sel);
}

describe("FlowStructureEditor", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  beforeEach(() => {
    vi.clearAllMocks();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    if (root) act(() => root?.unmount());
    root = null;
    document.body.innerHTML = "";
  });

  async function render(props: {
    onSave?: (
      nodes: FlowNodeSummary[],
    ) => Promise<{ ok: boolean; error?: string }>;
    onCancel?: () => void;
  }) {
    await act(async () => {
      root!.render(
        <FlowStructureEditor
          doc={makeDoc()}
          onSave={props.onSave ?? vi.fn().mockResolvedValue({ ok: true })}
          onCancel={props.onCancel ?? vi.fn()}
        />,
      );
      await flushPromises();
    });
  }

  it("seeds one row per existing node", async () => {
    await render({});
    expect(rows().length).toBe(2);
  });

  it("Add node appends a row", async () => {
    await render({});
    await act(async () => {
      q<HTMLButtonElement>("[data-testid='fse-add']")!.click();
      await flushPromises();
    });
    expect(rows().length).toBe(3);
  });

  it("existing node id input is read-only", async () => {
    await render({});
    const idInput = rows()[0].querySelector<HTMLInputElement>(
      "[data-testid='fse-node-id']",
    )!;
    expect(idInput.disabled).toBe(true);
  });

  it("a newly added node has an editable id input", async () => {
    await render({});
    await act(async () => {
      q<HTMLButtonElement>("[data-testid='fse-add']")!.click();
      await flushPromises();
    });
    const newRow = rows()[2];
    const idInput = newRow.querySelector<HTMLInputElement>(
      "[data-testid='fse-node-id']",
    )!;
    expect(idInput.disabled).toBe(false);
  });

  it("changing type to channel_thread swaps owner for participants", async () => {
    await render({});
    const row = rows()[0];
    expect(
      row.querySelector("[data-testid='fse-node-owner']"),
    ).not.toBeNull();
    const typeSelect = row.querySelector<HTMLSelectElement>(
      "[data-testid='fse-node-type']",
    )!;
    await act(async () => {
      setNativeValue(typeSelect, "channel_thread", "change");
      await flushPromises();
    });
    const updated = rows()[0];
    expect(updated.querySelector("[data-testid='fse-node-owner']")).toBeNull();
    expect(
      updated.querySelector("[data-testid='fse-node-participants']"),
    ).not.toBeNull();
  });

  it("Save calls onSave with the current draft nodes", async () => {
    const onSave = vi.fn().mockResolvedValue({ ok: true });
    await render({ onSave });
    await act(async () => {
      q<HTMLButtonElement>("[data-testid='fse-save']")!.click();
      await flushPromises();
    });
    expect(onSave).toHaveBeenCalledTimes(1);
    const arg = onSave.mock.calls[0][0] as FlowNodeSummary[];
    expect(arg.length).toBe(2);
    expect(arg[0].id).toBe("changelog");
    expect(arg[1].needs).toEqual(["changelog"]);
  });

  it("Save error keeps the rows and surfaces the message", async () => {
    const onSave = vi
      .fn()
      .mockResolvedValue({ ok: false, error: "cycle detected in flow DAG" });
    await render({ onSave });
    await act(async () => {
      q<HTMLButtonElement>("[data-testid='fse-save']")!.click();
      await flushPromises();
    });
    const err = q("[data-testid='fse-error']");
    expect(err).not.toBeNull();
    expect(err!.textContent).toContain("cycle");
    // Draft is not lost — both rows remain editable.
    expect(rows().length).toBe(2);
  });

  it("Cancel invokes onCancel", async () => {
    const onCancel = vi.fn();
    await render({ onCancel });
    await act(async () => {
      q<HTMLButtonElement>("[data-testid='fse-cancel']")!.click();
      await flushPromises();
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
