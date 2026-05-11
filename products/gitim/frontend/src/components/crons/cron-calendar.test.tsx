// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const getCronTimelineMock = vi.hoisted(() => vi.fn());
const getCronRunBodyMock = vi.hoisted(() => vi.fn());
const showCronMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/client", () => ({
  getCronTimeline: getCronTimelineMock,
  getCronRunBody: getCronRunBodyMock,
  showCron: showCronMock,
}));

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
  return null;
});

import { CronCalendar } from "./cron-calendar";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

async function flushPromises(times = 3) {
  for (let i = 0; i < times; i++) {
    await Promise.resolve();
  }
}

function makeRoot() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  return { container, root: createRoot(container) };
}

describe("CronCalendar", () => {
  let root: Root | null = null;

  beforeEach(() => {
    getCronTimelineMock.mockReset();
    getCronRunBodyMock.mockReset();
    showCronMock.mockReset();
    useWorkspaceStore.setState({
      activeSlug: "phone",
      workspaces: [{
        slug: "phone",
        workspace_name: "Phone",
        path: "/tmp/phone",
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

  it("renders the empty-window message when the timeline returns no entries", async () => {
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: { entries: [] },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    expect(getCronTimelineMock).toHaveBeenCalledWith(
      "phone",
      expect.stringMatching(/^\d{4}-\d{2}-01T00:00:00\.000Z$/),
      expect.stringMatching(/^\d{4}-\d{2}-\d{2}T23:59:59\.000Z$/),
      expect.any(AbortSignal),
    );
    expect(container.textContent).toContain("周期任务");
    expect(container.textContent).toContain("这个时间窗内没有计划任务");
  });

  it("renders past, future, and missed entries with kind-specific classes", async () => {
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    // Three different days inside the current UTC month so all entries
    // appear regardless of the actual calendar position.
    const pastTs = `${year}-${pad(month)}-02T09:00:00Z`;
    const missedTs = `${year}-${pad(month)}-03T09:00:00Z`;
    const futureTs = `${year}-${pad(month)}-28T09:00:00Z`;

    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          {
            ts: pastTs,
            kind: "past",
            cron_name: "weekly-report",
            thread_url: `/workspaces/phone/crons/weekly-report/runs/${pastTs.replace(/:/g, "-")}`,
          },
          {
            ts: missedTs,
            kind: "missed",
            cron_name: "daily-standup",
            reason: "no thread file present",
          },
          {
            ts: futureTs,
            kind: "future",
            cron_name: "weekly-report",
          },
        ],
        truncated: false,
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    // All three cron names appear somewhere in the calendar.
    expect(container.textContent).toContain("weekly-report");
    expect(container.textContent).toContain("daily-standup");

    // Each kind has a dot with the right design-token background class.
    const html = container.innerHTML;
    expect(html).toContain("bg-success");
    expect(html).toContain("bg-primary");
    expect(html).toContain("bg-error");
  });

  it("surfaces the truncated banner when the timeline reports truncation", async () => {
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: { entries: [], truncated: true },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    expect(container.textContent).toContain("部分结果被截断");
  });

  it("re-fetches with a new window when the user navigates months", async () => {
    getCronTimelineMock.mockResolvedValue({ ok: true, data: { entries: [] } });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    const initialCalls = getCronTimelineMock.mock.calls.length;
    expect(initialCalls).toBeGreaterThan(0);

    // Click "Next month" — the aria-label is set on the chevron button.
    const next = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Next month"]',
    );
    expect(next).not.toBeNull();
    await act(async () => {
      next?.click();
      await flushPromises();
    });

    expect(getCronTimelineMock.mock.calls.length).toBeGreaterThan(initialCalls);
    // The new call must have a different `from` than the first one.
    const firstFrom = getCronTimelineMock.mock.calls[0][1];
    const latestFrom = getCronTimelineMock.mock.calls.at(-1)?.[1];
    expect(latestFrom).not.toBe(firstFrom);
  });

  it("renders +N more for days with more than the visible cap", async () => {
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    const day = `${year}-${pad(month)}-15`;
    const baseTs = (h: number, mi: number) =>
      `${day}T${pad(h)}:${pad(mi)}:00Z`;
    // 5 entries on the same day → 3 visible + "+2 more".
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          { ts: baseTs(8, 0), kind: "past", cron_name: "job-a" },
          { ts: baseTs(9, 0), kind: "past", cron_name: "job-b" },
          { ts: baseTs(10, 0), kind: "past", cron_name: "job-c" },
          { ts: baseTs(11, 0), kind: "future", cron_name: "job-d" },
          { ts: baseTs(12, 0), kind: "future", cron_name: "job-e" },
        ],
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    expect(container.textContent).toContain("+2 more");
  });
});
