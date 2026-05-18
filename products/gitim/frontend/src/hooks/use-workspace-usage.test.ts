import { describe, expect, it } from "vitest";
import type { Agent, UsageBucket, UsageDayEntry, UsageSummary } from "../lib/types";
import { aggregateWorkspaceUsage } from "./use-workspace-usage";

function bucket(input: number, output: number, cr = 0, cc = 0, turns = 0): UsageBucket {
  return { input, output, cacheRead: cr, cacheCreation: cc, turns };
}

function summary(
  totals: UsageBucket,
  today: UsageBucket,
  byDay: UsageDayEntry[] = [],
  providerReportsUsage = true,
): UsageSummary {
  return {
    providerReportsUsage,
    firstSeen: "2026-05-01T00:00:00Z",
    lastUpdated: "2026-05-10T12:00:00Z",
    totals,
    today,
    byDay,
  };
}

function agent(id: string, provider: string, s?: UsageSummary): Agent {
  return {
    id,
    name: id,
    status: "idle",
    provider: provider as Agent["provider"],
    systemPrompt: "",
    repoPath: "",
    messagesProcessed: 0,
    usageSummary: s,
  };
}

describe("aggregateWorkspaceUsage", () => {
  it("returns hasData=false when no agent has usage data", () => {
    const out = aggregateWorkspaceUsage([
      agent("a", "claude"),
      agent("b", "codex"),
    ]);
    expect(out.hasData).toBe(false);
    expect(out.totals.input).toBe(0);
    expect(out.byProvider).toEqual([]);
  });

  it("sums totals + today + byDay across agents", () => {
    const out = aggregateWorkspaceUsage([
      agent(
        "a",
        "claude",
        summary(bucket(100, 50, 1000, 10, 3), bucket(40, 20, 0, 0, 1), [
          { date: "2026-05-09", bucket: bucket(60, 30, 1000, 10, 2) },
          { date: "2026-05-10", bucket: bucket(40, 20, 0, 0, 1) },
        ]),
      ),
      agent(
        "b",
        "codex",
        summary(bucket(200, 80, 5000, 20, 5), bucket(50, 30, 0, 0, 2), [
          { date: "2026-05-09", bucket: bucket(150, 50, 5000, 20, 3) },
          { date: "2026-05-10", bucket: bucket(50, 30, 0, 0, 2) },
        ]),
      ),
    ]);
    expect(out.hasData).toBe(true);
    expect(out.totals).toEqual(bucket(300, 130, 6000, 30, 8));
    expect(out.today).toEqual(bucket(90, 50, 0, 0, 3));
    expect(out.byDay.find((d) => d.date === "2026-05-09")?.bucket).toEqual(
      bucket(210, 80, 6000, 30, 5),
    );
  });

  it("groups totals by provider and sorts descending", () => {
    const out = aggregateWorkspaceUsage([
      agent("a", "claude", summary(bucket(100, 0), bucket(0, 0))),
      agent("b", "claude", summary(bucket(200, 0), bucket(0, 0))),
      agent("c", "codex", summary(bucket(50, 0), bucket(0, 0))),
    ]);
    expect(out.byProvider.map((p) => p.provider)).toEqual(["claude", "codex"]);
    expect(out.byProvider[0].bucket.input).toBe(300);
    expect(out.byProvider[1].bucket.input).toBe(50);
  });

  it("groups both today and cumulative buckets by provider and handler", () => {
    const out = aggregateWorkspaceUsage([
      agent("alice", "codex", summary(bucket(300, 0, 0, 0, 9), bucket(30, 0, 0, 0, 1))),
      agent("bob", "codex", summary(bucket(100, 0, 0, 0, 4), bucket(40, 0, 0, 0, 2))),
      agent("cara", "claude", summary(bucket(20, 0, 0, 0, 1), bucket(10, 0, 0, 0, 1))),
    ]);

    expect(out.byProvider.find((p) => p.provider === "codex")?.bucket).toEqual(
      bucket(400, 0, 0, 0, 13),
    );
    expect(out.byProvider.find((p) => p.provider === "codex")?.today).toEqual(
      bucket(70, 0, 0, 0, 3),
    );
    expect(out.byHandler.find((h) => h.handler === "bob")?.bucket).toEqual(
      bucket(100, 0, 0, 0, 4),
    );
    expect(out.byHandler.find((h) => h.handler === "bob")?.today).toEqual(
      bucket(40, 0, 0, 0, 2),
    );
  });

  it("merges by_day by date string when agents have different windows", () => {
    // Agent A only has 2026-05-09, agent B only has 2026-05-10. Output
    // must contain both dates with their respective buckets.
    const out = aggregateWorkspaceUsage([
      agent(
        "a",
        "claude",
        summary(bucket(10, 5), bucket(0, 0), [
          { date: "2026-05-09", bucket: bucket(10, 5, 0, 0, 1) },
        ]),
      ),
      agent(
        "b",
        "codex",
        summary(bucket(20, 5), bucket(0, 0), [
          { date: "2026-05-10", bucket: bucket(20, 5, 0, 0, 1) },
        ]),
      ),
    ]);
    expect(out.byDay.length).toBe(2);
    expect(out.byDay[0].date).toBe("2026-05-09");
    expect(out.byDay[1].date).toBe("2026-05-10");
  });

  it("treats a missing provider as 'unknown'", () => {
    const out = aggregateWorkspaceUsage([
      {
        ...agent("a", "claude", summary(bucket(10, 5), bucket(0, 0))),
        provider: undefined,
      },
    ]);
    expect(out.byProvider[0].provider).toBe("unknown");
  });

  it("sorts byProvider by token total desc, alphabetical on ties", () => {
    const out = aggregateWorkspaceUsage([
      agent("a", "codex", summary(bucket(100, 0), bucket(0, 0))),
      agent("b", "claude", summary(bucket(100, 0), bucket(0, 0))),
      agent("c", "opencode", summary(bucket(50, 0), bucket(0, 0))),
    ]);
    expect(out.byProvider.map((p) => p.provider)).toEqual([
      "claude",
      "codex",
      "opencode",
    ]);
  });

  it("byHandler enumerates every agent including zero-usage ones", () => {
    const out = aggregateWorkspaceUsage([
      agent("alice", "codex", summary(bucket(100, 50), bucket(40, 20))),
      agent("bob", "claude"),
      agent("carol", "pi", summary(bucket(10, 5), bucket(0, 0))),
    ]);
    expect(out.byHandler.map((h) => h.handler)).toEqual([
      "alice",
      "carol",
      "bob",
    ]);
    expect(out.byHandler.find((h) => h.handler === "bob")?.bucket).toEqual({
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheCreation: 0,
      turns: 0,
    });
  });

  it("byProvider enumerates every distinct provider including zero-usage ones", () => {
    const out = aggregateWorkspaceUsage([
      agent("alice", "codex", summary(bucket(100, 50), bucket(40, 20))),
      agent("bob", "claude"),
    ]);
    expect(out.byProvider.map((p) => p.provider)).toEqual(["codex", "claude"]);
    expect(out.byProvider.find((p) => p.provider === "claude")?.bucket.input).toBe(0);
  });

  it("keeps non-token-reporting providers visible as turn-only breakdowns", () => {
    const out = aggregateWorkspaceUsage([
      agent("alice", "codex", summary(bucket(100, 50, 0, 0, 1), bucket(40, 20, 0, 0, 1))),
      agent(
        "kimi",
        "kimi",
        summary(bucket(0, 0, 0, 0, 6), bucket(0, 0, 0, 0, 6), [], false),
      ),
    ]);

    expect(out.hasData).toBe(true);
    expect(out.totals.turns).toBe(7);
    expect(out.today.turns).toBe(7);
    expect(out.byProvider.map((p) => p.provider)).toEqual(["codex", "kimi"]);
    expect(out.byProvider.find((p) => p.provider === "kimi")).toMatchObject({
      providerReportsUsage: false,
      bucket: bucket(0, 0, 0, 0, 6),
    });
    expect(out.byHandler.map((h) => h.handler)).toEqual(["alice", "kimi"]);
    expect(out.byHandler.find((h) => h.handler === "kimi")).toMatchObject({
      providerReportsUsage: false,
      bucket: bucket(0, 0, 0, 0, 6),
    });
  });

  it("returns empty byHandler when hasData=false (header stays hidden)", () => {
    const out = aggregateWorkspaceUsage([
      agent("a", "claude"),
      agent("b", "codex"),
    ]);
    expect(out.hasData).toBe(false);
    expect(out.byHandler).toEqual([]);
  });
});
