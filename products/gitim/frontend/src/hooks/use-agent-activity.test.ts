// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { useAgentStore } from "./use-agent-store";
import {
  applyUsageActivityEvent,
  applyQuickSessionActivityEvent,
} from "./use-agent-activity";
import type { Agent } from "../lib/types";
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

  it("ignores quick_session-scoped usage events to avoid polluting main agent buffer", () => {
    applyUsageActivityEvent({
      agent_id: "pc_op1",
      event_type: "usage",
      scope: "quick_session",
      detail: JSON.stringify({
        session_id: "qs-sid",
        input_tokens: 1000,
        output_tokens: 100,
      }),
      timestamp: "2026-05-11T10:32:00Z",
    });

    // Main agent usage should be unchanged (not overwritten by quick session event)
    const usage = useAgentStore.getState().agents[0]?.sessionUsage;
    expect(usage?.sessionId).toBe("sid-before-reset");
    expect(usage?.usedPercent).toBe(100);
  });

  it("ignores quick_session events even with empty detail (reset pattern)", () => {
    applyUsageActivityEvent({
      agent_id: "pc_op1",
      event_type: "usage",
      scope: "quick_session",
      detail: "",
      timestamp: "2026-05-11T10:32:00Z",
    });

    // Reset should NOT happen — quick session events are isolated
    expect(useAgentStore.getState().agents[0]?.sessionUsage).toBeDefined();
    expect(useAgentStore.getState().agents[0]?.sessionUsage!.sessionId).toBe(
      "sid-before-reset",
    );
  });
});

// ── quick session activity scope isolation ─────────────────────────────────

describe("applyQuickSessionActivityEvent", () => {
  beforeEach(() => {
    useQuickSessionStore.setState({
      sessions: [
        {
          item: {
            id: "qs-test",
            title: "Test Session",
            agent_id: "agent-1",
            status: "active",
            updated_at: "2026-07-01T00:00:00Z",
            ref_: "ref",
          },
          detailLoading: false,
        },
      ],
      loading: false,
      selectedId: null,
    });
  });

  it("ignores non-quick_session events", () => {
    applyQuickSessionActivityEvent({
      agent_id: "agent-1",
      event_type: "thinking",
      scope: undefined,
      detail: "",
      timestamp: "2026-07-01T00:00:00Z",
    });

    // Status should remain unchanged
    expect(
      useQuickSessionStore.getState().sessions[0]!.item.status,
    ).toBe("active");
  });

  it("updates status to running for thinking events", () => {
    applyQuickSessionActivityEvent({
      agent_id: "agent-1",
      event_type: "thinking",
      scope: "quick_session",
      session_id: "qs-test",
      detail: "",
      timestamp: "2026-07-01T00:00:00Z",
    });

    expect(
      useQuickSessionStore.getState().sessions[0]!.item.status,
    ).toBe("running");
  });

  it("updates status to error for error events", () => {
    applyQuickSessionActivityEvent({
      agent_id: "agent-1",
      event_type: "error",
      scope: "quick_session",
      session_id: "qs-test",
      detail: "something broke",
      timestamp: "2026-07-01T00:00:00Z",
    });

    expect(
      useQuickSessionStore.getState().sessions[0]!.item.status,
    ).toBe("error");
  });

  it("updates status to active for done events", () => {
    // First set to running
    useQuickSessionStore.getState().updateStatus("qs-test", "running");

    applyQuickSessionActivityEvent({
      agent_id: "agent-1",
      event_type: "done",
      scope: "quick_session",
      session_id: "qs-test",
      detail: "",
      timestamp: "2026-07-01T00:00:00Z",
    });

    expect(
      useQuickSessionStore.getState().sessions[0]!.item.status,
    ).toBe("active");
  });

  it("ignores events without session_id", () => {
    applyQuickSessionActivityEvent({
      agent_id: "agent-1",
      event_type: "done",
      scope: "quick_session",
      detail: "",
      timestamp: "2026-07-01T00:00:00Z",
    });

    // No session_id — status should be unchanged
    expect(
      useQuickSessionStore.getState().sessions[0]!.item.status,
    ).toBe("active");
  });
});
