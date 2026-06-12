// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Sidebar } from "./sidebar";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import { useProjectStore } from "../../hooks/use-project-store";
import type { Channel, Project } from "../../lib/types";

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
    listProjects: vi.fn().mockResolvedValue([]),
    unarchiveChannel: vi.fn(),
    unarchiveDm: vi.fn(),
  };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function channel(
  name: string,
  unreadCount = 0,
  hasMention = false,
  project?: string | null,
): Channel {
  return {
    name,
    kind: "channel",
    unreadCount,
    hasMention,
    members: ["lewis"],
    project: project ?? null,
  };
}

function project(slug: string): Project {
  return {
    slug,
    meta: {
      display_name: slug,
      created_by: "lewis",
      created_at: "2026-01-01T00:00:00Z",
      introduction: "",
    },
    channel_count: 0,
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
    useProjectStore.setState({ projects: [], loading: false, error: null });
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
    useProjectStore.getState().reset();
  });

  it("renders channels in lexicographic order (tree-based sort: pinned first, then lex)", () => {
    act(() => {
      root?.render(
        <Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />,
      );
    });

    // Tree sort: no pins → alphabetical. "general" < "infra" < "random".
    // Unread state no longer drives top-level order (buildSidebarTree uses lex sort).
    expect(visibleChannelNames(container)).toEqual([
      "general",
      "infra",
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

    // random is pinned → floats to top. Remaining channels sort lexicographically.
    expect(visibleChannelNames(container)).toEqual([
      "random",
      "general",
      "infra",
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

    // Tree sort: alphabetical, "random" is folded → ["general", "infra"] visible.
    expect(visibleChannelNames(container)).toEqual(["general", "infra"]);
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

describe("Sidebar with projects", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  function setup(
    channels: Channel[],
    projects: import("../../lib/types").Project[],
  ) {
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
    useProjectStore.setState({ projects, loading: false, error: null });
    useChatStore.getState().resetForWorkspaceSwitch();
    useChatStore.setState({
      currentUser: "lewis",
      users: ["lewis"],
      channels,
      currentChannel: channels[0]?.name ?? null,
    });
  }

  afterEach(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    container.remove();
    useAgentStore.getState().resetForWorkspaceSwitch();
    useChatStore.getState().resetForWorkspaceSwitch();
    useProjectStore.getState().reset();
  });

  it("renders project folder header with channel count", () => {
    setup(
      [channel("dev", 0, false, "design"), channel("ui", 0, false, "design")],
      [project("design")],
    );

    act(() => {
      root?.render(<Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />);
    });

    const header = container.querySelector('[data-testid="sidebar-project-header"]');
    expect(header).not.toBeNull();
    // Channel children are collapsed by default — not visible yet
    expect(
      container.querySelectorAll('[data-testid="sidebar-project-channel-item"]').length,
    ).toBe(0);
  });

  it("expands project folder on click to show children", () => {
    setup(
      [channel("dev", 0, false, "design"), channel("ui", 0, false, "design")],
      [project("design")],
    );

    act(() => {
      root?.render(<Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />);
    });

    // Click folder header to expand
    act(() => {
      container
        .querySelector<HTMLElement>('[data-testid="sidebar-project-header"]')
        ?.click();
    });

    const children = container.querySelector('[data-testid="sidebar-project-children"]');
    expect(children).not.toBeNull();
    expect(
      container.querySelectorAll('[data-testid="sidebar-project-channel-item"]').length,
    ).toBe(2);
  });

  it("hides empty project (no channels assigned)", () => {
    setup(
      [channel("general")],
      [project("design")], // design has no channels → hidden
    );

    act(() => {
      root?.render(<Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />);
    });

    expect(
      container.querySelectorAll('[data-testid="sidebar-project-item"]').length,
    ).toBe(0);
    // The unassigned channel is still visible
    expect(
      container.querySelectorAll('[data-testid="sidebar-channel-item"]').length,
    ).toBe(1);
  });

  it("pinning a project writes it to localStorage with projects key", () => {
    setup(
      [channel("dev", 0, false, "design"), channel("random")],
      [project("design")],
    );

    act(() => {
      root?.render(<Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />);
    });

    // Pin button is on the project header — click it
    const pinBtn = container.querySelector<HTMLElement>(
      '[data-testid="sidebar-project-header"] button[aria-label*="Pin project"]',
    );
    expect(pinBtn).not.toBeNull();
    act(() => {
      pinBtn?.click();
    });

    const stored = testEnv.localStorage.getItem("gitim-pinned-conversations:runtime:room");
    expect(stored).not.toBeNull();
    const parsed = JSON.parse(stored!) as { projects?: string[] };
    expect(parsed.projects).toContain("design");
  });

  it("backward-compat: old pinned schema without projects key does not crash", () => {
    testEnv.localStorage.setItem(
      "gitim-pinned-conversations:runtime:room",
      JSON.stringify({ channels: ["random"], dms: [] }),
    );
    setup([channel("random")], []);

    // Should render without throwing
    act(() => {
      root?.render(<Sidebar onChannelSelect={vi.fn()} onStartDm={vi.fn()} />);
    });

    const items = container.querySelectorAll('[data-testid="sidebar-channel-item"]');
    expect(items.length).toBe(1);
  });
});
