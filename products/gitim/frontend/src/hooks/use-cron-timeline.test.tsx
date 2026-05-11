// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const getCronTimelineMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/client", () => ({
  getCronTimeline: getCronTimelineMock,
}));

import { useCronTimeline } from "./use-cron-timeline";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

async function flushPromises(times = 2) {
  for (let i = 0; i < times; i++) {
    await Promise.resolve();
  }
}

function Probe({
  slug,
  from,
  to,
  onValue,
}: {
  slug: string | null;
  from?: string;
  to?: string;
  onValue: (value: ReturnType<typeof useCronTimeline>) => void;
}) {
  const value = useCronTimeline(slug, from, to);
  onValue(value);
  return <span>{value.loading ? "loading" : "ready"}</span>;
}

describe("useCronTimeline", () => {
  let root: Root | null = null;

  beforeEach(() => {
    getCronTimelineMock.mockReset();
  });

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("returns an empty entry list and skips fetching when slug is null", async () => {
    const captured: ReturnType<typeof useCronTimeline>[] = [];
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root!.render(
        <Probe slug={null} onValue={(v) => captured.push(v)} />,
      );
      await flushPromises();
    });

    expect(getCronTimelineMock).not.toHaveBeenCalled();
    const last = captured.at(-1)!;
    expect(last.entries).toEqual([]);
    expect(last.loading).toBe(false);
    expect(last.error).toBeNull();
  });

  it("fetches entries on mount and exposes truncated flag", async () => {
    getCronTimelineMock.mockResolvedValue({
      ok: true,
      data: {
        entries: [
          { ts: "2026-05-11T09:00:00Z", kind: "past", cron_name: "alpha" },
        ],
        truncated: true,
      },
    });
    const captured: ReturnType<typeof useCronTimeline>[] = [];
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root!.render(
        <Probe
          slug="phone"
          from="2026-05-01T00:00:00.000Z"
          to="2026-05-31T23:59:59.000Z"
          onValue={(v) => captured.push(v)}
        />,
      );
      await flushPromises();
    });

    expect(getCronTimelineMock).toHaveBeenCalledTimes(1);
    expect(getCronTimelineMock).toHaveBeenCalledWith(
      "phone",
      "2026-05-01T00:00:00.000Z",
      "2026-05-31T23:59:59.000Z",
      expect.any(AbortSignal),
    );
    const last = captured.at(-1)!;
    expect(last.entries.map((e) => e.cron_name)).toEqual(["alpha"]);
    expect(last.truncated).toBe(true);
    expect(last.error).toBeNull();
  });

  it("surfaces the error message from a failed fetch", async () => {
    getCronTimelineMock.mockResolvedValue({
      ok: false,
      error: "boom",
    });
    const captured: ReturnType<typeof useCronTimeline>[] = [];
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root!.render(
        <Probe
          slug="phone"
          from="2026-05-01T00:00:00.000Z"
          to="2026-05-31T23:59:59.000Z"
          onValue={(v) => captured.push(v)}
        />,
      );
      await flushPromises();
    });

    expect(captured.at(-1)!.error).toBe("boom");
    expect(captured.at(-1)!.entries).toEqual([]);
  });
});
