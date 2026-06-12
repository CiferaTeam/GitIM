// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DisplayNameDirectoryProvider } from "../../hooks/display-name-directory-provider";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import type { Agent, UserInfo } from "../../lib/types";
import { DmLabel } from "./dm-label";

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

async function renderLabel(
  name: string,
  currentUser: string,
  opts: { users?: UserInfo[]; agents?: Agent[] } = {},
): Promise<{ container: HTMLDivElement; root: Root }> {
  useAgentStore.setState({ agents: opts.agents ?? [] });
  useChatStore.setState({ userInfos: opts.users ?? [] });

  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <DisplayNameDirectoryProvider>
        <DmLabel name={name} currentUser={currentUser} />
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

describe("DmLabel", () => {
  it("renders the current user's DM peer as display name plus handler", async () => {
    const { container, root } = await renderLabel("alice--lewis", "lewis", {
      users: [{ handler: "alice", display_name: "Alice Chen" }],
    });

    expect(container.textContent).toBe("Alice Chen@alice");
    expect(container.querySelector("span span")?.textContent).toBe("@alice");
    act(() => root.unmount());
  });

  it("renders both endpoints for DMs outside the current user", async () => {
    const { container, root } = await renderLabel("cfo--glm51", "lewis", {
      agents: [agent("cfo", "Finance Bot"), agent("glm51", "GLM 5.1")],
    });

    expect(container.textContent).toBe("Finance Bot@cfo↔GLM 5.1@glm51");
    act(() => root.unmount());
  });

  it("keeps malformed DM names unchanged", async () => {
    const { container, root } = await renderLabel("general", "lewis");

    expect(container.textContent).toBe("general");
    act(() => root.unmount());
  });
});
