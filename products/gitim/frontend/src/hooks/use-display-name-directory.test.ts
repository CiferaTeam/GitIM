import { describe, expect, it } from "vitest";
import { buildDirectory } from "./use-display-name-directory";
import type { Agent, UserInfo } from "../lib/types";

function agent(handler: string, name: string): Agent {
  return {
    id: handler,
    handler,
    name,
    status: "running",
    systemPrompt: "",
    repoPath: "",
    messagesProcessed: 0,
  };
}

const users = (rows: Array<[string, string]>): UserInfo[] =>
  rows.map(([handler, display_name]) => ({ handler, display_name }));

describe("buildDirectory", () => {
  it("merges humans and agents into a handler→display_name map", () => {
    const dir = buildDirectory(
      [agent("cfo", "Finance Bot")],
      users([["alice", "Alice Chen"]]),
    );
    expect(dir.get("alice")).toBe("Alice Chen");
    expect(dir.get("cfo")).toBe("Finance Bot");
    expect(dir.size).toBe(2);
  });

  it("skips entries whose display_name equals the handler", () => {
    const dir = buildDirectory([agent("bob", "bob")], users([["bob", "bob"]]));
    expect(dir.has("bob")).toBe(false);
  });

  it("keys agents on agent.handler, not agent.id", () => {
    // name baked as display_name; handler distinct from a hypothetical id.
    const a = { ...agent("real-handler", "Display Name"), id: "internal-id" };
    const dir = buildDirectory([a], []);
    expect(dir.get("real-handler")).toBe("Display Name");
    expect(dir.has("internal-id")).toBe(false);
  });

  it("returns an empty map for empty sources", () => {
    expect(buildDirectory([], []).size).toBe(0);
  });
});
