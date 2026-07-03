// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

// Mock gets hoisted above all imports by vitest.
// Must export all symbols that use-quick-session-store imports from @/lib/client.
vi.mock("@/lib/client", () => ({
  listQuickSessions: vi.fn(),
  createQuickSession: vi.fn(),
  readQuickSession: vi.fn(),
  sendQuickSessionMessage: vi.fn(),
  setQuickSessionTitle: vi.fn(),
  archiveQuickSession: vi.fn(),
}));

import {
  parseThread,
  useQuickSessionStore,
} from "./use-quick-session-store";
import { setQuickSessionTitle } from "@/lib/client";

// ── parseThread ────────────────────────────────────────────────────────────

describe("parseThread", () => {
  it("parses a single message", () => {
    const raw =
      "[L000001][P000000][@flame4][20260702T141101Z] Hello world\n";
    const msgs = parseThread(raw);
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toMatchObject({
      line: "L000001",
      author: "flame4",
      body: "Hello world",
    });
  });

  it("parses multiple messages", () => {
    const raw = [
      "[L000001][P000000][@flame4][20260702T141101Z] First",
      "[L000002][P000000][@dev-qiangzai][20260702T141201Z] Second",
      "",
    ].join("\n");
    const msgs = parseThread(raw);
    expect(msgs).toHaveLength(2);
    expect(msgs[0]!.author).toBe("flame4");
    expect(msgs[1]!.author).toBe("dev-qiangzai");
  });

  it("handles empty input", () => {
    expect(parseThread("")).toEqual([]);
  });

  it("handles whitespace-only input", () => {
    expect(parseThread("\n\n")).toEqual([]);
  });

  it("returns empty for non-matching lines", () => {
    expect(parseThread("just some text\n")).toEqual([]);
  });

  it("rejects event lines", () => {
    const raw = [
      "[L000001][P000000][@flame4][20260702T141101Z] Hello",
      "[L000002][P000000][@system][20260702T141201Z][E:card_assigned] assigned",
      "[L000003][P000000][@dev-qiangzai][20260702T141301Z] reply",
      "",
    ].join("\n");
    const msgs = parseThread(raw);
    expect(msgs).toHaveLength(2);
    expect(msgs[0]!.author).toBe("flame4");
    expect(msgs[1]!.author).toBe("dev-qiangzai");
  });

  it("handles CJK characters", () => {
    const raw =
      "[L000001][P000000][@alice][20260702T141101Z] 你好世界\n";
    const msgs = parseThread(raw);
    expect(msgs[0]!.body).toBe("你好世界");
  });
});

// ── setTitle ───────────────────────────────────────────────────────────────

describe("useQuickSessionStore setTitle", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useQuickSessionStore.setState({
      sessions: [
        {
          item: {
            id: "qs-abc123",
            title: "Old Title",
            agent_id: "agent-1",
            status: "active",
            updated_at: "2026-07-01T00:00:00Z",
            ref_: "ref1",
          },
          detailLoading: false,
        },
        {
          item: {
            id: "qs-other",
            title: "Other",
            agent_id: "agent-2",
            status: "active",
            updated_at: "2026-07-01T00:00:00Z",
            ref_: "ref2",
          },
          detailLoading: false,
        },
      ],
      loading: false,
      selectedId: null,
    });
  });

  it("updates item.title on API success", async () => {
    vi.mocked(setQuickSessionTitle).mockResolvedValue({
      ok: true,
      data: null,
    } as never);

    const ok = await useQuickSessionStore
      .getState()
      .setTitle("ws", "qs-abc123", "New Title");

    expect(ok).toBe(true);
    expect(useQuickSessionStore.getState().sessions[0]!.item.title).toBe(
      "New Title",
    );
    expect(useQuickSessionStore.getState().sessions[1]!.item.title).toBe(
      "Other",
    );
  });

  it("updates detail.meta.title when detail is loaded", async () => {
    useQuickSessionStore.setState((s) => ({
      sessions: s.sessions.map((entry) =>
        entry.item.id === "qs-abc123"
          ? {
              ...entry,
              detail: {
                meta: {
                  id: "qs-abc123",
                  title: "Old Title",
                  title_source: "user",
                  agent_id: "agent-1",
                  created_by: "flame4",
                  status: "active" as const,
                  created_at: "2026-07-01T00:00:00Z",
                  updated_at: "2026-07-01T00:00:00Z",
                },
                thread: "",
              },
            }
          : entry,
      ),
    }));

    vi.mocked(setQuickSessionTitle).mockResolvedValue({
      ok: true,
      data: null,
    } as never);

    await useQuickSessionStore
      .getState()
      .setTitle("ws", "qs-abc123", "New Title");

    const entry = useQuickSessionStore.getState().sessions[0]!;
    expect(entry.item.title).toBe("New Title");
    expect(entry.detail?.meta.title).toBe("New Title");
  });

  it("does not update state on API failure", async () => {
    vi.mocked(setQuickSessionTitle).mockResolvedValue({
      ok: false,
      error: "boom",
    } as never);

    const ok = await useQuickSessionStore
      .getState()
      .setTitle("ws", "qs-abc123", "New Title");

    expect(ok).toBe(false);
    expect(useQuickSessionStore.getState().sessions[0]!.item.title).toBe(
      "Old Title",
    );
  });
});
