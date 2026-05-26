// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Sidebar } from "./sidebar";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import type { Channel } from "../../lib/types";

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

vi.mock("../../lib/client", async () => {
  const actual = await vi.importActual<typeof import("../../lib/client")>(
    "../../lib/client",
  );
  return {
    ...actual,
    archiveDm: vi.fn(),
    channels: vi.fn(),
    createChannel: vi.fn(),
    listArchivedChannels: vi.fn(),
    listArchivedDms: vi.fn(),
    unarchiveChannel: vi.fn(),
    unarchiveDm: vi.fn(),
  };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function channel(
  name: string,
  unreadCount = 0,
  hasMention = false,
): Channel {
  return {
    name,
    kind: "channel",
    unreadCount,
    hasMention,
    members: ["lewis"],
  };
}

function visibleChannelNames(container: HTMLElement): string[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>('[data-testid="sidebar-channel-item"]'),
  ).map((item) => {
    const label = item.querySelector("button span");
    return label?.textContent ?? "";
  });
}

describe("Sidebar channel ordering", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  beforeEach(() => {
    testEnv.localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

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
    useChatStore.getState().resetForWorkspaceSwitch();
    useChatStore.setState({
      currentUser: "lewis",
      users: ["lewis", "alice"],
      channels: [
        channel("general"),
        channel("infra", 2),
        channel("random"),
      ],
      currentChannel: "general",
    });
  });

  afterEach(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    container.remove();
    useAgentStore.getState().resetForWorkspaceSwitch();
    useChatStore.getState().resetForWorkspaceSwitch();
  });

  it("moves unread channels above read channels", () => {
    act(() => {
      root?.render(
        <Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />,
      );
    });

    expect(visibleChannelNames(container)).toEqual([
      "infra",
      "general",
      "random",
    ]);
  });

  it("pins float above unread non-pinned channels", () => {
    testEnv.localStorage.setItem(
      "gitim-pinned-conversations:runtime:room",
      JSON.stringify({ channels: ["random"], dms: [] }),
    );

    act(() => {
      root?.render(
        <Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />,
      );
    });

    // random is pinned (no unread); infra has unread but no pin; general is
    // neither. New behavior: pinned segment is rendered first, then the
    // unfolded-unpinned segment (which still uses unread-first ordering).
    expect(visibleChannelNames(container)).toEqual([
      "random",
      "infra",
      "general",
    ]);
  });

  it("hides folded channels from the main list and groups them under a Folded toggle", () => {
    testEnv.localStorage.setItem(
      "gitim-folded-channels:runtime:room",
      JSON.stringify(["random"]),
    );

    act(() => {
      root?.render(
        <Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />,
      );
    });

    expect(visibleChannelNames(container)).toEqual(["infra", "general"]);
    expect(
      container.querySelector('[data-testid="sidebar-folded-section-toggle"]'),
    ).not.toBeNull();
    // Folded section is collapsed by default, so the folded row isn't rendered.
    expect(
      container.querySelector('[data-testid="sidebar-folded-channel-item"]'),
    ).toBeNull();
  });

  it("indents channels rendered inside the Folded section", () => {
    testEnv.localStorage.setItem(
      "gitim-folded-channels:runtime:room",
      JSON.stringify(["random"]),
    );

    act(() => {
      root?.render(
        <Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />,
      );
    });
    act(() => {
      container
        .querySelector<HTMLButtonElement>(
          '[data-testid="sidebar-folded-section-toggle"]',
        )
        ?.click();
    });

    const foldedList = container.querySelector(
      '[data-testid="sidebar-folded-channel-list"]',
    );

    expect(foldedList).not.toBeNull();
    expect(foldedList?.className).toContain("pl-4");
    expect(
      foldedList?.querySelector('[data-testid="sidebar-folded-channel-item"]'),
    ).not.toBeNull();
  });

  it("renders a channel under the pin segment when stale state lists it as both pinned and folded", () => {
    // Pin and fold are mutually exclusive in the UI, but a stale localStorage
    // pair could co-list a channel. Pin wins for rendering — pinned filter
    // runs first, then the fold filter excludes anything already pinned.
    testEnv.localStorage.setItem(
      "gitim-pinned-conversations:runtime:room",
      JSON.stringify({ channels: ["infra"], dms: [] }),
    );
    testEnv.localStorage.setItem(
      "gitim-folded-channels:runtime:room",
      JSON.stringify(["infra"]),
    );

    act(() => {
      root?.render(
        <Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />,
      );
    });

    expect(visibleChannelNames(container)).toEqual(["infra", "general", "random"]);
    expect(
      container.querySelector('[data-testid="sidebar-folded-section-toggle"]'),
    ).toBeNull();
  });
});
