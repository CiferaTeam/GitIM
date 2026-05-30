// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MentionPopup } from "./mention-popup";
import { DisplayNameDirectoryProvider } from "../../hooks/display-name-directory-provider";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

async function renderPopup(filter: string): Promise<{
  container: HTMLDivElement;
  root: Root;
}> {
  useAgentStore.setState({ agents: [] });
  useChatStore.setState({
    userInfos: [
      { handler: "alice", display_name: "Alice Chen" },
      { handler: "bob", display_name: "Bob Smith" },
    ],
  });

  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <DisplayNameDirectoryProvider>
        <MentionPopup
          users={["alice", "bob"]}
          filter={filter}
          onSelect={() => {}}
          onClose={() => {}}
        />
      </DisplayNameDirectoryProvider>,
    );
    await Promise.resolve();
  });
  return { container, root };
}

afterEach(() => {
  useAgentStore.setState({ agents: [] });
  useChatStore.setState({ userInfos: [] });
});

describe("MentionPopup two-segment filter", () => {
  it("matches on the display name, not just the handle", async () => {
    // "chen" appears only in the display name "Alice Chen", not the handle.
    const { container, root } = await renderPopup("chen");
    const buttons = container.querySelectorAll("button");
    expect(buttons.length).toBe(1);
    expect(buttons[0].textContent).toBe("Alice Chen@alice");
    act(() => root.unmount());
  });

  it("still matches on the handle", async () => {
    const { container, root } = await renderPopup("bob");
    const buttons = container.querySelectorAll("button");
    expect(buttons.length).toBe(1);
    expect(buttons[0].textContent).toContain("@bob");
    act(() => root.unmount());
  });

  it("renders both segments for each candidate", async () => {
    const { container, root } = await renderPopup("");
    const buttons = container.querySelectorAll("button");
    expect(buttons.length).toBe(2);
    expect(buttons[0].textContent).toBe("Alice Chen@alice");
    act(() => root.unmount());
  });
});
