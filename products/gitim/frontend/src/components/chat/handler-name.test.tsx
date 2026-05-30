// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { HandlerName } from "./handler-name";
import { DisplayNameDirectoryProvider } from "../../hooks/display-name-directory-provider";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import type { Agent, UserInfo } from "../../lib/types";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function agent(handler: string, name: string): Agent {
  return {
    id: handler,
    handler,
    name,
    status: "running",
    systemPrompt: "",
    repoPath: "",
    messagesProcessed: 0,
  };
}

async function renderName(
  handler: string,
  opts: { users?: UserInfo[]; agents?: Agent[]; showHandle?: boolean } = {},
): Promise<{ container: HTMLDivElement; root: Root }> {
  useAgentStore.setState({ agents: opts.agents ?? [] });
  useChatStore.setState({ userInfos: opts.users ?? [] });

  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <DisplayNameDirectoryProvider>
        <HandlerName handler={handler} showHandle={opts.showHandle} />
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

describe("HandlerName", () => {
  it("renders display_name and a muted @handler for a known human", async () => {
    const { container, root } = await renderName("alice", {
      users: [{ handler: "alice", display_name: "Alice Chen" }],
    });
    expect(container.textContent).toBe("Alice Chen@alice");
    // The @handler lives in its own muted/mono span, not the display name.
    const handleSpan = container.querySelector("span span");
    expect(handleSpan?.textContent).toBe("@alice");
    expect(handleSpan?.className).toContain("font-mono");
    act(() => root.unmount());
  });

  it("resolves agents via their handler", async () => {
    const { container, root } = await renderName("cfo", {
      agents: [agent("cfo", "Finance Bot")],
    });
    expect(container.textContent).toBe("Finance Bot@cfo");
    act(() => root.unmount());
  });

  it("falls back to bare @handler for an unknown handler", async () => {
    const { container, root } = await renderName("ghost");
    expect(container.textContent).toBe("@ghost");
    expect(container.querySelector("span span")).toBeNull();
    act(() => root.unmount());
  });

  it("falls back to bare @handler when display_name equals handler", async () => {
    const { container, root } = await renderName("bob", {
      users: [{ handler: "bob", display_name: "bob" }],
    });
    expect(container.textContent).toBe("@bob");
    act(() => root.unmount());
  });

  it("drops the @handler segment when showHandle is false", async () => {
    const { container, root } = await renderName("alice", {
      users: [{ handler: "alice", display_name: "Alice Chen" }],
      showHandle: false,
    });
    expect(container.textContent).toBe("Alice Chen");
    act(() => root.unmount());
  });
});
