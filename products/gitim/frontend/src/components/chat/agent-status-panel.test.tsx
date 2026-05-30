// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const testEnv = vi.hoisted(() => {
  function createMemoryStorage(): Storage {
    const values = new Map<string, string>();
    return {
      get length() {
        return values.size;
      },
      clear() {
        values.clear();
      },
      getItem(key: string) {
        return values.get(key) ?? null;
      },
      key(index: number) {
        return Array.from(values.keys())[index] ?? null;
      },
      removeItem(key: string) {
        values.delete(key);
      },
      setItem(key: string, value: string) {
        values.set(key, value);
      },
    };
  }

  const localStorage = createMemoryStorage();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: localStorage,
  });
  return { localStorage };
});

import { useAgentActivityStore } from "@/hooks/use-agent-activity";
import { useAgentStore } from "@/hooks/use-agent-store";
import { fleetActivityKey, useFleetStore } from "@/hooks/use-fleet-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Agent } from "@/lib/types";
import { AgentStatusPanel } from "./agent-status-panel";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function agentWithUsage(usedPercent: number): Agent {
  return {
    id: "cfo",
    handler: "cfo",
    name: "cfo",
    status: "running",
    systemPrompt: "",
    repoPath: "/tmp/cfo",
    messagesProcessed: 0,
    sessionUsage: {
      sessionId: "019e0bd5",
      inputTokens: 50_000,
      outputTokens: 500,
      maxTokens: 100_000,
      usedPercent,
      source: "provider_reported",
      updatedAt: "2026-05-09T10:00:00Z",
    },
  };
}

describe("AgentStatusPanel", () => {
  let root: Root | null = null;

  beforeEach(() => {
    testEnv.localStorage.clear();
    useAgentStore.setState({ agents: [], selectedAgentId: null });
    useFleetStore.getState().resetForWorkspaceSwitch();
    useAgentActivityStore.setState({ activities: {}, lastSlug: null });
    useWorkspaceStore.setState({
      workspaces: [
        {
          slug: "room",
          workspace_name: "Room",
          path: "/tmp/room",
          provider: "local",
          initialized: true,
        },
      ],
      activeSlug: "room",
      loading: false,
      error: null,
      errorCode: null,
    });
  });

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("renders session usage as a left-to-right liquid fill", async () => {
    useAgentStore.setState({ agents: [agentWithUsage(47.5)], selectedAgentId: null });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<AgentStatusPanel />);
      await Promise.resolve();
    });

    const fill = container.querySelector<HTMLElement>(
      '[data-testid="agent-usage-liquid"]',
    );

    expect(fill).not.toBeNull();
    expect(fill?.style.getPropertyValue("--agent-usage-fill")).toBe("47.5%");
    expect(fill?.getAttribute("aria-hidden")).toBe("true");
  });

  it("marks active work with a spinner on the usage fill", async () => {
    useAgentStore.setState({ agents: [agentWithUsage(47.5)], selectedAgentId: null });
    useAgentActivityStore.getState().push({
      agent_id: "cfo",
      workspace_id: "room",
      event_type: "thinking",
      detail: "processing...",
      timestamp: "2026-05-18T11:19:35Z",
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<AgentStatusPanel />);
      await Promise.resolve();
    });

    expect(container.textContent).not.toContain("working");
    expect(
      container.querySelector(".agent-usage-working-spinner"),
    ).not.toBeNull();
  });

  it("renders a remote fleet agent with its fleet activity", async () => {
    useFleetStore.getState().setAgents([
      {
        nodeId: "mac-mini",
        nodeName: "lewismac-mini",
        workspaceId: "room",
        remoteWorkspaceId: "room",
        workspaceIdentity: "github.com/flame4/room",
        agent: {
          id: "glm51op",
          handler: "glm51op",
          name: "glm51op",
          status: "idle",
          systemPrompt: "",
          repoPath: "",
          messagesProcessed: 0,
        },
      },
    ]);
    useAgentActivityStore.getState().pushForKey(
      fleetActivityKey("mac-mini", "room", "glm51op"),
      {
        agent_id: "glm51op",
        workspace_id: "room",
        event_type: "done",
        detail: "done (18.5s)",
        timestamp: "2026-05-18T11:19:35Z",
      },
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<AgentStatusPanel />);
      await Promise.resolve();
    });

    expect(container.textContent).toContain("glm51op");
    expect(container.textContent).toContain("lewismac-mini");
    expect(container.textContent).toContain("done (18.5s)");
  });
});
