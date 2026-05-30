// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { useAgentStore } from "./use-agent-store";
import { applyUsageActivityEvent } from "./use-agent-activity";
import type { Agent } from "../lib/types";

function agentWithUsage(): Agent {
  return {
    id: "pc_op1",
    handler: "pc_op1",
    name: "pc_op1",
    status: "running",
    systemPrompt: "",
    repoPath: "/tmp/pc_op1",
    messagesProcessed: 0,
    sessionUsage: {
      sessionId: "sid-before-reset",
      inputTokens: 190_000,
      outputTokens: 2_000,
      maxTokens: 200_000,
      usedPercent: 100,
      source: "provider_reported",
      updatedAt: "2026-05-11T10:31:00Z",
    },
  };
}

describe("applyUsageActivityEvent", () => {
  beforeEach(() => {
    useAgentStore.setState({
      agents: [agentWithUsage()],
      selectedAgentId: null,
    });
  });

  it("clears cached session usage when runtime broadcasts reset", () => {
    applyUsageActivityEvent({
      agent_id: "pc_op1",
      event_type: "usage",
      detail: "",
      timestamp: "2026-05-11T10:31:14Z",
    });

    expect(useAgentStore.getState().agents[0]?.sessionUsage).toBeUndefined();
  });

  it("updates session usage from a normal usage payload", () => {
    applyUsageActivityEvent({
      agent_id: "pc_op1",
      event_type: "usage",
      detail: JSON.stringify({
        session_id: "sid-after-reset",
        input_tokens: 12_000,
        output_tokens: 300,
        max_tokens: 200_000,
        used_percent: 6,
        source: "runtime_estimated",
        updated_at: "2026-05-11T10:32:00Z",
      }),
      timestamp: "2026-05-11T10:32:00Z",
    });

    const usage = useAgentStore.getState().agents[0]?.sessionUsage;
    expect(usage?.sessionId).toBe("sid-after-reset");
    expect(usage?.usedPercent).toBe(6);
    expect(usage?.source).toBe("runtime_estimated");
  });
});
