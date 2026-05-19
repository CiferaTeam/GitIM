// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type React from "react";
import { ChatLayout } from "./chat-layout";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";

const mocks = vi.hoisted(() => ({
  client: {
    send: vi.fn(),
    read: vi.fn(),
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

vi.mock("../../lib/client", () => mocks.client);

vi.mock("../../hooks/use-media-query", () => ({
  useIsMobile: () => false,
}));

vi.mock("../../hooks/use-scroll-at-bottom", () => ({
  useScrollAtBottom: () => ({
    atBottom: true,
    scrollToBottom: vi.fn(),
  }),
}));

vi.mock("../cards/channel-card-drawer", () => ({
  ChannelCardDrawer: () => null,
}));

vi.mock("../flows/channel-active-runs", () => ({
  ChannelActiveRuns: () => null,
}));

vi.mock("../mobile/mobile-action-sheet", () => ({
  MobileActionSheet: () => null,
}));

vi.mock("../mobile/mobile-sidebar-drawer", () => ({
  MobileSidebarDrawer: () => null,
}));

vi.mock("../mobile/mobile-thread-overlay", () => ({
  MobileThreadOverlay: () => null,
}));

vi.mock("./header", () => ({
  ChatHeader: ({ children }: { children?: React.ReactNode }) => (
    <div data-testid="chat-header">{children}</div>
  ),
}));

vi.mock("./message-list", () => ({
  MessageList: () => <div data-testid="message-list" />,
}));

vi.mock("./scroll-to-bottom-button", () => ({
  ScrollToBottomButton: () => null,
}));

vi.mock("./sidebar", () => ({
  Sidebar: () => null,
}));

vi.mock("./thread-panel", () => ({
  ThreadPanel: () => null,
}));

vi.mock("./user-card", () => ({
  UserCard: () => null,
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function setTextareaValue(textarea: HTMLTextAreaElement, value: string) {
  const valueSetter = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  valueSetter?.call(textarea, value);
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("ChatLayout all mention send", () => {
  let root: Root | null = null;

  beforeEach(() => {
    testEnv.localStorage.clear();
    vi.clearAllMocks();
    mocks.client.send.mockResolvedValue({
      ok: true,
      data: { line_number: 7 },
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
    });
    useAgentStore.setState({ agents: [], selectedAgentId: null });
    useChatStore.setState({
      currentUser: "lewis",
      isGuest: false,
      users: ["lewis", "alice", "bob"],
      channels: [
        {
          name: "general",
          kind: "channel",
          unreadCount: 0,
          hasMention: false,
          members: ["lewis", "alice", "bob"],
        },
      ],
      archivedChannels: [],
      currentChannel: "general",
      messages: [],
      replyTo: null,
      highlightLine: null,
      pendingScrollLine: null,
      threadRoot: null,
      threadMessages: [],
      navHistory: [],
      hasMoreHistory: true,
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
    }
    root = null;
    document.body.innerHTML = "";
  });

  it("sends channel <@all> as concrete member mentions", async () => {
    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      setTextareaValue(textarea!, "<@all> please review");
      textarea!.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
        }),
      );
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mocks.client.send).toHaveBeenCalledWith(
      "room",
      "general",
      "<@lewis> <@alice> <@bob> please review",
      "lewis",
      0,
    );
  });
});
