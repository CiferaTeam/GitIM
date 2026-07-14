// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useAgentStore } from "@/hooks/use-agent-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useFleetStore } from "@/hooks/use-fleet-store";
import { useQuickSessionStore } from "@/hooks/use-quick-session-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { QuickSessionHub } from "./quick-session-hub";
import { QuickSessionList } from "./quick-session-list";

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

function changeValue(element: HTMLSelectElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set?.call(
    element,
    value,
  );
  element.dispatchEvent(new Event("change", { bubbles: true }));
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
    api.readQuickSession.mockResolvedValue({
      ok: true,
      data: { session },
    });
    api.sendQuickSessionMessage.mockResolvedValue({
      ok: true,
      data: {
        session_id: SESSION_ID,
        line_number: 2,
        status: "active",
        revision: 5,
        ref: `session:${SESSION_ID}:L000002`,
      },
    });
    api.archiveQuickSession.mockResolvedValue({
      ok: true,
      data: {
        session_id: SESSION_ID,
        status: "archived",
        revision: 5,
        archived_at: "2026-07-11T00:02:00Z",
      },
    });
    api.unarchiveQuickSession.mockResolvedValue({ ok: true });
    useQuickSessionStore.getState().resetForWorkspaceSwitch();
    useFleetStore.getState().resetForWorkspaceSwitch();
    useChatStore.setState({ users: [], userInfos: [] });
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

  it("opens on keyboard focus and closes after the unpinned hover delay", async () => {
    vi.useFakeTimers();
    try {
      await act(async () => root.render(<QuickSessionHub />));
      const trigger = container.querySelector(
        "button[aria-label='Quick Sessions']",
      ) as HTMLButtonElement;
      await act(async () => trigger.focus());
      expect(trigger.className).toContain("bg-primary/10");

      await act(async () => {
        trigger.parentElement?.dispatchEvent(
          new Event("pointerout", { bubbles: true }),
        );
        vi.advanceTimersByTime(179);
      });
      expect(trigger.className).toContain("bg-primary/10");
      await act(async () => vi.advanceTimersByTime(1));
      expect(trigger.className).not.toContain("bg-primary/10");
    } finally {
      vi.useRealTimers();
    }
  });

  it("opens detail, sends, archives, and refreshes the archived filter", async () => {
    api.listQuickSessions.mockResolvedValue({
      ok: true,
      data: {
        sessions: [
          {
            id: session.meta.id,
            title: session.meta.title,
            agent_id: session.meta.agent_id,
            created_by: session.meta.created_by,
            status: session.meta.status,
            updated_at: session.meta.updated_at,
            last_message_preview: session.meta.last_message_preview,
            revision: session.meta.revision,
            archived: false,
            ref: `session:${session.meta.id}`,
          },
        ],
      },
    });
    await act(async () => root.render(<QuickSessionHub />));
    const trigger = container.querySelector(
      "button[aria-label='Quick Sessions']",
    ) as HTMLButtonElement;
    await act(async () => {
      trigger.click();
      await vi.waitFor(() => expect(container.textContent).toContain("Investigate flakes"));
    });
    const rowButton = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent?.includes("Investigate flakes"),
    )!;
    await act(async () => {
      rowButton.click();
      await vi.waitFor(() => expect(api.readQuickSession).toHaveBeenCalled());
    });

    const composer = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("Continue"),
    )!;
    await act(async () => setValue(composer, "follow up"));
    await act(async () => {
      (container.querySelector(
        "button[aria-label='Send Quick Session message']",
      ) as HTMLButtonElement).click();
      await vi.waitFor(() => expect(api.sendQuickSessionMessage).toHaveBeenCalled());
    });
    expect(api.sendQuickSessionMessage).toHaveBeenCalledWith(
      "room",
      SESSION_ID,
      "follow up",
      expect.stringMatching(/^[0-9A-HJKMNP-TV-Z]{26}$/),
    );

    await act(async () => {
      (container.querySelector(
        "button[aria-label='Archive session']",
      ) as HTMLButtonElement).click();
      await vi.waitFor(() => expect(api.archiveQuickSession).toHaveBeenCalled());
    });
    expect(useQuickSessionStore.getState().selectedId).toBeNull();

    const archived = container.querySelector("input[type='checkbox']") as HTMLInputElement;
    await act(async () => archived.click());
    await vi.waitFor(() => {
      expect(api.listQuickSessions).toHaveBeenLastCalledWith("room", {
        archived: true,
      });
    });
  });

  it("offers deduplicated local and matching-workspace fleet agents", async () => {
    useFleetStore.setState({
      agents: [
        {
          nodeId: "node-a",
          nodeName: "Mac mini",
          workspaceId: "room",
          agent: {
            id: "bob",
            handler: "bob",
            name: "Bob",
            status: "idle",
            systemPrompt: "",
            repoPath: "",
            messagesProcessed: 0,
          },
        },
        {
          nodeId: "node-b",
          workspaceId: "other",
          agent: {
            id: "carol",
            handler: "carol",
            name: "Carol",
            status: "idle",
            systemPrompt: "",
            repoPath: "",
            messagesProcessed: 0,
          },
        },
      ],
    });

    await act(async () => {
      root.render(<QuickSessionHub />);
      await Promise.resolve();
    });
    await act(async () => {
      (container.querySelector("button[aria-label='New Quick Session']") as HTMLButtonElement).click();
    });

    const options = Array.from(
      container.querySelectorAll("select[aria-label='Quick Session agent'] option"),
    ).map((option) => option.textContent);
    expect(options).toEqual(["Alice (@alice)", "Bob (@bob) · Mac mini"]);
  });

  it("uses active browser workspace handlers and labels them unverified", async () => {
    useConnectionStore.setState({ mode: "local" });
    useAgentStore.setState({ agents: [], selectedAgentId: null });
    useChatStore.setState({ users: ["lewis", "remote-agent"] });

    await act(async () => {
      root.render(<QuickSessionHub />);
      await Promise.resolve();
    });
    await act(async () => {
      (container.querySelector("button[aria-label='New Quick Session']") as HTMLButtonElement).click();
    });

    const select = container.querySelector(
      "select[aria-label='Quick Session agent']",
    ) as HTMLSelectElement;
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual([
      "@lewis (unverified)",
      "@remote-agent (unverified)",
    ]);
  });

  it("resets create state and selected handler when the workspace changes", async () => {
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
        {
          slug: "other",
          workspace_name: "Other",
          path: "/tmp/other",
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
          repoPath: "",
          messagesProcessed: 0,
        },
        {
          id: "bob",
          handler: "bob",
          name: "Bob",
          status: "idle",
          systemPrompt: "",
          repoPath: "",
          messagesProcessed: 0,
        },
      ],
    });

    await act(async () => root.render(<QuickSessionHub />));
    await act(async () => {
      (container.querySelector("button[aria-label='New Quick Session']") as HTMLButtonElement).click();
    });
    const select = container.querySelector(
      "select[aria-label='Quick Session agent']",
    ) as HTMLSelectElement;
    const firstMessage = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("focus on"),
    )!;
    await act(async () => {
      changeValue(select, "bob");
      setValue(firstMessage, "workspace-specific draft");
    });

    useAgentStore.setState({
      agents: [
        {
          id: "carol",
          handler: "carol",
          name: "Carol",
          status: "idle",
          systemPrompt: "",
          repoPath: "",
          messagesProcessed: 0,
        },
      ],
    });
    await act(async () => useWorkspaceStore.setState({ activeSlug: "other" }));

    const trigger = container.querySelector(
      "button[aria-label='Quick Sessions']",
    ) as HTMLButtonElement;
    expect(trigger.getAttribute("aria-pressed")).toBe("false");
    await act(async () => trigger.click());
    await act(async () => {
      (container.querySelector("button[aria-label='New Quick Session']") as HTMLButtonElement).click();
    });
    const nextSelect = container.querySelector(
      "select[aria-label='Quick Session agent']",
    ) as HTMLSelectElement;
    const nextMessage = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("focus on"),
    )!;
    expect(nextSelect.value).toBe("carol");
    expect(nextMessage.value).toBe("");
  });

  it("discards the conversation draft when selection changes", async () => {
    const otherSession = {
      ...session,
      meta: {
        ...session.meta,
        id: "qs-01JYYYYYYYYYYYYYYYYYYYYYYY",
        title: "Other session",
      },
    };
    useQuickSessionStore.setState({
      selectedId: SESSION_ID,
      detailById: {
        [SESSION_ID]: session,
        [otherSession.meta.id]: otherSession,
      },
    });
    await act(async () => root.render(<QuickSessionHub />));
    let composer = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("Continue"),
    )!;
    await act(async () => setValue(composer, "belongs to the first session"));

    await act(async () => useQuickSessionStore.getState().select(otherSession.meta.id));
    composer = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("Continue"),
    )!;
    expect(composer.value).toBe("");
  });

  it("keeps a newer selection and its draft when an older archive finishes", async () => {
    const otherSession = {
      ...session,
      meta: {
        ...session.meta,
        id: "qs-01JYYYYYYYYYYYYYYYYYYYYYYY",
        title: "Other session",
      },
    };
    let resolveArchive!: (value: {
      ok: boolean;
      data: {
        session_id: string;
        status: string;
        revision: number;
        archived_at: string;
      };
    }) => void;
    api.archiveQuickSession.mockReturnValue(
      new Promise((resolve) => {
        resolveArchive = resolve;
      }),
    );
    useQuickSessionStore.setState({
      selectedId: SESSION_ID,
      detailById: {
        [SESSION_ID]: session,
        [otherSession.meta.id]: otherSession,
      },
    });
    await act(async () => root.render(<QuickSessionHub />));
    await act(async () => {
      (container.querySelector(
        "button[aria-label='Archive session']",
      ) as HTMLButtonElement).click();
    });

    await act(async () => useQuickSessionStore.getState().select(otherSession.meta.id));
    const composer = Array.from(container.querySelectorAll("textarea")).find(
      (element) => element.placeholder.includes("Continue"),
    )!;
    await act(async () => setValue(composer, "keep this newer draft"));
    await act(async () => {
      resolveArchive({
        ok: true,
        data: {
          session_id: SESSION_ID,
          status: "archived",
          revision: 5,
          archived_at: "2026-07-11T00:03:00Z",
        },
      });
      await vi.waitFor(() => expect(api.listQuickSessions).toHaveBeenCalled());
    });

    expect(useQuickSessionStore.getState().selectedId).toBe(otherSession.meta.id);
    expect(composer.value).toBe("keep this newer draft");
  });

  it("renders list loading, error, empty, and complete row metadata states", async () => {
    const props = {
      selectedId: null,
      workspaceKey: "runtime:room",
      onSelect: vi.fn(),
      onCopy: vi.fn(),
    };
    await act(async () => {
      root.render(
        <QuickSessionList
          {...props}
          items={[]}
          loading={true}
          error={null}
        />,
      );
    });
    expect(container.textContent).toContain("Loading sessions");

    await act(async () => {
      root.render(
        <QuickSessionList
          {...props}
          items={[]}
          loading={false}
          error="list failed"
        />,
      );
    });
    expect(container.textContent).toContain("list failed");

    await act(async () => {
      root.render(
        <QuickSessionList
          {...props}
          items={[]}
          loading={false}
          error={null}
        />,
      );
    });
    expect(container.textContent).toContain("No Quick Sessions");

    await act(async () => {
      root.render(
        <QuickSessionList
          {...props}
          items={[
            {
              id: session.meta.id,
              title: session.meta.title,
              agent_id: session.meta.agent_id,
              created_by: session.meta.created_by,
              status: session.meta.status,
              updated_at: session.meta.updated_at,
              last_message_preview: session.meta.last_message_preview,
              revision: session.meta.revision,
              archived: false,
              ref: `session:${session.meta.id}`,
            },
          ]}
          loading={false}
          error={null}
        />,
      );
    });
    expect(container.textContent).toContain("active");
    expect(container.textContent).toContain("session:qs-");
    expect(container.textContent).toContain("08:01");

    const setData = vi.fn();
    const row = container.querySelector("[role='listitem']") as HTMLDivElement;
    const drag = new Event("dragstart", { bubbles: true, cancelable: true });
    Object.defineProperty(drag, "dataTransfer", {
      value: { effectAllowed: "none", setData },
    });
    await act(async () => row.dispatchEvent(drag));
    expect(setData).toHaveBeenCalledWith(
      "application/x-gitim-quick-session-ref",
      JSON.stringify({
        ref: `session:${session.meta.id}`,
        workspaceKey: "runtime:room",
      }),
    );

    await act(async () => {
      (container.querySelector("button:not([aria-label])") as HTMLButtonElement).click();
      (container.querySelector("button[aria-label^='Copy reference']") as HTMLButtonElement).click();
    });
    expect(props.onSelect).toHaveBeenCalledWith(session.meta.id);
    expect(props.onCopy).toHaveBeenCalledWith(`session:${session.meta.id}`);
  });
});
