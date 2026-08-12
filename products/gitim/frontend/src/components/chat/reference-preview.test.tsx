// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useCardStore } from "@/hooks/use-card-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { QUICK_SESSION_DRAG_MIME } from "@/lib/quick-session-ref";
import type { Message } from "@/lib/types";
import {
  CardReferenceLink,
  QuickSessionReferenceLink,
} from "./reference-preview";
import {
  getCardReplyPreviewReadQuery,
  getCardPreviewReadQuery,
  selectCardPreviewMessages,
} from "./reference-preview-utils";

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

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const api = vi.hoisted(() => ({
  readQuickSession: vi.fn(),
  readCard: vi.fn(),
  autoOpenHoverCard: false,
}));

vi.mock("@/lib/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/client")>()),
  readQuickSession: api.readQuickSession,
  readCard: api.readCard,
}));

vi.mock("@/components/ui/hover-card", async () => {
  const React = await import("react");
  const HoverOpenContext = React.createContext(false);
  return {
    HoverCard: ({
      open,
      onOpenChange,
      children,
    }: {
      open?: boolean;
      onOpenChange?: (open: boolean) => void;
      children?: React.ReactNode;
    }) => {
      React.useEffect(() => {
        if (api.autoOpenHoverCard) onOpenChange?.(true);
      }, [onOpenChange]);
      return (
        <HoverOpenContext.Provider value={Boolean(open)}>
          <div data-open={String(open)}>{children}</div>
        </HoverOpenContext.Provider>
      );
    },
    HoverCardTrigger: ({ children }: { children?: React.ReactNode }) => (
      <>{children}</>
    ),
    HoverCardContent: ({ children }: { children?: React.ReactNode }) => {
      const isOpen = React.useContext(HoverOpenContext);
      if (!isOpen) return null;
      return <div>{children}</div>;
    },
  };
});

vi.mock("@/components/ui/popover", async () => {
  const React = await import("react");
  const PopoverOpenContext = React.createContext(false);
  return {
    Popover: ({
      open,
      children,
    }: {
      open?: boolean;
      children?: React.ReactNode;
    }) => (
      <PopoverOpenContext.Provider value={Boolean(open)}>
        <div data-popover-open={String(open)}>{children}</div>
      </PopoverOpenContext.Provider>
    ),
    PopoverTrigger: ({ children }: { children?: React.ReactNode }) => (
      <>{children}</>
    ),
    PopoverContent: ({
      children,
      onPointerEnter,
      onPointerLeave,
      "aria-label": ariaLabel,
    }: {
      children?: React.ReactNode;
      onPointerEnter?: React.PointerEventHandler<HTMLDivElement>;
      onPointerLeave?: React.PointerEventHandler<HTMLDivElement>;
      "aria-label"?: string;
    }) => {
      const isOpen = React.useContext(PopoverOpenContext);
      if (!isOpen) return null;
      return (
        <div
          role="dialog"
          aria-label={ariaLabel}
          onPointerEnter={onPointerEnter}
          onPointerLeave={onPointerLeave}
        >
          {children}
        </div>
      );
    },
  };
});

