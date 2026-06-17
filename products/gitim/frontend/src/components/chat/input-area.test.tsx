// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { InputArea } from "./input-area";

vi.mock("../../hooks/use-media-query", () => ({
  useIsMobile: () => false,
}));

const memoryStorage = (() => {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k: string) => m.get(k) ?? null,
    key: (i: number) => Array.from(m.keys())[i] ?? null,
    removeItem: (k: string) => m.delete(k),
    setItem: (k: string, v: string) => {
      m.set(k, v);
    },
  } as Storage;
})();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: memoryStorage,
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function setTextareaValue(textarea: HTMLTextAreaElement, value: string) {
  const valueSetter = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  valueSetter?.call(textarea, value);
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}

const noopSend = vi.fn(async () => ({ ok: true as const }));

describe("InputArea card recipient preview", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
    container.remove();
  });

  it("previews the reporter and assignee for a card draft", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(
        <InputArea
          workspaceKey="ws"
          scopeKey="card:strategy/20260616-174312-42b"
          replyTo={null}
          onReplyToChange={() => {}}
          mentionCandidates={[]}
          recipientCard={{ created_by: "leader1", assignee: "leader2" }}
          currentUser="lewis"
          onSend={noopSend}
        />,
      );
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      setTextareaValue(textarea!, "ss");
      await Promise.resolve();
    });

    const preview = document.querySelector("[data-recipient-preview]");
    expect(preview).not.toBeNull();
    expect(preview?.textContent).toContain("@leader1");
    expect(preview?.textContent).toContain("@leader2");
    expect(preview?.textContent).not.toContain("no one else");
  });

  it("excludes the current user when they are a card role", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(
        <InputArea
          workspaceKey="ws"
          scopeKey="card:strategy/c1"
          replyTo={null}
          onReplyToChange={() => {}}
          mentionCandidates={[]}
          recipientCard={{ created_by: "leader1", assignee: "lewis" }}
          currentUser="lewis"
          onSend={noopSend}
        />,
      );
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    await act(async () => {
      setTextareaValue(textarea!, "ss");
      await Promise.resolve();
    });

    const preview = document.querySelector("[data-recipient-preview]");
    expect(preview?.textContent).toContain("@leader1");
    expect(preview?.textContent).not.toContain("@lewis");
  });
});
