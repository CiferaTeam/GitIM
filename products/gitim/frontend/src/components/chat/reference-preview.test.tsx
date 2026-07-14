// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useCardStore } from "@/hooks/use-card-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Message } from "@/lib/types";
import {
  CardReferenceLink,
  QuickSessionReferenceLink,
} from "./reference-preview";
import {
  getCardPreviewReadQuery,
  selectCardPreviewMessages,
} from "./reference-preview-utils";

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
    useWorkspaceStore.setState({ activeSlug: "room" });
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
            title_source: "api_set",
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
});
