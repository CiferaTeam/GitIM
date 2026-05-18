// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import type { Message } from "./lib/types";

const mocks = vi.hoisted(() => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
  },
  client: {
    listWorkspaces: vi.fn(),
    me: vi.fn(),
    channels: vi.fn(),
    users: vi.fn(),
    listAgents: vi.fn(),
    listFleetAgents: vi.fn(),
    listFleetStatus: vi.fn(),
    listCards: vi.fn(),
    listBoards: vi.fn(),
    listArchivedCards: vi.fn(),
    read: vi.fn(),
    readCard: vi.fn(),
    showBoard: vi.fn(),
    poll: vi.fn(),
    activateBrowserWorkspace: vi.fn(),
  },
}));

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

vi.mock("sonner", () => ({
  Toaster: () => null,
  toast: mocks.toast,
}));

vi.mock("./lib/client", () => mocks.client);

vi.mock("./hooks/use-agent-activity", () => ({
  useAgentActivitySSE: () => undefined,
}));

vi.mock("./hooks/use-fleet-store", async () => {
  const actual = await vi.importActual<typeof import("./hooks/use-fleet-store")>(
    "./hooks/use-fleet-store",
  );
  return {
    ...actual,
    useFleetSSE: () => undefined,
  };
});

vi.mock("./hooks/use-media-query", () => ({
  useIsMobile: () => false,
  useMediaQuery: () => false,
}));

vi.mock("./components/layout/app-shell", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  const { Outlet } = await vi.importActual<typeof import("react-router")>(
    "react-router",
  );
  return {
    AppShell: () => React.createElement(Outlet),
  };
});

vi.mock("./components/cards/card-detail", () => ({
  CardDetail: () => null,
}));

vi.mock("./components/cards/card-kanban", () => ({
  CardKanban: () => null,
}));

vi.mock("./components/boards/boards-view", () => ({
  BoardsView: () => null,
}));

vi.mock("./components/chat/chat-layout", () => ({
  ChatLayout: () => null,
}));

vi.mock("./components/crons/cron-calendar", () => ({
  CronCalendar: () => null,
}));

vi.mock("./components/flows/flows-view", () => ({
  FlowsView: () => null,
}));

vi.mock("./components/flows/run-detail", () => ({
  RunDetail: () => null,
}));

vi.mock("./components/management/agent-detail", () => ({
  AgentDetail: () => null,
}));

vi.mock("./components/management/agent-list", () => ({
  AgentList: () => null,
}));

vi.mock("./components/docs/docs-page", () => ({
  DocsPage: () => null,
}));

import App from "./app";
import { useAgentStore } from "./hooks/use-agent-store";
import { useBoardStore } from "./hooks/use-board-store";
import { useCardStore } from "./hooks/use-card-store";
import { useChatStore } from "./hooks/use-chat-store";
import { useConnectionStore } from "./hooks/use-connection-store";
import { useWorkspaceStore } from "./hooks/use-workspace-store";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function LocationProbe({ onPath }: { onPath: (path: string) => void }) {
  const location = useLocation();
  onPath(location.pathname);
  return null;
}

function NavigationProbe({
  onNavigate,
}: {
  onNavigate: (navigate: (to: string) => void) => void;
}) {
  const navigate = useNavigate();
  onNavigate((to) => {
    navigate(to);
  });
  return null;
}

async function flushPromises(times = 4) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

