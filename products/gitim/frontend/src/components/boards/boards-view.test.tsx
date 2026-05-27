// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const listBoardsMock = vi.hoisted(() => vi.fn());
const showBoardMock = vi.hoisted(() => vi.fn());

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

vi.mock("@/lib/client", () => ({
  listBoards: listBoardsMock,
  showBoard: showBoardMock,
}));

import { BoardsView } from "./boards-view";
import { useBoardStore } from "@/hooks/use-board-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type {
  ApiResponse,
  BoardReadResponse,
  BoardSummary,
} from "@/lib/types";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function boardSummary(handler: string, summary = `${handler} board`): BoardSummary {
  return {
    handler,
    path: `showboards/${handler}/board.md`,
    updated_at: "20260509T120000Z",
    status: "working",
    summary,
    tags: ["mobile", "board"],
  };
}

function boardRead(handler: string, summary = `${handler} board`): BoardReadResponse {
  return {
    handler,
    path: `showboards/${handler}/board.md`,
    meta: {
      version: 1,
      handler,
      updated_at: "20260509T120000Z",
      status: "working",
      summary,
      tags: ["mobile", "board"],
    },
    body: `## 当前状态\n\n${summary}`,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

async function flushPromises(count = 2) {
  for (let i = 0; i < count; i += 1) {
    await Promise.resolve();
  }
}

describe("BoardsView", () => {
  let root: Root | null = null;

  beforeEach(() => {
    testEnv.localStorage.clear();
    listBoardsMock.mockReset();
    showBoardMock.mockReset();
    useBoardStore.getState().resetForWorkspaceSwitch();
    useWorkspaceStore.setState({
      activeSlug: "phone",
      workspaces: [{
        slug: "phone",
        workspace_name: "Phone",
        path: "indexeddb://gitim",
        provider: "github",
        initialized: true,
      }],
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

  it("loads board list and selected board detail", async () => {
    listBoardsMock.mockResolvedValueOnce({
      ok: true,
      data: {
        boards: [{
          handler: "alice",
          path: "showboards/alice/board.md",
          updated_at: "20260509T120000Z",
          status: "working",
          summary: "Shipping board UI",
          tags: ["mobile", "board"],
        }],
      },
    });
    showBoardMock.mockResolvedValueOnce({
      ok: true,
      data: {
        handler: "alice",
        path: "showboards/alice/board.md",
        meta: {
          version: 1,
          handler: "alice",
          updated_at: "20260509T120000Z",
          status: "working",
          summary: "Shipping board UI",
          tags: ["mobile", "board"],
        },
        body: "## 当前状态\n\n正在接入移动端。\n\n## 待确认\n\n- poll 刷新",
      },
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(listBoardsMock).toHaveBeenCalledWith("phone");
    expect(showBoardMock).toHaveBeenCalledWith("phone", "alice");
    expect(container.textContent).toContain("@alice");
    expect(container.textContent).toContain("Shipping board UI");
    expect(container.textContent).toContain("当前状态");
    expect(container.textContent).toContain("正在接入移动端。");
    expect(container.textContent).toContain("poll 刷新");
  });

  it("constrains long board status labels in list headers", async () => {
    const longStatus = "Phase1b1: 4个对标产品并行调研，两 coder 互评";
    const robinRead = boardRead("robin", "Coordinating comparison research");
    listBoardsMock.mockResolvedValueOnce({
      ok: true,
      data: {
        boards: [{
          ...boardSummary("robin", "Coordinating comparison research"),
          status: longStatus,
        }],
      },
    });
    showBoardMock.mockResolvedValueOnce({
      ok: true,
      data: {
        ...robinRead,
        meta: {
          ...robinRead.meta,
          status: longStatus,
        },
      },
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await flushPromises();
    });

    const robinButton = Array.from(container.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("@robin"),
    );
    expect(robinButton).toBeDefined();

    const status = Array.from(robinButton!.querySelectorAll("span")).find(
      (span) => span.textContent === longStatus,
    );
    expect(status?.getAttribute("title")).toBe(longStatus);
    expect(status?.className).toContain("max-w-[55%]");
    expect(status?.className).toContain("truncate");
    expect(status?.className).not.toContain("shrink-0");
  });

  it("refresh button reloads the board list", async () => {
    listBoardsMock.mockResolvedValue({
      ok: true,
      data: { boards: [] },
    });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await Promise.resolve();
    });

    const button = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Refresh boards"]',
    );
    expect(button).not.toBeNull();

    await act(async () => {
      button?.click();
      await Promise.resolve();
    });

    expect(listBoardsMock).toHaveBeenCalledTimes(2);
  });

  it("refresh button reloads the selected board detail", async () => {
    listBoardsMock
      .mockResolvedValueOnce({
        ok: true,
        data: { boards: [boardSummary("alice", "Old summary")] },
      })
      .mockResolvedValueOnce({
        ok: true,
        data: { boards: [boardSummary("alice", "New summary")] },
      });
    showBoardMock
      .mockResolvedValueOnce({
        ok: true,
        data: boardRead("alice", "Old detail"),
      })
      .mockResolvedValueOnce({
        ok: true,
        data: boardRead("alice", "New detail"),
      });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await flushPromises();
    });

    expect(container.textContent).toContain("Old detail");

    const button = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Refresh boards"]',
    );
    expect(button).not.toBeNull();

    await act(async () => {
      button?.click();
      await flushPromises();
    });

    expect(listBoardsMock).toHaveBeenCalledTimes(2);
    expect(showBoardMock).toHaveBeenCalledTimes(2);
    expect(showBoardMock).toHaveBeenNthCalledWith(1, "phone", "alice");
    expect(showBoardMock).toHaveBeenNthCalledWith(2, "phone", "alice");
    expect(container.textContent).toContain("New detail");
    expect(container.textContent).not.toContain("Old detail");
  });

  it("clears a stale detail error after a later board loads", async () => {
    listBoardsMock.mockResolvedValueOnce({
      ok: true,
      data: {
        boards: [
          boardSummary("alice", "Alice summary"),
          boardSummary("bob", "Bob summary"),
        ],
      },
    });
    showBoardMock
      .mockResolvedValueOnce({
        ok: false,
        error: "Alice detail failed",
      })
      .mockResolvedValueOnce({
        ok: true,
        data: boardRead("bob", "Bob detail"),
      });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await flushPromises();
    });

    expect(container.textContent).toContain("Alice detail failed");

    const bobButton = Array.from(container.querySelectorAll("button")).find((button) =>
      button.textContent?.includes("@bob"),
    );
    expect(bobButton).toBeDefined();

    await act(async () => {
      bobButton?.click();
      await flushPromises();
    });

    expect(showBoardMock).toHaveBeenNthCalledWith(2, "phone", "bob");
    expect(container.textContent).toContain("Bob detail");
    expect(container.textContent).not.toContain("Alice detail failed");
  });

  it("ignores a stale list response after the workspace changes", async () => {
    const stalePhoneList = deferred<ApiResponse<{ boards: BoardSummary[] }>>();
    listBoardsMock
      .mockReturnValueOnce(stalePhoneList.promise)
      .mockResolvedValueOnce({
        ok: true,
        data: { boards: [boardSummary("bob", "Tablet board")] },
      });
    showBoardMock.mockImplementation((_slug: string, handler: string) =>
      Promise.resolve({
        ok: true,
        data: boardRead(handler, `${handler} detail`),
      }),
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await flushPromises();
    });
    expect(listBoardsMock).toHaveBeenCalledWith("phone");

    await act(async () => {
      useWorkspaceStore.setState({
        activeSlug: "tablet",
        workspaces: [{
          slug: "tablet",
          workspace_name: "Tablet",
          path: "indexeddb://tablet",
          provider: "github",
          initialized: true,
        }],
      });
      await flushPromises();
    });
    expect(listBoardsMock).toHaveBeenCalledWith("tablet");
    expect(container.textContent).toContain("@bob");

    await act(async () => {
      stalePhoneList.resolve({
        ok: true,
        data: { boards: [boardSummary("alice", "Stale phone board")] },
      });
      await flushPromises();
    });

    expect(container.textContent).toContain("@bob");
    expect(container.textContent).not.toContain("@alice");
    expect(useBoardStore.getState().boards.map((board) => board.handler)).toEqual([
      "bob",
    ]);
  });

  it("keeps the newest refresh result when an earlier refresh returns later", async () => {
    const firstRefresh = deferred<ApiResponse<{ boards: BoardSummary[] }>>();
    const secondRefresh = deferred<ApiResponse<{ boards: BoardSummary[] }>>();
    listBoardsMock
      .mockResolvedValueOnce({
        ok: true,
        data: { boards: [boardSummary("alice", "Initial board")] },
      })
      .mockReturnValueOnce(firstRefresh.promise)
      .mockReturnValueOnce(secondRefresh.promise);
    showBoardMock.mockImplementation((_slug: string, handler: string) =>
      Promise.resolve({
        ok: true,
        data: boardRead(handler, `${handler} detail`),
      }),
    );

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<BoardsView />);
      await flushPromises();
    });

    const button = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Refresh boards"]',
    );
    expect(button).not.toBeNull();

    await act(async () => {
      button?.click();
      await flushPromises();
      button?.click();
      await flushPromises();
    });
    expect(listBoardsMock).toHaveBeenCalledTimes(3);

    await act(async () => {
      secondRefresh.resolve({
        ok: true,
        data: { boards: [boardSummary("carol", "Newest board")] },
      });
      await flushPromises();
    });
    expect(container.textContent).toContain("@carol");

    await act(async () => {
      firstRefresh.resolve({
        ok: true,
        data: { boards: [boardSummary("bob", "Older board")] },
      });
      await flushPromises();
    });

    expect(container.textContent).toContain("@carol");
    expect(container.textContent).not.toContain("@bob");
    expect(useBoardStore.getState().boards.map((board) => board.handler)).toEqual([
      "carol",
    ]);
  });
});
