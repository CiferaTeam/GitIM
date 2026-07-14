// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useAgentStore } from "@/hooks/use-agent-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useQuickSessionStore } from "@/hooks/use-quick-session-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { QuickSessionHub } from "./quick-session-hub";

vi.hoisted(() => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      get length() {
        return values.size;
      },
      clear: () => values.clear(),
      getItem: (key: string) => values.get(key) ?? null,
      key: (index: number) => Array.from(values.keys())[index] ?? null,
      removeItem: (key: string) => values.delete(key),
      setItem: (key: string, value: string) => values.set(key, value),
    },
  });
});

const api = vi.hoisted(() => ({
  listQuickSessions: vi.fn(),
  createQuickSession: vi.fn(),
  readQuickSession: vi.fn(),
  sendQuickSessionMessage: vi.fn(),
  archiveQuickSession: vi.fn(),
  unarchiveQuickSession: vi.fn(),
}));

vi.mock("@/lib/client", () => api);
vi.mock("@/components/ui/popover", () => ({
  Popover: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  PopoverTrigger: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  PopoverContent: ({
    children,
    onPointerEnter,
    onPointerLeave,
  }: {
    children: React.ReactNode;
    onPointerEnter?: React.PointerEventHandler<HTMLDivElement>;
    onPointerLeave?: React.PointerEventHandler<HTMLDivElement>;
  }) => (
    <div onPointerEnter={onPointerEnter} onPointerLeave={onPointerLeave}>
      {children}
    </div>
  ),
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const SESSION_ID = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const session = {
  meta: {
    id: SESSION_ID,
    title: "Investigate flakes",
    title_source: "api_set" as const,
    agent_id: "alice",
    created_by: "lewis",
    status: "active" as const,
    created_at: "2026-07-11T00:00:00Z",
    updated_at: "2026-07-11T00:01:00Z",
    last_message_preview: "fixed",
    revision: 4,
  },
  entries: [
    {
      line_number: 1,
      point_to: 0,
      author: "lewis",
      timestamp: "20260711T000000Z",
      body: "Investigate flakes",
    },
  ],
  archived: false,
};

function setValue(element: HTMLTextAreaElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(
    element,
    value,
  );
  element.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("QuickSessionHub", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.clearAllMocks();
    api.listQuickSessions.mockResolvedValue({ ok: true, data: { sessions: [] } });
    api.createQuickSession.mockResolvedValue({
      ok: true,
      data: { session, line_number: 1, ref: `session:${SESSION_ID}` },
    });
    useQuickSessionStore.getState().resetForWorkspaceSwitch();
    useConnectionStore.setState({ mode: "remote" });
    useWorkspaceStore.setState({
      activeSlug: "room",
      workspaces: [
        {
          slug: "room",
          workspace_name: "Room",
          path: "/tmp/room",
          provider: "local",
          initialized: true,
        },
      ],
    });
    useAgentStore.setState({
      agents: [
        {
          id: "alice",
          handler: "alice",
          name: "Alice",
          status: "idle",
          systemPrompt: "",
          repoPath: "/tmp/alice",
          messagesProcessed: 0,
        },
      ],
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("pins by click and creates a session through the selected agent", async () => {
    await act(async () => {
      root.render(<QuickSessionHub />);
      await Promise.resolve();
    });
    const trigger = container.querySelector(
      "button[aria-label='Quick Sessions']",
    ) as HTMLButtonElement;
    await act(async () => {
      trigger.click();
      await Promise.resolve();
    });
    expect(trigger.getAttribute("aria-pressed")).toBe("true");

    const newButton = container.querySelector(
      "button[aria-label='New Quick Session']",
    ) as HTMLButtonElement;
    await act(async () => newButton.click());
    const textarea = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("focus on"),
    )!;
    await act(async () => setValue(textarea, "Investigate flakes"));
    const start = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent?.includes("Start session"),
    )!;
    await act(async () => {
      start.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(api.createQuickSession).toHaveBeenCalledWith(
      "room",
      "alice",
      "Investigate flakes",
      expect.stringMatching(/^qs-/),
    );
    expect(container.textContent).toContain("Investigate flakes");
    expect(useQuickSessionStore.getState().selectedId).toBe(SESSION_ID);
  });
});
