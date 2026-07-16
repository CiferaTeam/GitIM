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

const api = vi.hoisted(() => ({ readQuickSession: vi.fn() }));
vi.mock("@/lib/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/client")>()),
  readQuickSession: api.readQuickSession,
}));

describe("CardReferenceLink", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.clearAllMocks();
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
    act(() => {
      root.unmount();
    });
    container.remove();
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

  it("reads and renders only the target card discussion line", () => {
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

    expect(getCardPreviewReadQuery(2)).toEqual({ since: 1, limit: 1 });
    expect(selectCardPreviewMessages(messages, 2).map((msg) => msg.line_number)).toEqual([2]);
  });

  it("loads active or archived Quick Sessions and highlights the requested line", async () => {
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
    });
    await act(async () => {
      (container.querySelector("button") as HTMLButtonElement).click();
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
