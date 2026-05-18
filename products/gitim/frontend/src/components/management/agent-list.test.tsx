// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { AgentList } from "./agent-list";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useFleetStore } from "@/hooks/use-fleet-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Agent, UsageSummary } from "@/lib/types";

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

function usageSummary(tokens = 100): UsageSummary {
  return {
    providerReportsUsage: true,
    firstSeen: "2026-05-15T00:00:00Z",
    lastUpdated: "2026-05-15T00:10:00Z",
    totals: {
      input: tokens,
      output: 0,
      cacheRead: 0,
      cacheCreation: 0,
      turns: 1,
    },
    today: {
      input: tokens,
      output: 0,
      cacheRead: 0,
      cacheCreation: 0,
      turns: 1,
    },
    byDay: [
      {
        date: "2026-05-15",
        bucket: {
          input: tokens,
          output: 0,
          cacheRead: 0,
          cacheCreation: 0,
          turns: 1,
        },
      },
    ],
  };
}

function agent(id: string, provider = "codex", usage?: UsageSummary): Agent {
  return {
    id,
    name: id,
    status: "running",
    provider: provider as Agent["provider"],
    systemPrompt: "",
    repoPath: `/tmp/${id}`,
    messagesProcessed: 0,
    usageSummary: usage,
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

  it("does not duplicate fleet and local usage when there are no remote agents", () => {
    useAgentStore.setState({
      agents: [agent("local-cfo", "codex", usageSummary())],
      selectedAgentId: null,
    });
    useFleetStore.setState({
      agents: [],
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

    act(() => {
      root?.render(
        <MemoryRouter>
          <AgentList />
        </MemoryRouter>,
      );
    });

    const text = container.textContent ?? "";
    expect(text).toContain("Workspace Usage");
    expect(text).not.toContain("Fleet Usage");
    expect(text).not.toContain("Local Usage");
  });

  it("collapses per-node usage details until the node toggle is opened", () => {
    useAgentStore.setState({
      agents: [agent("local-cfo", "codex", usageSummary(100))],
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
          agent: agent("remote-cfo", "claude", usageSummary(50)),
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

    act(() => {
      root?.render(
        <MemoryRouter>
          <AgentList />
        </MemoryRouter>,
      );
    });

    const localToggle = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Show Local usage details"]',
    );
    const remoteToggle = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Show mac-mini usage details"]',
    );
    expect(localToggle).not.toBeNull();
    expect(remoteToggle).not.toBeNull();
    expect(localToggle?.getAttribute("aria-expanded")).toBe("false");
    expect(remoteToggle?.getAttribute("aria-expanded")).toBe("false");
    expect(container.textContent ?? "").toContain("Fleet Usage");
    expect(container.textContent ?? "").not.toContain("Local Usage");
    expect(container.textContent ?? "").not.toContain("mac-mini Usage");

    act(() => {
      localToggle?.click();
    });

    expect(localToggle?.getAttribute("aria-expanded")).toBe("true");
    expect(container.textContent ?? "").toContain("Local Usage");
    expect(container.textContent ?? "").not.toContain("mac-mini Usage");
  });
});
