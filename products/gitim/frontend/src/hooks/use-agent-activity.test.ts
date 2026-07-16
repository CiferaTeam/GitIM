// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { useAgentStore } from "./use-agent-store";
import {
  applyUsageActivityEvent,
  routeAgentActivityEvent,
} from "./use-agent-activity";
import type { Agent } from "../lib/types";
import { useAgentActivityStore } from "./use-agent-activity";
import { useQuickSessionStore } from "./use-quick-session-store";

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

  it("routes scoped usage before main usage and activity consumers", () => {
    useQuickSessionStore.getState().resetForWorkspaceSwitch();
    useQuickSessionStore.getState().applyDetail({
      meta: {
        id: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
        title: "Scoped usage",
        agent_id: "pc_op1",
        created_by: "lewis",
        status: "running",
        created_at: "2026-07-11T00:00:00Z",
        updated_at: "2026-07-11T00:00:01Z",
        last_message_preview: "working",
        processing_input_line: 1,
        processing_started_at: "2026-07-11T00:00:01Z",
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY",
        revision: 3,
      },
      entries: [],
      archived: false,
    });
    useAgentActivityStore.getState().clear();

    routeAgentActivityEvent({
      agent_id: "pc_op1",
      event_type: "usage",
      detail: JSON.stringify({
        session_id: "provider-quick",
        used_percent: 31,
        source: "runtime_estimated",
        updated_at: "2026-07-11T00:00:02Z",
      }),
      timestamp: "2026-07-11T00:00:02Z",
      scope: "quick_session",
      session_id: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
      session_revision: 3,
      attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY",
      context_generation: 1,
    });

    expect(useAgentStore.getState().agents[0]?.sessionUsage?.sessionId).toBe(
      "sid-before-reset",
    );
    expect(useAgentActivityStore.getState().activities).toEqual({});
    expect(
      useQuickSessionStore.getState().runtimeById[
        "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ"
      ]?.usage?.usedPercent,
    ).toBe(31);
  });
});
