// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { MessageBody } from "./message-body";
import { useCardStore } from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import type { Card } from "@/lib/types";

vi.mock("./reference-preview", () => ({
  CardReferenceLink: ({
    reference,
  }: {
    reference: { cardId: string; line?: number };
  }) => (
    <span data-testid="card-ref">
      [{reference.cardId}:L{reference.line ?? ""}]
    </span>
  ),
  MessageReferenceLink: () => <span data-testid="message-ref" />,
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const card: Card = {
  card_id: "20260703-122930-bc0",
  channel: "gitim-pr47-sop-0702",
  title: "Recovery review 3",
  status: "done",
  labels: [],
  assignee: null,
  created_by: "cfo",
  created_at: "20260703T122930Z",
  updated_at: "20260703T123201Z",
};

describe("MessageBody legacy card references", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    useCardStore.getState().resetForWorkspaceSwitch();
    useCardStore.getState().upsertCard(card);
    useChatStore.setState({ currentChannel: "gitim-pr47-sop-0702" });
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

  it("folds trailing L-number text into the inline card reference", async () => {
    await act(async () => {
      root.render(
        <MemoryRouter>
          <MessageBody body="Recovery review (card `20260703-122930-bc0` L2)." />
        </MemoryRouter>,
      );
      await Promise.resolve();
    });

    expect(container.textContent).toContain("[20260703-122930-bc0:L2]");
    expect(container.textContent).not.toContain(" L2).");
  });
});
