// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const getCronRunBodyMock = vi.hoisted(() => vi.fn());
const showCronMock = vi.hoisted(() => vi.fn());

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

vi.mock("@/lib/client", () => ({
  getCronRunBody: getCronRunBodyMock,
  showCron: showCronMock,
}));

import { CronDayPanel } from "./cron-day-panel";
import type { CronTimelineEntry } from "@/lib/types";

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

const PAST_ENTRY: CronTimelineEntry = {
  ts: "2026-05-11T09:00:00Z",
  kind: "past",
  cron_name: "weekly-report",
  target: "alice",
  thread_url: "/workspaces/phone/crons/weekly-report/runs/2026-05-11T09-00-00Z",
};

const FUTURE_ENTRY: CronTimelineEntry = {
  ts: "2026-05-18T09:00:00Z",
  kind: "future",
  cron_name: "weekly-report",
  target: "alice",
};

const MISSED_ENTRY: CronTimelineEntry = {
  ts: "2026-05-12T09:30:00Z",
  kind: "missed",
  cron_name: "daily-standup",
  target: "bob",
  reason: "no thread file present",
};

describe("CronDayPanel", () => {
  let root: Root | null = null;

  beforeEach(() => {
    getCronRunBodyMock.mockReset();
    showCronMock.mockReset();
  });

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("shows placeholder when no day is selected", () => {
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey={null}
          entries={null}
          onClose={() => {}}
        />,
      );
    });
    expect(container.textContent).toContain("选中日历中的一天");
  });

  it("lists the day's entries with kind labels", () => {
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[PAST_ENTRY, FUTURE_ENTRY, MISSED_ENTRY]}
          onClose={() => {}}
        />,
      );
    });
    expect(container.textContent).toContain("weekly-report");
    expect(container.textContent).toContain("daily-standup");
    expect(container.textContent).toContain("已执行");
    expect(container.textContent).toContain("未来");
    expect(container.textContent).toContain("未执行");
  });

  it("clicking a past entry fetches and renders the thread body", async () => {
    // The cron engine writes:
    //   [L000001][P000000][@system][20260511T090000Z] cron(weekly-report): hi
    getCronRunBodyMock.mockResolvedValue({
      ok: true,
      data: {
        body: "[L000001][P000000][@system][20260511T090000Z] cron(weekly-report): hi\n",
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[PAST_ENTRY]}
          onClose={() => {}}
        />,
      );
    });

    const button = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("weekly-report"),
    );
    expect(button).toBeDefined();
    await act(async () => {
      button?.click();
      await flushPromises();
    });

    expect(getCronRunBodyMock).toHaveBeenCalledWith(
      "phone",
      "weekly-report",
      "2026-05-11T09-00-00Z",
      expect.any(AbortSignal),
    );
    expect(container.textContent).toContain("cron(weekly-report)");
    expect(container.textContent).toContain("@system");
  });

  it("clicking a future entry opens the spec detail without a missed badge", async () => {
    showCronMock.mockResolvedValue({
      ok: true,
      data: {
        name: "weekly-report",
        spec: {
          version: 1,
          schedule: "0 9 * * 1",
          target: "alice",
          prompt: "summarize the week",
          enabled: true,
          created_by: "alice",
          created_at: "2026-05-01T00:00:00Z",
        },
        recent_runs: [],
        next_fire: "2026-05-18T09:00:00Z",
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={[FUTURE_ENTRY]}
          onClose={() => {}}
        />,
      );
    });

    const button = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("weekly-report"),
    );
    await act(async () => {
      button?.click();
      await flushPromises();
    });

    expect(showCronMock).toHaveBeenCalledWith(
      "phone",
      "weekly-report",
      expect.any(AbortSignal),
    );
    expect(container.textContent).toContain("summarize the week");
    expect(container.textContent).toContain("0 9 * * 1");
    expect(container.textContent).toContain("预计 fire 时刻");
    expect(container.textContent).not.toContain("missed at");
  });

  it("clicking a missed entry opens the spec detail with a missed badge", async () => {
    showCronMock.mockResolvedValue({
      ok: true,
      data: {
        name: "daily-standup",
        spec: {
          version: 1,
          schedule: "30 9 * * *",
          target: "bob",
          prompt: "standup ping",
          enabled: true,
          created_by: "bob",
          created_at: "2026-05-01T00:00:00Z",
        },
        recent_runs: [],
        next_fire: "2026-05-13T09:30:00Z",
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-12"
          entries={[MISSED_ENTRY]}
          onClose={() => {}}
        />,
      );
    });

    const button = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("daily-standup"),
    );
    await act(async () => {
      button?.click();
      await flushPromises();
    });

    expect(showCronMock).toHaveBeenCalledWith(
      "phone",
      "daily-standup",
      expect.any(AbortSignal),
    );
    expect(container.textContent).toContain("missed at 2026-05-12T09:30:00Z");
    expect(container.textContent).toContain("runtime 当时未运行");
    expect(container.textContent).toContain("standup ping");
  });

  it("rapidly switching between past entries aborts the in-flight fetch", async () => {
    // First click resolves slow; second click triggers abort on the first.
    // We assert the first invocation's AbortSignal ends up aborted.
    let firstSignal: AbortSignal | null = null;
    const SECOND_ENTRY: CronTimelineEntry = {
      ts: "2026-05-11T10:00:00Z",
      kind: "past",
      cron_name: "later-job",
      target: "alice",
    };
    getCronRunBodyMock.mockImplementation(
      (_slug: string, _name: string, _ts: string, signal?: AbortSignal) => {
        if (firstSignal === null) firstSignal = signal ?? null;
        return new Promise(() => {
          // never resolves — we want the abort path to do the work
        });
      },
    );

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[PAST_ENTRY, SECOND_ENTRY]}
          onClose={() => {}}
        />,
      );
    });

    // Click first entry.
    const first = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("weekly-report"),
    );
    await act(async () => {
      first?.click();
      await flushPromises();
    });
    expect(firstSignal).not.toBeNull();
    expect(firstSignal!.aborted).toBe(false);

    // Back to list, then click the second.
    const backBtn = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Back to day"]',
    );
    await act(async () => {
      backBtn?.click();
      await flushPromises();
    });

    // Back-to-list unmounts CronRunViewer → effect cleanup aborts the
    // pending fetch. That's the behavior we're locking in.
    expect(firstSignal!.aborted).toBe(true);
  });

  it("renders a sane fallback when the runtime emits an unknown kind", () => {
    // Future-proofing against a `kind: "failed"` (or similar) arriving
    // from a newer runtime than the WebUI. The badge should render the
    // missed style (the `kindStyle` default) instead of crashing on a
    // `KIND_STYLES[undefined].chip` access.
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[
            {
              // Force-cast — we're simulating wire data that exceeds the
              // declared union.
              ts: "2026-05-11T09:00:00Z",
              kind: "failed" as unknown as CronTimelineEntry["kind"],
              cron_name: "weekly-report",
              target: "alice",
            },
          ]}
          onClose={() => {}}
        />,
      );
    });
    // Falls back to "未执行" (missed style). No crash, the badge renders.
    expect(container.textContent).toContain("未执行");
    expect(container.textContent).toContain("weekly-report");
  });

  it("back button returns from detail view to the list", async () => {
    getCronRunBodyMock.mockResolvedValue({
      ok: true,
      data: { body: "" },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[PAST_ENTRY]}
          onClose={() => {}}
        />,
      );
    });

    const openButton = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("weekly-report"),
    );
    await act(async () => {
      openButton?.click();
      await flushPromises();
    });

    const backButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Back to day"]',
    );
    expect(backButton).not.toBeNull();
    await act(async () => {
      backButton?.click();
      await flushPromises();
    });

    // We're back on the list — the entry button reappears.
    const listEntry = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("weekly-report"),
    );
    expect(listEntry).toBeDefined();
    expect(container.textContent).toContain("已执行");
  });

  it("resets detail view when navigating away from a day and back", async () => {
    getCronRunBodyMock.mockResolvedValue({
      ok: true,
      data: {
        body: "[L000001][P000000][@system][20260511T090000Z] cron(weekly-report): hi\n",
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[PAST_ENTRY]}
          onClose={() => {}}
        />,
      );
    });

    const openButton = Array.from(container.querySelectorAll("button")).find(
      (b) => b.textContent?.includes("weekly-report"),
    );
    await act(async () => {
      openButton?.click();
      await flushPromises();
    });
    expect(container.querySelector('button[aria-label="Back to day"]')).not.toBeNull();

    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={[FUTURE_ENTRY]}
          onClose={() => {}}
        />,
      );
      await flushPromises();
    });
    expect(container.querySelector('button[aria-label="Back to day"]')).toBeNull();

    await act(async () => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={[PAST_ENTRY]}
          onClose={() => {}}
        />,
      );
      await flushPromises();
    });

    expect(container.querySelector('button[aria-label="Back to day"]')).toBeNull();
    expect(container.textContent).toContain("weekly-report");
    expect(container.textContent).toContain("已执行");
    expect(getCronRunBodyMock).toHaveBeenCalledTimes(1);
  });

  // -- Hour grouping (threshold = 12) --
  //
  // ≤12 entries: flat list (current behavior). >12: collapsed-by-default
  // hour groups so a high-frequency cron (e.g. */30 ≡ 48/day) doesn't
  // dump a wall of rows at the user.

  function makeBulkEntries(count: number, hour: number): CronTimelineEntry[] {
    // Spread `count` entries inside a single hour, two minutes apart, so
    // they all bucket to the same hour group regardless of `count`.
    return Array.from({ length: count }, (_, i) => ({
      ts: `2026-05-18T${String(hour).padStart(2, "0")}:${String(i * 2).padStart(2, "0")}:00Z`,
      kind: "future" as const,
      cron_name: "bulk-job",
      target: "alice",
    }));
  }

  it("renders flat list when entry count is at or below the threshold (12)", () => {
    const entries = makeBulkEntries(12, 9);
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    // No hour-group header should appear at threshold-edge.
    expect(container.querySelector('[data-testid="hour-group"]')).toBeNull();
    // All 12 entry buttons exist directly under the list.
    const entryButtons = container.querySelectorAll('[data-testid="entry-row"]');
    expect(entryButtons.length).toBe(12);
  });

  it("groups entries by hour when count exceeds the threshold", () => {
    // 8 in hour 09, 5 in hour 10 = 13 total (> 12), spans two hour buckets.
    const entries = [
      ...makeBulkEntries(8, 9),
      ...makeBulkEntries(5, 10),
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const headers = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[data-testid="hour-group-header"]'),
    );
    expect(headers).toHaveLength(2);
    expect(headers[0].textContent).toContain("09:00Z");
    expect(headers[0].textContent).toContain("8");
    expect(headers[1].textContent).toContain("10:00Z");
    expect(headers[1].textContent).toContain("5");
    // Default state is collapsed: aria-expanded="false" on every header
    // and no entry row visible.
    for (const h of headers) {
      expect(h.getAttribute("aria-expanded")).toBe("false");
    }
    expect(container.querySelector('[data-testid="entry-row"]')).toBeNull();
  });

  it("clicking an hour-group header expands and re-clicking collapses", () => {
    const entries = [
      ...makeBulkEntries(7, 9),
      ...makeBulkEntries(7, 10),
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const firstHeader = container.querySelector<HTMLButtonElement>(
      '[data-testid="hour-group-header"]',
    )!;
    expect(firstHeader.getAttribute("aria-expanded")).toBe("false");

    act(() => {
      firstHeader.click();
    });
    expect(firstHeader.getAttribute("aria-expanded")).toBe("true");
    expect(
      container.querySelectorAll('[data-testid="entry-row"]').length,
    ).toBe(7);

    act(() => {
      firstHeader.click();
    });
    expect(firstHeader.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector('[data-testid="entry-row"]')).toBeNull();
  });

  it("re-opens the panel with all hour groups collapsed (state reset)", () => {
    const entries = [
      ...makeBulkEntries(7, 9),
      ...makeBulkEntries(7, 10),
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const firstHeader = container.querySelector<HTMLButtonElement>(
      '[data-testid="hour-group-header"]',
    )!;
    act(() => {
      firstHeader.click();
    });
    expect(firstHeader.getAttribute("aria-expanded")).toBe("true");

    // Switch to a different day, then back.
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-19"
          entries={[]}
          onClose={() => {}}
        />,
      );
    });
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const headerAfterReopen = container.querySelector<HTMLButtonElement>(
      '[data-testid="hour-group-header"]',
    )!;
    expect(headerAfterReopen.getAttribute("aria-expanded")).toBe("false");
  });

  it("hour-group header reports kind breakdown in aria-label and dot count", () => {
    // Hour 09: 4 past + 2 future + 1 missed (all kinds present).
    const entries: CronTimelineEntry[] = [
      ...Array.from({ length: 4 }, (_, i) => ({
        ts: `2026-05-18T09:${String(i).padStart(2, "0")}:00Z`,
        kind: "past" as const,
        cron_name: "p",
        target: "alice",
      })),
      ...Array.from({ length: 2 }, (_, i) => ({
        ts: `2026-05-18T09:${String(10 + i).padStart(2, "0")}:00Z`,
        kind: "future" as const,
        cron_name: "f",
        target: "alice",
      })),
      {
        ts: "2026-05-18T09:20:00Z",
        kind: "missed",
        cron_name: "m",
        target: "alice",
      },
      // Pad with 6 future entries in hour 10 so total = 13 > threshold.
      ...makeBulkEntries(6, 10),
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const headers = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[data-testid="hour-group-header"]'),
    );
    const hour09 = headers.find((h) => h.textContent?.includes("09:00Z"))!;
    const label = hour09.getAttribute("aria-label") ?? "";
    expect(label).toContain("09:00Z");
    expect(label).toContain("4 已执行");
    expect(label).toContain("2 未来");
    expect(label).toContain("1 未执行");
    // Three kind dots visible (one per non-zero kind), each marked aria-hidden.
    const dots = hour09.querySelectorAll('[data-testid="kind-dot"]');
    expect(dots.length).toBe(3);
  });

  it("flat list shows @target handler inline on each entry row", () => {
    const entries: CronTimelineEntry[] = [
      {
        ts: "2026-05-11T09:00:00Z",
        kind: "past",
        cron_name: "shared-cron",
        target: "alice",
      },
      {
        ts: "2026-05-11T10:00:00Z",
        kind: "past",
        cron_name: "shared-cron",
        target: "bob",
      },
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-11"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    // Both handlers appear, prefixed with "@".
    expect(container.textContent).toContain("@alice");
    expect(container.textContent).toContain("@bob");
    // Each EntryRow contains its own @target string.
    const rows = Array.from(
      container.querySelectorAll<HTMLElement>('[data-testid="entry-row"]'),
    );
    expect(rows.find((r) => r.textContent?.includes("@alice"))).toBeTruthy();
    expect(rows.find((r) => r.textContent?.includes("@bob"))).toBeTruthy();
  });

  it("grouped mode shows @target handler inside each expanded entry row", () => {
    // 13 entries → grouped mode. Mix two handlers in the same hour so
    // an expanded group reveals both.
    const entries: CronTimelineEntry[] = [
      ...Array.from({ length: 7 }, (_, i) => ({
        ts: `2026-05-18T09:${String(i * 2).padStart(2, "0")}:00Z`,
        kind: "future" as const,
        cron_name: "shared",
        target: i % 2 === 0 ? "alice" : "bob",
      })),
      ...Array.from({ length: 6 }, (_, i) => ({
        ts: `2026-05-18T10:${String(i * 2).padStart(2, "0")}:00Z`,
        kind: "future" as const,
        cron_name: "shared",
        target: "carol",
      })),
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const headers = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[data-testid="hour-group-header"]'),
    );
    const hour09 = headers.find((h) => h.textContent?.includes("09:00Z"))!;
    act(() => {
      hour09.click();
    });
    const rows = Array.from(
      container.querySelectorAll<HTMLElement>('[data-testid="entry-row"]'),
    );
    expect(rows.find((r) => r.textContent?.includes("@alice"))).toBeTruthy();
    expect(rows.find((r) => r.textContent?.includes("@bob"))).toBeTruthy();
    // Carol's entries are in hour 10 (still collapsed), so no @carol yet.
    expect(rows.find((r) => r.textContent?.includes("@carol"))).toBeUndefined();
  });

  it("hour-group with only one kind shows a single dot", () => {
    // 13 past-only entries split across two hours (single kind in each).
    const entries: CronTimelineEntry[] = [
      ...makeBulkEntries(7, 9).map((e) => ({ ...e, kind: "past" as const })),
      ...makeBulkEntries(6, 10).map((e) => ({ ...e, kind: "past" as const })),
    ];
    const { container, root: r } = makeRoot();
    root = r;
    act(() => {
      r.render(
        <CronDayPanel
          slug="phone"
          dayKey="2026-05-18"
          entries={entries}
          onClose={() => {}}
        />,
      );
    });
    const firstHeader = container.querySelector<HTMLButtonElement>(
      '[data-testid="hour-group-header"]',
    )!;
    const dots = firstHeader.querySelectorAll('[data-testid="kind-dot"]');
    expect(dots.length).toBe(1);
  });
});
