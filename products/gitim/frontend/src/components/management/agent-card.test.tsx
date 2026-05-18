import { describe, expect, it } from "vitest";
import type { Agent } from "@/lib/types";
import { agentModelLabel } from "./agent-card";

function agent(provider: Agent["provider"], model?: string): Agent {
  return {
    id: `${provider}-agent`,
    name: `${provider}-agent`,
    status: "running",
    provider,
    model,
    systemPrompt: "",
    repoPath: `/tmp/${provider}-agent`,
    messagesProcessed: 0,
  };
}

describe("agentModelLabel", () => {
  it("renders Kimi default model mode instead of an empty dash", () => {
    expect(agentModelLabel(agent("kimi"))).toBe("default");
  });

  it("renders explicit Kimi models verbatim", () => {
    expect(agentModelLabel(agent("kimi", "kimi-code/kimi-for-coding"))).toBe(
      "kimi-code/kimi-for-coding",
    );
  });
});
