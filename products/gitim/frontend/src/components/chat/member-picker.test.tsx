// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemberPicker } from "./member-picker";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function findCheckbox(container: HTMLElement, handle: string): HTMLInputElement {
  const label = Array.from(container.querySelectorAll("label")).find((el) =>
    el.textContent?.includes(`@${handle}`),
  );
  if (!label) throw new Error(`missing checkbox for @${handle}`);
  const checkbox = label.querySelector<HTMLInputElement>('input[type="checkbox"]');
  if (!checkbox) throw new Error(`missing input for @${handle}`);
  return checkbox;
}

function candidateLabels(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll("label")).map((el) =>
    el.textContent?.trim() ?? "",
  );
}

function setInputValue(input: HTMLInputElement, value: string) {
  const valueSetter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )?.set;
  valueSetter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

function renderPicker(options: {
  value?: string[];
  excludeHandlers?: string[];
  onChange?: (selected: string[]) => void;
} = {}) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const onChange = options.onChange ?? vi.fn();

  function Harness() {
    const [value, setValue] = useState(options.value ?? []);
    return (
      <MemberPicker
        allUsers={["alice", "bob", "carol", "dave"]}
        excludeHandlers={options.excludeHandlers ?? []}
        value={value}
        onChange={(next) => {
          onChange(next);
          setValue(next);
        }}
        emptyMessage="Nobody found"
      />
    );
  }

  act(() => {
    root.render(<Harness />);
  });

  return { container, onChange, root };
}

describe("MemberPicker", () => {
  let roots: Root[] = [];

  afterEach(() => {
    for (const root of roots) {
      act(() => {
        root.unmount();
      });
    }
    roots = [];
    document.body.innerHTML = "";
  });

  it("filters out excluded handlers before rendering candidates", () => {
    const rendered = renderPicker({ excludeHandlers: ["bob", "carol"] });
    roots.push(rendered.root);

    expect(candidateLabels(rendered.container)).toEqual(["@alice", "@dave"]);
  });

  it("filters candidates case-insensitively", async () => {
    const rendered = renderPicker();
    roots.push(rendered.root);
    const search = rendered.container.querySelector<HTMLInputElement>("input[data-slot='input']");
    expect(search).not.toBeNull();

    await act(async () => {
      setInputValue(search!, "AL");
      await Promise.resolve();
    });

    expect(candidateLabels(rendered.container)).toEqual(["@alice"]);
  });

  it("toggles and removes selected handlers", () => {
    const onChange = vi.fn();
    const rendered = renderPicker({ value: ["bob"], onChange });
    roots.push(rendered.root);

    expect(findCheckbox(rendered.container, "bob").checked).toBe(true);

    act(() => {
      findCheckbox(rendered.container, "alice").click();
    });
    expect(onChange).toHaveBeenLastCalledWith(["bob", "alice"]);
    expect(rendered.container.querySelector("[aria-label='Remove alice']")).not.toBeNull();

    act(() => {
      rendered.container
        .querySelector<HTMLButtonElement>("[aria-label='Remove bob']")!
        .click();
    });
    expect(onChange).toHaveBeenLastCalledWith(["alice"]);
    expect(findCheckbox(rendered.container, "bob").checked).toBe(false);
  });
});
