// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { UserCardContent } from "./user-card";
import { DisplayNameDirectoryProvider } from "../../hooks/display-name-directory-provider";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useFleetStore } from "../../hooks/use-fleet-store";
import type { Agent, FleetAgentSnapshot } from "../../lib/types";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function agent(overrides: Partial<Agent> & Pick<Agent, "handler">): Agent {
  return {
    id: overrides.handler,
    name: overrides.handler,
    status: "idle",
    systemPrompt: "",
    repoPath: "",
    messagesProcessed: 0,
    ...overrides,
  };
}

async function renderContent(
  handler: string,
  onStartDm?: (h: string) => void,
): Promise<{ container: HTMLDivElement; root: Root }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <DisplayNameDirectoryProvider>
        <UserCardContent handler={handler} onStartDm={onStartDm} />
      </DisplayNameDirectoryProvider>,
    );
    await Promise.resolve();
  });
  return { container, root };
}

afterEach(() => {
  useAgentStore.setState({ agents: [] });
  useChatStore.setState({ userInfos: [], currentUser: "" });
  useFleetStore.setState({ agents: [] });
});

describe("UserCardContent", () => {
  it("shows provider and model for a local agent", async () => {
    useAgentStore.setState({
      agents: [
        agent({
          handler: "dev-yun-long",
          name: "云龙",
          provider: "claude",
          model: "claude-opus-4-8",
        }),
      ],
    });
    useChatStore.setState({
      currentUser: "lewis",
      userInfos: [{ handler: "dev-yun-long", display_name: "云龙", kind: "agent" }],
    });

    const { container, root } = await renderContent("dev-yun-long", vi.fn());
    expect(container.querySelector("[data-testid=user-card-model]")?.textContent).toBe(
      "Claude · claude-opus-4-8",
    );
    expect(container.textContent).toContain("Agent");
    act(() => root.unmount());
  });

  it("shows hermes llm provider/model", async () => {
    useAgentStore.setState({
      agents: [
        agent({
          handler: "mishu-xiaoqian",
          provider: "hermes",
          llmProvider: "deepseek",
          llmModel: "deepseek-chat",
        }),
      ],
    });
    useChatStore.setState({ currentUser: "lewis" });

    const { container, root } = await renderContent("mishu-xiaoqian", vi.fn());
    expect(container.querySelector("[data-testid=user-card-model]")?.textContent).toBe(
      "Hermes · deepseek / deepseek-chat",
    );
    act(() => root.unmount());
  });

  it("resolves model info from a fleet agent when not local", async () => {
    const snapshot: FleetAgentSnapshot = {
      nodeId: "node-1",
      workspaceId: "ws",
      agent: agent({
        handler: "remote-bot",
        provider: "codex",
        model: "gpt-5.4",
      }),
    };
    useFleetStore.setState({ agents: [snapshot] });
    useChatStore.setState({ currentUser: "lewis" });

    const { container, root } = await renderContent("remote-bot", vi.fn());
    expect(container.querySelector("[data-testid=user-card-model]")?.textContent).toBe(
      "Codex · gpt-5.4",
    );
    act(() => root.unmount());
  });

  it("hides model line for a human and still offers DM", async () => {
    useChatStore.setState({
      currentUser: "lewis",
      userInfos: [{ handler: "alice", display_name: "Alice Chen", kind: "human" }],
    });

    const onStartDm = vi.fn();
    const { container, root } = await renderContent("alice", onStartDm);
    expect(container.querySelector("[data-testid=user-card-model]")).toBeNull();
    expect(container.textContent).toContain("Human");
    const button = container.querySelector("[data-testid=user-card-dm]");
    expect(button).not.toBeNull();
    await act(async () => {
      button?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onStartDm).toHaveBeenCalledWith("alice");
    act(() => root.unmount());
  });

  it("does not show Send message when hovering yourself", async () => {
    useChatStore.setState({ currentUser: "lewis" });
    const { container, root } = await renderContent("lewis", vi.fn());
    expect(container.querySelector("[data-testid=user-card-dm]")).toBeNull();
    act(() => root.unmount());
  });
});
