// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useCardStore } from "@/hooks/use-card-store";
import { CardReferenceLink } from "./reference-preview";

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
});