describe("App card thread toasts", () => {
  let root: Root | null = null;

  beforeEach(() => {
    vi.useFakeTimers();
    testEnv.localStorage.clear();
    vi.clearAllMocks();

    mocks.client.listWorkspaces.mockResolvedValue({
      ok: true,
      data: {
        workspaces: [
          {
            slug: "room",
            workspace_name: "room",
            path: "/tmp/room",
            provider: "local",
            initialized: true,
          },
        ],
      },
    });
    mocks.client.me.mockResolvedValue({ ok: true, data: { handler: "lewis" } });
    mocks.client.channels.mockResolvedValue({
      ok: true,
      data: {
        channels: [
          {
            name: "general",
            kind: "channel",
            unreadCount: 0,
            hasMention: false,
            members: ["lewis", "alice"],
          },
        ],
      },
    });
    mocks.client.users.mockResolvedValue({ ok: true, data: { users: ["lewis", "alice"] } });
    mocks.client.listAgents.mockResolvedValue({ ok: true, data: { agents: [] } });
    mocks.client.listFleetAgents.mockResolvedValue({ ok: true, data: { agents: [] } });
    mocks.client.listFleetStatus.mockResolvedValue({ ok: true, data: { nodes: [] } });
    mocks.client.listCards.mockResolvedValue({
      ok: true,
      data: {
        cards: [
          {
            card_id: "card-123456789",
            channel: "general",
            title: "Follow up",
            status: "todo",
            labels: [],
            assignee: null,
            created_by: "lewis",
            created_at: "20260516T000000Z",
            updated_at: "20260516T000000Z",
          },
        ],
      },
    });
    mocks.client.listBoards.mockResolvedValue({ ok: true, data: { boards: [] } });
    mocks.client.listArchivedCards.mockResolvedValue({ ok: true, data: { cards: [] } });
    mocks.client.read.mockResolvedValue({ ok: true, data: { entries: [] } });
    mocks.client.readCard.mockResolvedValue({
      ok: true,
      data: {
        archived: false,
        meta: {
          card_id: "card-123456789",
          channel: "general",
          title: "Follow up",
          status: "todo",
          labels: [],
          assignee: null,
          created_by: "lewis",
          created_at: "20260516T000000Z",
          updated_at: "20260516T000000Z",
        },
        entries: [],
      },
    });
    mocks.client.showBoard.mockResolvedValue({ ok: true, data: null });
    mocks.client.poll.mockResolvedValue({
      ok: true,
      data: {
        commit_id: "next-head",
        changes: [
          {
            kind: "card_thread",
            channel: "card:general/card-123456789",
            entries: [
              {
                line_number: 1,
                point_to: 0,
                author: "alice",
                timestamp: "20260516T000001Z",
                body: "done",
              } satisfies Message,
            ],
          },
        ],
      },
    });

    useConnectionStore.setState({
      mode: "remote",
      status: "ready",
      port: 5317,
      runtimeVersion: null,
      headCommit: null,
      error: null,
      isUpdating: false,
      isRestarting: false,
      updateError: null,
      localReady: false,
      cloneProgress: null,
    });
    useWorkspaceStore.setState({
      workspaces: [
        {
          slug: "room",
          workspace_name: "room",
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
    useChatStore.getState().resetForWorkspaceSwitch();
    useAgentStore.getState().resetForWorkspaceSwitch();
    useCardStore.getState().resetForWorkspaceSwitch();
    useBoardStore.getState().resetForWorkspaceSwitch();
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
      root = null;
    }
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  it("navigates to the card when the card thread toast action is clicked", async () => {
    let currentPath = "/docs";
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(
        <MemoryRouter initialEntries={["/docs"]}>
          <LocationProbe onPath={(path) => { currentPath = path; }} />
          <App />
        </MemoryRouter>,
      );
      await flushPromises();
    });

    await act(async () => {
      vi.advanceTimersByTime(3000);
      await flushPromises();
    });

    expect(mocks.toast.info).toHaveBeenCalledWith(
      "Card #card-123: new message from @alice",
      expect.objectContaining({
        action: expect.objectContaining({
          label: "Open card",
          onClick: expect.any(Function),
        }),
      }),
    );

    const [, options] = mocks.toast.info.mock.calls[0];
    await act(async () => {
      options.action.onClick();
      await flushPromises();
    });

    expect(currentPath).toBe("/cards/general/card-123456789");
  });

  it("does not refresh local or fleet agents on every poll tick", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(
        <MemoryRouter initialEntries={["/chat"]}>
          <App />
        </MemoryRouter>,
      );
      await flushPromises();
    });

    expect(mocks.client.listAgents).toHaveBeenCalledTimes(1);
    expect(mocks.client.listFleetAgents).toHaveBeenCalledTimes(1);
    expect(mocks.client.listFleetStatus).toHaveBeenCalledTimes(1);

    mocks.client.listAgents.mockClear();
    mocks.client.listFleetAgents.mockClear();
    mocks.client.listFleetStatus.mockClear();

    await act(async () => {
      vi.advanceTimersByTime(3000);
      await flushPromises();
    });

    expect(mocks.client.poll).toHaveBeenCalled();
    expect(mocks.client.listAgents).not.toHaveBeenCalled();
    expect(mocks.client.listFleetAgents).not.toHaveBeenCalled();
    expect(mocks.client.listFleetStatus).not.toHaveBeenCalled();
  });

  it("refreshes local and fleet agents when entering management", async () => {
    let navigateTo: (to: string) => void = () => {};
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(
        <MemoryRouter initialEntries={["/chat"]}>
          <NavigationProbe onNavigate={(navigate) => { navigateTo = navigate; }} />
          <App />
        </MemoryRouter>,
      );
      await flushPromises();
    });

    mocks.client.listAgents.mockClear();
    mocks.client.listFleetAgents.mockClear();
    mocks.client.listFleetStatus.mockClear();

    await act(async () => {
      navigateTo("/management");
      await flushPromises();
    });

    expect(mocks.client.listAgents).toHaveBeenCalledTimes(1);
    expect(mocks.client.listFleetAgents).toHaveBeenCalledTimes(1);
    expect(mocks.client.listFleetStatus).toHaveBeenCalledTimes(1);
  });
});
