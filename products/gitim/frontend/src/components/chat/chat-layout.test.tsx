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
import {
  incrementChatScopeUnread,
  readActiveChatScope,
  readChatScopeState,
  writeActiveChatScope,
  writeChatScopeViewAnchor,
} from "../../lib/chat-ui-state";

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
  MessageList: ({
    onMessageLinkClick,
    restoreAnchor,
  }: {
    onMessageLinkClick?: (channel: string, line: number) => void;
    restoreAnchor?: { line: number; offsetPx: number } | null;
  }) => (
    <div
      data-testid="message-list"
      data-restore-anchor-line={restoreAnchor?.line ?? ""}
      data-restore-anchor-offset={restoreAnchor?.offsetPx ?? ""}
    >
      <button
        data-testid="message-link"
        onClick={() => onMessageLinkClick?.("random", 42)}
      />
    </div>
  ),
}));

vi.mock("./scroll-to-bottom-button", () => ({
  ScrollToBottomButton: () => null,
}));

vi.mock("./sidebar", () => ({
  Sidebar: ({ onChannelSelect }: { onChannelSelect?: (name: string) => void }) => (
    <button
      data-testid="select-random-channel"
      onClick={() => onChannelSelect?.("random")}
    />
  ),
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
    mocks.client.read.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          {
            line_number: 42,
            point_to: 0,
            author: "alice",
            timestamp: "20260511T120000Z",
            body: "linked",
          },
        ],
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
          created_by: "lewis",
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

  it("previews routed recipients before sending", async () => {
    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      setTextareaValue(textarea!, "<@alice> please review");
      await Promise.resolve();
    });

    const preview = document.querySelector("[data-recipient-preview]");
    expect(preview).not.toBeNull();
    expect(preview?.textContent).toContain("@alice");
    expect(preview?.textContent).not.toContain("@bob");
  });

  it("shows the current user when they are the only routed recipient", async () => {
    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      setTextareaValue(textarea!, "hello");
      await Promise.resolve();
    });

    const preview = document.querySelector("[data-recipient-preview]");
    expect(preview?.textContent).toContain("@lewis");
    expect(preview?.textContent).not.toContain("no one else");
  });
  it("renders high-visibility routed recipient chips while replying", async () => {
    useChatStore.setState({
      channels: [
        {
          name: "general",
          kind: "channel",
          unreadCount: 0,
          hasMention: false,
          members: ["lewis", "alice", "bob"],
          created_by: "cfo",
        },
      ],
      replyTo: {
        line_number: 1,
        point_to: 0,
        author: "alice",
        timestamp: "20260511T120000Z",
        body: "please keep this visible",
      },
    });

    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      setTextareaValue(textarea!, "replying");
      await Promise.resolve();
    });

    const chips = Array.from(document.querySelectorAll("[data-recipient-chip]"));
    expect(chips).toHaveLength(2);
    expect(chips.map((chip) => chip.textContent)).toEqual(["@alice", "@cfo"]);
    for (const chip of chips) {
      expect(chip.className).toContain("route-recipient-nudge");
      expect(chip.className).toContain("text-primary");
    }
  });

  it("keeps the target line scroll intent after following a message link", async () => {
    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const link = document.querySelector<HTMLButtonElement>(
      "[data-testid='message-link']",
    );
    expect(link).not.toBeNull();

    await act(async () => {
      link!.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(useChatStore.getState().currentChannel).toBe("random");
    expect(useChatStore.getState().pendingScrollLine).toBe(42);
  });

  it("does not mark unread messages read when auto-selecting the fallback channel on mount", async () => {
    useChatStore.setState({
      currentChannel: null,
      channels: [
        {
          name: "general",
          kind: "channel",
          unreadCount: 10,
          hasMention: true,
          members: ["lewis", "alice", "bob"],
          created_by: "lewis",
        },
      ],
      messages: [],
    });

    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const general = useChatStore
      .getState()
      .channels.find((channel) => channel.name === "general");
    expect(useChatStore.getState().currentChannel).toBe("general");
    expect(general?.unreadCount).toBe(10);
    expect(general?.hasMention).toBe(true);
  });

  it("auto-selects the stored active scope when /chat mounts without a channel", async () => {
    writeActiveChatScope("runtime:room", "channel:random");
    useChatStore.setState({
      currentChannel: null,
      channels: [
        {
          name: "general",
          kind: "channel",
          unreadCount: 0,
          hasMention: false,
          members: ["lewis"],
          created_by: "lewis",
        },
        {
          name: "random",
          kind: "channel",
          unreadCount: 0,
          hasMention: false,
          members: ["lewis"],
          created_by: "lewis",
        },
      ],
      messages: [],
    });

    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(useChatStore.getState().currentChannel).toBe("random");
    expect(mocks.client.read).toHaveBeenCalledWith("room", "random", 50, undefined);
  });

  it("restores persisted line anchor when /chat remounts with an existing current channel", async () => {
    writeChatScopeViewAnchor("runtime:room", "channel:general", {
      line: 321,
      offsetPx: 18,
    });

    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const list = document.querySelector<HTMLElement>(
      "[data-testid='message-list']",
    );
    expect(list?.dataset.restoreAnchorLine).toBe("321");
    expect(list?.dataset.restoreAnchorOffset).toBe("18");
  });

  it("uses firstUnreadLine when opening a channel with persisted unread state", async () => {
    useChatStore.setState({
      channels: [
        {
          name: "general",
          kind: "channel",
          unreadCount: 0,
          hasMention: false,
          members: ["lewis"],
          created_by: "lewis",
        },
        {
          name: "random",
          kind: "channel",
          unreadCount: 2,
          hasMention: true,
          members: ["lewis", "alice"],
          created_by: "lewis",
        },
      ],
      currentChannel: "general",
      messages: [],
    });
    incrementChatScopeUnread("runtime:room", "channel:random", {
      count: 2,
      hasMention: true,
      firstUnreadLine: 88,
    });
    expect(readChatScopeState("runtime:room", "channel:random")).toMatchObject({
      unreadCount: 2,
      firstUnreadLine: 88,
    });

    await act(async () => {
      root!.render(<ChatLayout />);
      await Promise.resolve();
    });

    const selectRandom = document.querySelector<HTMLButtonElement>(
      "[data-testid='select-random-channel']",
    );
    expect(selectRandom).not.toBeNull();

    await act(async () => {
      selectRandom!.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(useChatStore.getState().currentChannel).toBe("random");
    expect(useChatStore.getState().pendingScrollLine).toBe(88);
    expect(readActiveChatScope("runtime:room")).toBe("channel:random");
    expect(readChatScopeState("runtime:room", "channel:random")).toMatchObject({
      unreadCount: 0,
      hasMention: false,
      firstUnreadLine: null,
    });
    const random = useChatStore
      .getState()
      .channels.find((channel) => channel.name === "random");
    expect(random?.unreadCount).toBe(0);
    expect(random?.hasMention).toBe(false);
  });
});