describe("CardReferenceLink", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.clearAllMocks();
    api.autoOpenHoverCard = false;
    useCardStore.getState().resetForWorkspaceSwitch();
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
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    api.autoOpenHoverCard = false;
    act(() => {
      root.unmount();
    });
    container.remove();
    vi.restoreAllMocks();
  });

  it("renders an uncached card reference without re-rendering forever", async () => {
    await act(async () => {
      root.render(
        <CardReferenceLink
          reference={{
            channel: "gitim-pr47-sop-0702",
            cardId: "20260702-031858-067",
          }}
          onOpen={vi.fn()}
        />,
      );
      await Promise.resolve();
    });

    expect(container.textContent).toContain("20260702...067");
  });

  it("does not stick on Loading preview when the load effect re-runs after status=loading", async () => {
    api.autoOpenHoverCard = true;
    let resolveRead: ((value: unknown) => void) | undefined;
    api.readCard.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveRead = resolve;
        }),
    );

    await act(async () => {
      root.render(
        <CardReferenceLink
          reference={{
            channel: "awesome-agents-team-0519",
            cardId: "20260801-135834-917",
            label: "Verify strict multi-agent scope",
          }}
          onOpen={vi.fn()}
        />,
      );
      await Promise.resolve();
    });

    expect(document.body.textContent).toContain("Loading preview...");
    expect(api.readCard).toHaveBeenCalled();

    await act(async () => {
      resolveRead?.({
        ok: true,
        data: {
          meta: {
            card_id: "20260801-135834-917",
            channel: "awesome-agents-team-0519",
            title: "Verify strict multi-agent scope",
            status: "done",
            labels: [],
            assignee: "pi-minimax3-code",
            created_by: "alice",
            created_at: "20260801T135834Z",
            updated_at: "20260801T140000Z",
          },
          entries: [
            {
              line_number: 1,
              point_to: 0,
              author: "alice",
              timestamp: "20260801T135900Z",
              body: "kickoff notes for the verify pass",
            },
            {
              line_number: 2,
              point_to: 0,
              author: "pi-minimax3-code",
              timestamp: "20260801T140000Z",
              body: "verification complete — scope is strict",
            },
          ],
          archived: false,
        },
      });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(document.body.textContent).not.toContain("Loading preview...");
    expect(document.body.textContent).toContain(
      "verification complete — scope is strict",
    );
  });

  it("shows sparse grouped replies in an isolated scrollable card change preview", async () => {
    const onOuterWheel = vi.fn();
    const onOpen = vi.fn();
    const longReply = [
      "first recent reply",
      "context line two",
      "context line three",
      "context line four",
      "context line five",
      "context line six",
    ].join("\n");
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(
      function clientHeight(this: HTMLElement) {
        return this.textContent === longReply ? 72 : 18;
      },
    );
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(
      function scrollHeight(this: HTMLElement) {
        return this.textContent === longReply ? 144 : 18;
      },
    );
    const entries: Message[] = [
      {
        line_number: 2,
        point_to: 0,
        author: "alice",
        timestamp: "20260811T150000Z",
        body: longReply,
      },
      {
        line_number: 40,
        point_to: 2,
        author: "bob",
        timestamp: "20260811T150100Z",
        body: "second recent reply",
      },
      {
        line_number: 90,
        point_to: 40,
        author: "cfo",
        timestamp: "20260811T150200Z",
        body: "latest recent reply",
      },
      {
        line_number: 140,
        point_to: 90,
        author: "alice",
        timestamp: "20260811T150300Z",
        body: "unrelated later reply",
      },
    ];
    api.readCard.mockImplementation(
      async (_slug: string, _channel: string, _cardId: string, query: {
        limit?: number;
        since?: number;
      }) => {
        const afterCursor = query.since == null
          ? entries
          : entries.filter((message) => message.line_number > query.since!);
        const selected = query.since == null
          ? afterCursor.slice(-(query.limit ?? afterCursor.length))
          : afterCursor.slice(0, query.limit);
        return {
          ok: true,
          data: {
            meta: {
              card_id: "20260811-145635-fa3",
              channel: "awesome-agents-team-0519",
              title: "Review PR #51: Crewargo",
              status: "done",
              labels: [],
              assignee: "opencode-dsflash-paid",
              created_by: "cfo",
              created_at: "20260811T145635Z",
              updated_at: "20260811T150200Z",
            },
            entries: selected,
            archived: false,
          },
        };
      },
    );

    await act(async () => {
      root.render(
        <div onWheel={onOuterWheel}>
          <CardReferenceLink
            reference={{
              channel: "awesome-agents-team-0519",
              cardId: "20260811-145635-fa3",
              line: 90,
            }}
            latestReplyCount={7}
            previewStartLine={2}
            onOpen={onOpen}
          />
        </div>,
      );
      await Promise.resolve();
    });

    const trigger = container.querySelector("button");
    expect(trigger).not.toBeNull();
    await act(async () => {
      trigger?.click();
      await Promise.resolve();
    });

    await act(async () => {
      await vi.waitFor(() => {
        expect(document.body.textContent).toContain("first recent reply");
      });
    });

    expect(onOpen).not.toHaveBeenCalled();

    expect(api.readCard).toHaveBeenCalledWith(
      "room",
      "awesome-agents-team-0519",
      "20260811-145635-fa3",
      { since: 1, limit: 12 },
    );
    expect(document.body.textContent).toContain("second recent reply");
    expect(document.body.textContent).toContain("latest recent reply");
    expect(document.body.textContent).not.toContain("unrelated later reply");
    expect(document.body.textContent).toContain("7 new replies");

    const firstReply = document.body.querySelector<HTMLButtonElement>(
      'button[title="Open full card at L000002"]',
    );
    expect(firstReply).not.toBeNull();
    const firstReplyBody = firstReply?.querySelector<HTMLElement>(
      ".whitespace-pre-wrap",
    );
    expect(firstReplyBody).toBeDefined();
    expect(firstReplyBody?.className).toContain("line-clamp-4");

    const expandButtons = Array.from(document.body.querySelectorAll("button")).filter(
      (button) => button.textContent?.trim() === "Show more",
    );
    expect(expandButtons).toHaveLength(1);
    expect(expandButtons[0]?.getAttribute("aria-expanded")).toBe("false");
    await act(async () => {
      expandButtons[0]?.click();
    });
    expect(onOpen).not.toHaveBeenCalled();
    expect(firstReplyBody?.className).not.toContain("line-clamp-");
    expect(expandButtons[0]?.textContent?.trim()).toBe("Show less");
    expect(expandButtons[0]?.getAttribute("aria-expanded")).toBe("true");
    await act(async () => {
      expandButtons[0]?.click();
    });
    expect(firstReplyBody?.className).toContain("line-clamp-4");
    expect(document.body.textContent).not.toContain("Scroll to browse");

    const selectionSpy = vi.spyOn(window, "getSelection").mockReturnValue({
      anchorNode: firstReplyBody?.firstChild ?? null,
      focusNode: firstReplyBody?.firstChild ?? null,
      isCollapsed: false,
      toString: () => longReply,
    } as Selection);
    await act(async () => {
      firstReply?.dispatchEvent(
        new MouseEvent("click", { bubbles: true, detail: 1 }),
      );
    });
    expect(onOpen).not.toHaveBeenCalled();
    selectionSpy.mockRestore();

    const secondReply = document.body.querySelector<HTMLButtonElement>(
      'button[title="Open full card at L000040"]',
    );
    expect(secondReply).not.toBeNull();
    await act(async () => {
      secondReply?.click();
    });
    expect(onOpen).toHaveBeenCalledWith(40);
    onOpen.mockClear();

    const scroller = document.body.querySelector<HTMLElement>(
      '[aria-label="Recent card discussion"]',
    );
    expect(scroller).not.toBeNull();
    expect(scroller?.getAttribute("role")).toBe("region");
    expect(scroller?.className).toContain("overflow-y-auto");
    await act(async () => {
      scroller?.dispatchEvent(new WheelEvent("wheel", { bubbles: true }));
    });
    expect(onOuterWheel).not.toHaveBeenCalled();

    const viewAll = Array.from(document.body.querySelectorAll("button")).find(
      (button) => button.textContent?.includes("View all"),
    );
    expect(viewAll).toBeDefined();
    await act(async () => {
      viewAll?.click();
    });
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenCalledWith();
  });

  it("fetches the full merged range and displays its latest replies", async () => {
    const entries: Message[] = Array.from({ length: 16 }, (_, index) => ({
      line_number: 2 + index * 10,
      point_to: index === 0 ? 0 : 2 + (index - 1) * 10,
      author: index % 2 === 0 ? "alice" : "bob",
      timestamp: `20260811T15${String(index).padStart(2, "0")}00Z`,
      body: index === 15 ? "unrelated later reply" : `merged reply ${index + 1}`,
    }));
    api.readCard.mockImplementation(
      async (_slug: string, _channel: string, _cardId: string, query: {
        limit?: number;
        since?: number;
      }) => {
        const afterCursor = entries.filter(
          (message) => query.since == null || message.line_number > query.since,
        );
        return {
          ok: true,
          data: {
            meta: {
              card_id: "20260811-145635-fa3",
              channel: "awesome-agents-team-0519",
              title: "Review PR #51: Crewargo",
              status: "done",
              labels: [],
              assignee: "opencode-dsflash-paid",
              created_by: "cfo",
              created_at: "20260811T145635Z",
              updated_at: "20260811T151400Z",
            },
            entries: afterCursor.slice(0, query.limit),
            archived: false,
          },
        };
      },
    );

    await act(async () => {
      root.render(
        <CardReferenceLink
          reference={{
            channel: "awesome-agents-team-0519",
            cardId: "20260811-145635-fa3",
            line: 142,
          }}
          latestReplyCount={15}
          previewStartLine={2}
          onOpen={vi.fn()}
        />,
      );
      await Promise.resolve();
    });

    const trigger = container.querySelector("button");
    await act(async () => {
      trigger?.click();
      await Promise.resolve();
    });
    await act(async () => {
      await vi.waitFor(() => {
        expect(document.body.textContent).toContain("merged reply 15");
      });
    });

    expect(api.readCard).toHaveBeenCalledWith(
      "room",
      "awesome-agents-team-0519",
      "20260811-145635-fa3",
      { since: 1, limit: 15 },
    );
    expect(document.body.textContent).not.toContain("L000002");
    expect(document.body.textContent).not.toContain("L000022");
    expect(document.body.textContent).toContain("merged reply 4");
    expect(document.body.textContent).toContain("merged reply 15");
    expect(document.body.textContent).not.toContain("unrelated later reply");
  });

  it("reads a window of card discussion messages around the target line", () => {
    const messages: Message[] = [
      {
        line_number: 1,
        point_to: 0,
        author: "alice",
        timestamp: "20260703T120000Z",
        body: "setup",
      },
      {
        line_number: 2,
        point_to: 0,
        author: "bob",
        timestamp: "20260703T120100Z",
        body: "target",
      },
      {
        line_number: 3,
        point_to: 0,
        author: "alice",
        timestamp: "20260703T120200Z",
        body: "follow-up",
      },
    ];

    expect(getCardPreviewReadQuery(2)).toEqual({ since: 0, limit: 11 });
    expect(selectCardPreviewMessages(messages, 2).map((msg) => msg.line_number)).toEqual([
      1, 2, 3,
    ]);
    expect(getCardPreviewReadQuery()).toEqual({ limit: 12 });
    expect(getCardReplyPreviewReadQuery(2, 15)).toEqual({ since: 1, limit: 15 });
    expect(getCardReplyPreviewReadQuery(2, 5000)).toEqual({ since: 1, limit: 1000 });
    expect(selectCardPreviewMessages(messages).map((msg) => msg.line_number)).toEqual([
      1, 2, 3,
    ]);
  });

  it("loads active or archived Quick Sessions and highlights the requested line", async () => {
    api.autoOpenHoverCard = true;
    api.readQuickSession.mockResolvedValue({
      ok: true,
      data: {
        session: {
          meta: {
            id: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
            title: "Investigate flakes",
            agent_id: "alice",
            created_by: "lewis",
            status: "archived",
            created_at: "2026-07-11T00:00:00Z",
            updated_at: "2026-07-11T00:01:00Z",
            archived_at: "2026-07-11T00:01:00Z",
            archived_from: "active",
            last_message_preview: "fixed",
            summary: "The flaky clock was isolated.",
            revision: 8,
          },
          entries: [
            {
              line_number: 7,
              point_to: 0,
              author: "alice",
              timestamp: "20260711T000100Z",
              body: "target line",
            },
          ],
          archived: true,
        },
      },
    });

    await act(async () => {
      root.render(
        <QuickSessionReferenceLink
          reference={{
            sessionId: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
            line: 7,
          }}
        />,
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    await act(async () => {
      await vi.waitFor(() => {
        expect(document.body.textContent).toContain("Investigate flakes");
      });
    });

    expect(api.readQuickSession).toHaveBeenCalledWith(
      "room",
      "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
      { since: 1, limit: 11 },
    );
    expect(document.body.textContent).toContain("target line");
    expect(document.body.textContent).toContain("archived");
    const highlighted = Array.from(document.body.querySelectorAll("div")).find(
      (element) => element.textContent?.includes("target line") && element.className.includes("bg-primary/10"),
    );
    expect(highlighted).toBeDefined();
  });

  it("drags a rendered Quick Session token with its workspace identity", async () => {
    await act(async () => {
      root.render(
        <QuickSessionReferenceLink
          reference={{
            sessionId: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
            line: 7,
          }}
        />,
      );
    });
    const setData = vi.fn();
    const trigger = container.querySelector("button") as HTMLButtonElement;
    const event = new Event("dragstart", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", {
      value: { effectAllowed: "none", setData },
    });
    await act(async () => trigger.dispatchEvent(event));

    expect(trigger.draggable).toBe(true);
    expect(setData).toHaveBeenCalledWith(
      QUICK_SESSION_DRAG_MIME,
      JSON.stringify({
        ref: "session:qs-01JZZZZZZZZZZZZZZZZZZZZZZZ:L000007",
        workspaceKey: "runtime:room",
      }),
    );
    expect(setData).toHaveBeenCalledWith(
      "text/plain",
      "session:qs-01JZZZZZZZZZZZZZZZZZZZZZZZ:L000007",
    );
  });
});
