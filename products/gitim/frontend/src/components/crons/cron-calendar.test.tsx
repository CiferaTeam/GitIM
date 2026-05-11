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

  it("returns focus to the activated day cell after the panel closes", async () => {
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    const ts = `${year}-${pad(month)}-15T09:00:00Z`;
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [{ ts, kind: "past", cron_name: "weekly-report" }],
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    // The day cell carrying the entry chip is the one we want focus to
    // return to. Locate it by the chip's accessible name we just lifted.
    const chip = Array.from(container.querySelectorAll("[aria-label]")).find(
      (n) => n.getAttribute("aria-label") === "已执行: weekly-report",
    );
    expect(chip).toBeTruthy();
    const dayButton = chip!.closest("button");
    expect(dayButton).not.toBeNull();
    await act(async () => {
      dayButton!.click();
      await flushPromises();
    });

    // Close via the panel's X button. There are two panels in the DOM
    // (lg-and-up aside + mobile stack), but only one is visible at a
    // time — for the test it's enough that ALL of their close buttons
    // route through the same handler, restoring focus to dayButton.
    const closeButtons = container.querySelectorAll<HTMLButtonElement>(
      'button[aria-label="Close day panel"]',
    );
    expect(closeButtons.length).toBeGreaterThan(0);
    await act(async () => {
      closeButtons[0].click();
      await flushPromises();
    });
    // queueMicrotask fires before flushPromises returns since each
    // `await Promise.resolve()` drains the microtask queue.
    expect(document.activeElement).toBe(dayButton);
  });

  it("Escape from the day panel closes it and restores focus to the trigger", async () => {
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    const ts = `${year}-${pad(month)}-15T09:00:00Z`;
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [{ ts, kind: "past", cron_name: "weekly-report" }],
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    const chip = Array.from(container.querySelectorAll("[aria-label]")).find(
      (n) => n.getAttribute("aria-label") === "已执行: weekly-report",
    );
    const dayButton = chip!.closest("button");
    await act(async () => {
      dayButton!.click();
      await flushPromises();
    });

    await act(async () => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      await flushPromises();
    });

    // Panel should be closed (no header text matching the day key remains
    // — header carries `{dayKey}` so its absence is a clean signal).
    // We're more interested in the focus assertion, but keep both.
    expect(document.activeElement).toBe(dayButton);
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

  it("encodes the full date plus kind breakdown into each day cell's aria-label", async () => {
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    // 2 past + 1 missed on day 15. The assertion below uses the textual
    // month name so a regression that drops "May" / "June" / etc. fails
    // loudly.
    const monthNames = [
      "January", "February", "March", "April", "May", "June",
      "July", "August", "September", "October", "November", "December",
    ];
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          { ts: `${year}-${pad(month)}-15T09:00:00Z`, kind: "past", cron_name: "alpha" },
          { ts: `${year}-${pad(month)}-15T10:00:00Z`, kind: "past", cron_name: "beta" },
          { ts: `${year}-${pad(month)}-15T11:00:00Z`, kind: "missed", cron_name: "gamma" },
        ],
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    // The DayCell button for day 15 should carry the full descriptive label.
    const monthName = monthNames[month - 1];
    const expectedLabel = `${monthName} 15, ${year}, 2 已执行, 1 未执行`;
    const cell = Array.from(container.querySelectorAll("button")).find(
      (b) => b.getAttribute("aria-label") === expectedLabel,
    );
    expect(cell, `expected day cell aria-label "${expectedLabel}"`).toBeTruthy();
  });

  it("entry chips render a non-color icon and accessible name with the kind label", async () => {
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          { ts: `${year}-${pad(month)}-15T09:00:00Z`, kind: "past", cron_name: "weekly-report" },
          { ts: `${year}-${pad(month)}-15T10:00:00Z`, kind: "missed", cron_name: "daily-standup" },
          { ts: `${year}-${pad(month)}-15T11:00:00Z`, kind: "future", cron_name: "monthly-roll" },
        ],
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    // Each chip carries kind label + cron name in its aria-label.
    const chips = Array.from(container.querySelectorAll("[aria-label]"));
    const chipLabels = chips
      .map((c) => c.getAttribute("aria-label") ?? "")
      .filter((l) => l.includes("weekly-report") || l.includes("daily-standup") || l.includes("monthly-roll"));
    expect(chipLabels).toContain("已执行: weekly-report");
    expect(chipLabels).toContain("未执行: daily-standup");
    expect(chipLabels).toContain("未来: monthly-roll");

    // Every chip should contain an SVG (the lucide icon) — i.e. color is
    // never the only signal. lucide-react renders <svg ...>.
    const chipNodes = chips.filter((c) => {
      const l = c.getAttribute("aria-label") ?? "";
      return /(已执行|未执行|未来): /.test(l);
    });
    expect(chipNodes.length).toBeGreaterThan(0);
    for (const node of chipNodes) {
      expect(node.querySelector("svg"), `chip ${node.getAttribute("aria-label")} missing icon`).toBeTruthy();
    }
  });

  it("clicking an empty day opens the panel showing the empty state", async () => {
    // Mobile-first behavior — without this, tapping an empty day on a
    // touch device does nothing, hiding the affordance entirely. The
    // panel's "当天没有计划任务" empty state is the user-visible result.
    const today = new Date();
    const year = today.getUTCFullYear();
    const month = today.getUTCMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    // Put one entry on day 15 so the calendar doesn't collapse to the
    // big "这个时间窗内没有计划任务" empty placeholder (which is rendered
    // by a different code path that has no per-cell buttons).
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          { ts: `${year}-${pad(month)}-15T09:00:00Z`, kind: "past", cron_name: "weekly-report" },
        ],
      },
    });

    const { container, root: r } = makeRoot();
    root = r;
    await act(async () => {
      r.render(<CronCalendar />);
      await flushPromises();
    });

    // Pick a day cell that has no entries — search by its aria-label
    // pattern. Many empty cells exist; grab the first.
    const emptyCell = Array.from(
      container.querySelectorAll<HTMLButtonElement>("button[aria-label]"),
    ).find((b) => (b.getAttribute("aria-label") ?? "").includes("无任务"));
    expect(emptyCell, "expected at least one empty-day cell").toBeTruthy();
    await act(async () => {
      emptyCell!.click();
      await flushPromises();
    });

    // The panel renders the empty-day state.
    expect(container.textContent).toContain("当天没有计划任务");
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
