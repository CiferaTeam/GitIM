// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemberPicker } from "./member-picker";
import { DisplayNameDirectoryProvider } from "../../hooks/display-name-directory-provider";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import type { UserInfo } from "../../lib/types";

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
  userInfos?: UserInfo[];
} = {}) {
  useAgentStore.setState({ agents: [] });
  useChatStore.setState({ userInfos: options.userInfos ?? [] });

  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const onChange = options.onChange ?? vi.fn();

  function Harness() {
    const [value, setValue] = useState(options.value ?? []);
    return (
      <DisplayNameDirectoryProvider>
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
      </DisplayNameDirectoryProvider>
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
    useAgentStore.setState({ agents: [] });
    useChatStore.setState({ userInfos: [] });
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

  it("filters candidates on display_name, not just the handle", async () => {
    const rendered = renderPicker({
      userInfos: [{ handler: "alice", display_name: "Alice Chen" }],
    });
    roots.push(rendered.root);
    const search = rendered.container.querySelector<HTMLInputElement>(
      "input[data-slot='input']",
    );

    await act(async () => {
      // "chen" appears only in the display name, not in any handle.
      setInputValue(search!, "chen");
      await Promise.resolve();
    });

    expect(candidateLabels(rendered.container)).toEqual(["Alice Chen@alice"]);
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
