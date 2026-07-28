// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import {
  MemoryRouter,
  Route,
  Routes,
  useNavigate,
  type NavigateFunction,
} from "react-router";

import type { FlowDocument, FlowRunDetail } from "@/lib/types";

vi.hoisted(() => {
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

  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: createMemoryStorage(),
  });
});

const mocks = vi.hoisted(() => ({
  client: {
    cancelFlowRun: vi.fn(),
    getFlow: vi.fn(),
    getFlowRun: vi.fn(),
  },
  flowDagNodes: [] as Array<Array<Record<string, unknown>>>,
}));

vi.mock("@/lib/client", () => mocks.client);

vi.mock("./flow-dag", () => ({
  FlowDAG: ({ nodes }: { nodes: Array<Record<string, unknown>> }) => {
    mocks.flowDagNodes.push(nodes);
    return <pre data-testid="flow-dag-nodes">{JSON.stringify(nodes)}</pre>;
  },
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

import { useFlowRunStore } from "@/hooks/use-flow-run-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { RunDetail } from "./run-detail";

async function flushPromises(times = 6) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

const runDetail: FlowRunDetail = {
  run_id: "20260702T141906-e4dc17",
  flow_slug: "multi-agent-sop-dev",
  channel: "gitim-pr47-sop-0702",
  started_at: "2026-07-02T14:19:06Z",
  started_by: "cfo",
  status: "in_progress",
  updated_at: "2026-07-02T22:20:00Z",
  nodes: [
    {
      id: "scope-gate",
      status: "done",
      actor: "cfo",
      completed_at: "20260702T142025Z",
    },
    {
      id: "requirements-analysis",
      status: "in_progress",
      actor: "dev-wingman",
    },
    { id: "plan-review", status: "pending" },
  ],
};

const flowDocument: FlowDocument = {
  slug: "multi-agent-sop-dev",
  name: "Multi-Agent SOP Dev",
  description: "Role-based development flow",
  created_by: "cfo",
  created_at: "2026-05-25T00:00:00Z",
  nodes: [
    {
      id: "scope-gate",
      type: "human_review",
      owner: "cfo",
      prompt: "Check scope.",
    },
    {
      id: "requirements-analysis",
      type: "agent_mention",
      owner: "dev-wingman",
      needs: ["scope-gate"],
      prompt: "Analyze requirements.",
    },
    {
      id: "plan-review",
      type: "human_review",
      owner: "cfo",
      needs: ["requirements-analysis"],
      prompt: "Review plan.",
    },
  ],
  raw_markdown: "",
};

function NavigationProbe({
  onNavigate,
}: {
  onNavigate: (navigate: NavigateFunction) => void;
}) {
  onNavigate(useNavigate());
  return null;
}

function renderRunDetail(
  initialEntry = "/runs/20260702T141906-e4dc17",
  onNavigate?: (navigate: NavigateFunction) => void,
) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(
      <MemoryRouter initialEntries={[initialEntry]}>
        {onNavigate ? <NavigationProbe onNavigate={onNavigate} /> : null}
        <Routes>
          <Route path="/runs/:runId" element={<RunDetail />} />
        </Routes>
      </MemoryRouter>,
    );
  });
  return { container, root };
}

function nodeRow(container: HTMLElement, nodeId: string): HTMLElement {
  const label = Array.from(container.querySelectorAll("div")).find(
    (el) => el.textContent === nodeId,
  );
  expect(label).toBeTruthy();
  const row = label!.parentElement;
  expect(row).toBeTruthy();
  return row!;
}

