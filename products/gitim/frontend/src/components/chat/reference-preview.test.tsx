// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useCardStore } from "@/hooks/use-card-store";
import type { Message } from "@/lib/types";
import { CardReferenceLink } from "./reference-preview";
import {
  getCardPreviewReadQuery,
  selectCardPreviewMessages,
} from "./reference-preview-utils";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

describe("CardReferenceLink", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    useCardStore.getState().resetForWorkspaceSwitch();
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
});
