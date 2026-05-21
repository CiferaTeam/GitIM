// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter, Route, Routes } from "react-router";
import { CardDetail } from "./card-detail";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useCardStore } from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Card, Message } from "@/lib/types";

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

const mocks = vi.hoisted(() => ({
  client: {
    readCard: vi.fn(),
    sendCardMessage: vi.fn(),
    updateCard: vi.fn(),
    archiveCard: vi.fn(),
    unarchiveCard: vi.fn(),
  },
  messageListProps: [] as Array<Record<string, unknown>>,
}));

vi.mock("@/lib/client", () => mocks.client);

vi.mock("@/components/chat/message-list", () => ({
  MessageList: (props: Record<string, unknown>) => {
    mocks.messageListProps.push(props);
    return <div data-testid="message-list" />;
  },
}));

vi.mock("@/components/chat/input-area", () => ({
  InputArea: () => null,
}));

vi.mock("./card-meta-bar", () => ({
  CardMetaBar: () => null,
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const card: Card = {
  card_id: "card-1",
  channel: "general",
  title: "Investigate state",
  status: "todo",
  labels: [],
  assignee: null,
  created_by: "lewis",
  created_at: "20260516T000000Z",
  updated_at: "20260516T000000Z",
};

const entries: Message[] = [
  {
    line_number: 1,
    point_to: 0,
    author: "alice",
    timestamp: "20260516T000001Z",
    body: "first",
  },
];

async function flushPromises(times = 4) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

describe("CardDetail message scroll state", () => {
  let root: Root | null = null;

  beforeEach(() => {
    testEnv.localStorage.clear();
    vi.clearAllMocks();
    mocks.messageListProps.length = 0;
    mocks.client.readCard.mockResolvedValue({
      ok: true,
      data: {
        archived: false,
        meta: card,
        entries,
      },
    });

    useConnectionStore.setState({ mode: "remote", status: "ready" });
    useWorkspaceStore.setState({
      activeSlug: "room",
      workspaces: [
        {
          slug: "room",
          workspace_name: "room",
          path: "/tmp/room",
          provider: "local",
          initialized: true,
        },
      ],
      loading: false,
      error: null,
      errorCode: null,
    });
    useChatStore.setState({
      currentUser: "lewis",
      users: ["lewis", "alice"],
      isGuest: false,
    });
    useAgentStore.setState({ agents: [], selectedAgentId: null });
    useCardStore.getState().resetForWorkspaceSwitch();
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("passes persisted card discussion scrollTop into MessageList and stores new positions", async () => {
    localStorage.setItem(
      "gitim-ui-state:runtime:room",
      JSON.stringify({
        messageScrollByScope: {
          "card:general/card-1": 240,
        },
      }),
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(
        <MemoryRouter initialEntries={["/cards/general/card-1"]}>
          <Routes>
            <Route path="/cards/:channel/:card_id" element={<CardDetail />} />
          </Routes>
        </MemoryRouter>,
      );
      await flushPromises();
    });

    const props = mocks.messageListProps.at(-1)!;
    expect(props.restoreScrollTop).toBe(240);

    const onScrollTopChange = props.onScrollTopChange as
      | ((scrollTop: number) => void)
      | undefined;
    expect(onScrollTopChange).toBeTypeOf("function");

    act(() => {
      onScrollTopChange?.(360);
    });

    expect(
      JSON.parse(localStorage.getItem("gitim-ui-state:runtime:room") ?? "{}")
        .messageScrollByScope,
    ).toEqual({
      "card:general/card-1": 360,
    });
  });
});