describe("RunDetail", () => {
  let root: Root | null = null;

  beforeEach(() => {
    vi.clearAllMocks();
    mocks.flowDagNodes.length = 0;
    mocks.client.getFlowRun.mockResolvedValue({ ok: true, data: runDetail });
    mocks.client.getFlow.mockResolvedValue({ ok: true, data: flowDocument });
    useFlowRunStore.getState().resetForWorkspaceSwitch();
    useWorkspaceStore.setState({
      activeSlug: "room",
      workspaces: [
        {
          slug: "room",
          workspace_name: "Room",
          path: "/tmp/room",
          provider: "local",
          initialized: true,
        },
      ],
      loading: false,
      error: null,
      errorCode: null,
    });
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
    }
    root = null;
    document.body.innerHTML = "";
    vi.unstubAllGlobals();
  });

  it("renders the run DAG with dependencies from the flow template", async () => {
    const rendered = renderRunDetail();
    root = rendered.root;

    await act(async () => {
      await flushPromises();
    });

    expect(mocks.client.getFlow).toHaveBeenCalledWith(
      "room",
      "multi-agent-sop-dev",
    );
    const latestNodes = mocks.flowDagNodes.at(-1) ?? [];
    expect(latestNodes).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          id: "requirements-analysis",
          needs: ["scope-gate"],
          prompt: "Analyze requirements.",
        }),
        expect.objectContaining({
          id: "plan-review",
          needs: ["requirements-analysis"],
        }),
      ]),
    );
  });

  it("uses muted semantic status backgrounds for in-progress node rows", async () => {
    const rendered = renderRunDetail();
    root = rendered.root;

    await act(async () => {
      await flushPromises();
    });

    const row = nodeRow(rendered.container, "requirements-analysis");
    expect(row.className).toContain("bg-warning/10");
    expect(row.className).toContain("border-warning/30");
    expect(row.className).not.toContain("bg-yellow-100");
  });

  it("places paginated run steps above the DAG", async () => {
    const nodes = Array.from({ length: 8 }, (_, index) => ({
      id: `step-${index + 1}`,
      status: index === 0 ? ("done" as const) : ("pending" as const),
    }));
    mocks.client.getFlowRun.mockResolvedValueOnce({
      ok: true,
      data: { ...runDetail, nodes },
    });

    const rendered = renderRunDetail();
    root = rendered.root;

    await act(async () => {
      await flushPromises();
    });

    const stepsSection = rendered.container.querySelector<HTMLElement>(
      "[data-testid='run-steps']",
    );
    const dagSection = rendered.container.querySelector<HTMLElement>(
      "[data-testid='run-dag']",
    );
    expect(stepsSection).not.toBeNull();
    expect(dagSection).not.toBeNull();
    expect(
      stepsSection!.compareDocumentPosition(dagSection!) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    expect(stepsSection!.textContent).toContain("1-6 of 8");
    expect(stepsSection!.textContent).toContain("step-1");
    expect(stepsSection!.textContent).toContain("step-6");
    expect(stepsSection!.textContent).not.toContain("step-7");

    const nextButton = stepsSection!.querySelector<HTMLButtonElement>(
      "button[aria-label='Next step page']",
    );
    expect(nextButton).not.toBeNull();

    await act(async () => {
      nextButton!.click();
      await Promise.resolve();
    });

    expect(stepsSection!.textContent).toContain("7-8 of 8");
    expect(stepsSection!.textContent).not.toContain("step-1");
    expect(stepsSection!.textContent).toContain("step-7");
    expect(stepsSection!.textContent).toContain("step-8");
  });

  it("keeps the newest route selected when an older run request resolves last", async () => {
    const oldRequest = deferred<{ ok: true; data: FlowRunDetail }>();
    const newRequest = deferred<{ ok: true; data: FlowRunDetail }>();
    const newerRun = {
      ...runDetail,
      run_id: "20260702T150000-new001",
      flow_slug: "newer-flow",
    };
    mocks.client.getFlowRun.mockImplementation(
      (_slug: string, id: string) =>
        id === runDetail.run_id ? oldRequest.promise : newRequest.promise,
    );

    let navigate: NavigateFunction = () => {};
    const rendered = renderRunDetail(
      `/runs/${runDetail.run_id}`,
      (nextNavigate) => {
        navigate = nextNavigate;
      },
    );
    root = rendered.root;

    await act(async () => {
      navigate(`/runs/${newerRun.run_id}`);
      await flushPromises();
    });

    await act(async () => {
      newRequest.resolve({ ok: true, data: newerRun });
      await flushPromises();
    });
    expect(useFlowRunStore.getState().selectedRun?.run_id).toBe(
      newerRun.run_id,
    );

    await act(async () => {
      oldRequest.resolve({ ok: true, data: runDetail });
      await flushPromises();
    });

    expect(useFlowRunStore.getState().selectedRun?.run_id).toBe(
      newerRun.run_id,
    );
    expect(rendered.container.textContent).toContain(newerRun.run_id);
  });

  it("ignores a completed cancel after the user switches to another run", async () => {
    const cancelRequest = deferred<{ ok: true }>();
    const newerRun = {
      ...runDetail,
      run_id: "20260702T160000-new002",
      flow_slug: "newer-flow",
    };
    mocks.client.cancelFlowRun.mockReturnValue(cancelRequest.promise);
    mocks.client.getFlowRun.mockImplementation(
      (_slug: string, id: string) =>
        Promise.resolve({
          ok: true,
          data: id === newerRun.run_id ? newerRun : runDetail,
        }),
    );
    vi.stubGlobal("confirm", vi.fn(() => true));

    let navigate: NavigateFunction = () => {};
    const rendered = renderRunDetail(
      `/runs/${runDetail.run_id}`,
      (nextNavigate) => {
        navigate = nextNavigate;
      },
    );
    root = rendered.root;
    await act(async () => {
      await flushPromises();
    });

    await act(async () => {
      Array.from(rendered.container.querySelectorAll("button"))
        .find((button) => button.textContent?.trim() === "Cancel run")
        ?.click();
      await flushPromises();
    });

    await act(async () => {
      navigate(`/runs/${newerRun.run_id}`);
      await flushPromises();
    });
    expect(useFlowRunStore.getState().selectedRun?.run_id).toBe(
      newerRun.run_id,
    );

    mocks.client.getFlowRun.mockClear();
    await act(async () => {
      cancelRequest.resolve({ ok: true });
      await flushPromises();
    });

    expect(mocks.client.getFlowRun).not.toHaveBeenCalled();
    expect(useFlowRunStore.getState().selectedRun?.run_id).toBe(
      newerRun.run_id,
    );
    expect(rendered.container.textContent).toContain(newerRun.run_id);
  });
});
