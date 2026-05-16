// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { AgentList } from "./agent-list";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useFleetStore } from "@/hooks/use-fleet-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Agent } from "@/lib/types";

vi.mock("./add-agent-dialog", () => ({
  AddAgentDialog: () => <button type="button">Add Agent</button>,
}));

vi.mock("@/lib/client", async () => {
  const actual = await vi.importActual<typeof import("@/lib/client")>(
    "@/lib/client",
  );
  return {
    ...actual,
    listArchivedUsers: vi.fn(),
  };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function agent(id: string, provider = "codex"): Agent {
  return {
    id,
    name: id,
    status: "running",
    provider: provider as Agent["provider"],
    systemPrompt: "",
    repoPath: `/tmp/${id}`,
    messagesProcessed: 0,
  };
}

describe("AgentList fleet grouping", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    useWorkspaceStore.setState({ activeSlug: "room" });
    useAgentStore.setState({
      agents: [agent("local-cfo")],
      selectedAgentId: null,
    });
    useFleetStore.setState({
      agents: [
        {
          nodeId: "node-a",
          nodeName: "mac-mini",
          nodeIp: "100.64.0.10",
          workspaceId: "room",
          remoteWorkspaceId: "remote-room",
          workspaceIdentity: "github.com/org/repo",
          agent: agent("remote-cfo", "claude"),
        },
      ],
      statuses: [
        {
          nodeId: "node-a",
          nodeName: "mac-mini",
          nodeIp: "100.64.0.10",
          workspaceId: "room",
          remoteWorkspaceId: "remote-room",
          workspaceIdentity: "github.com/org/repo",
          status: "connected",
          retryCount: 0,
        },
      ],
    });
  });

  afterEach(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    container.remove();
    useAgentStore.getState().resetForWorkspaceSwitch();
    useFleetStore.getState().resetForWorkspaceSwitch();
  });

  it("renders local agents before remote node groups", () => {
    act(() => {
      root?.render(
        <MemoryRouter>
          <AgentList />
        </MemoryRouter>,
      );
    });

    const text = container.textContent ?? "";
    expect(text).toContain("Local");
    expect(text).toContain("local-cfo");
    expect(text).toContain("mac-mini");
    expect(text).toContain("Connected");
    expect(text).toContain("remote-cfo");
    expect(text.indexOf("Local")).toBeLessThan(text.indexOf("mac-mini"));
  });
});
