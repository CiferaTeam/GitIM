import { describe, expect, it } from "vitest";
import {
  formatOutsideSummary,
  partitionAgentsRoster,
} from "./agents-roster";

describe("partitionAgentsRoster", () => {
  const base = {
    userInfos: [
      { handler: "alice", display_name: "Alice", kind: "human" },
      { handler: "bob", display_name: "Bob", kind: "human" },
      { handler: "cfo", kind: "agent" },
      { handler: "ghost" }, // unknown
      { handler: "local-bot", kind: "agent" },
    ],
    localAgents: [{ id: "local-bot", handler: "local-bot" }],
    fleetSnapshots: [{ agent: { id: "cfo", handler: "cfo" } }],
    query: "",
    statusFilter: null,
  };

  it("puts humans on top list and excludes live agents from outside", () => {
    const r = partitionAgentsRoster(base);
    expect(r.humans.map((h) => h.handler)).toEqual(["alice", "bob"]);
    expect(r.outside.map((o) => o.handler)).toEqual(["ghost"]);
    expect(r.outsideUnknownCount).toBe(1);
    expect(r.outsideAgentCount).toBe(0);
    expect(r.showHumansAndOutside).toBe(true);
  });

  it("counts non-live agents in outside when not on local/fleet", () => {
    const r = partitionAgentsRoster({
      ...base,
      fleetSnapshots: [],
    });
    expect(r.outside.map((o) => o.handler).sort()).toEqual(["cfo", "ghost"]);
    expect(r.outsideAgentCount).toBe(1);
    expect(r.outsideUnknownCount).toBe(1);
  });

  it("hides humans/outside when status filter active", () => {
    const r = partitionAgentsRoster({ ...base, statusFilter: "online" });
    expect(r.showHumansAndOutside).toBe(false);
    expect(r.humans).toEqual([]);
    expect(r.outside).toEqual([]);
  });

  it("filters humans/outside by query", () => {
    const r = partitionAgentsRoster({ ...base, query: "ali", fleetSnapshots: [] });
    expect(r.humans.map((h) => h.handler)).toEqual(["alice"]);
    expect(r.outside).toEqual([]);
  });

  it("treats the signed-in user as human when kind is missing", () => {
    const r = partitionAgentsRoster({
      ...base,
      userInfos: [{ handler: "flame4", display_name: "lewis" }],
      localAgents: [],
      fleetSnapshots: [],
      currentUser: "flame4",
    });
    expect(r.humans).toEqual([
      { handler: "flame4", displayName: "lewis", kind: "human" },
    ]);
    expect(r.outside).toEqual([]);
    expect(r.outsideUnknownCount).toBe(0);
  });
});

describe("formatOutsideSummary", () => {
  it("omits zero segments and returns null when empty", () => {
    expect(formatOutsideSummary(0, 0)).toBeNull();
    expect(formatOutsideSummary(1, 0)).toBe("Outside · 1 agent");
    expect(formatOutsideSummary(3, 0)).toBe("Outside · 3 agents");
    expect(formatOutsideSummary(0, 2)).toBe("Outside · 2 unclassified");
    expect(formatOutsideSummary(3, 2)).toBe("Outside · 3 agents · 2 unclassified");
  });
});
