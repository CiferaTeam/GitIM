// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { useAgentActivityStore } from "./use-agent-activity";
import {
  applyFleetAgentActivityEvent,
  fleetActivityKey,
  useFleetStore,
} from "./use-fleet-store";
import type { Agent, FleetAgentSnapshot } from "../lib/types";

function agent(id: string): Agent {
  return {
    id,
    handler: id,
    name: id,
    status: "running",
    systemPrompt: "",
    repoPath: `/remote/${id}`,
    messagesProcessed: 0,
  };
}

function snapshot(nodeId: string, id: string): FleetAgentSnapshot {
  return {
    nodeId,
    nodeName: nodeId === "node-a" ? "mac-mini" : undefined,
    workspaceId: "room",
    remoteWorkspaceId: "remote-room",
    workspaceIdentity: "github.com/org/repo",
    agent: agent(id),
  };
}

describe("useFleetStore", () => {
  beforeEach(() => {
    useFleetStore.getState().resetForWorkspaceSwitch();
    useAgentActivityStore.getState().clear();
    useFleetStore.getState().setAgents([
      snapshot("node-a", "cfo"),
      snapshot("node-b", "cfo"),
    ]);
  });

  it("patches usage on the matching remote node agent only", () => {
    applyFleetAgentActivityEvent({
      kind: "agent_activity",
      node_id: "node-a",
      node_name: "mac-mini",
      workspace_id: "room",
      remote_workspace_id: "remote-room",
      workspace_identity: "github.com/org/repo",
      agent_id: "cfo",
      received_at: "2026-05-15T00:10:00Z",
      event: {
        agent_id: "cfo",
        workspace_id: "room",
        event_type: "usage",
        detail: JSON.stringify({
          session_id: "sid",
          input_tokens: 120,
          output_tokens: 30,
          max_tokens: 200000,
          used_percent: 12,
          source: "provider_reported",
          updated_at: "2026-05-15T00:10:00Z",
          usage_summary: {
            provider_reports_usage: true,
            first_seen: "2026-05-15T00:00:00Z",
            last_updated: "2026-05-15T00:10:00Z",
            totals: {
              input: 120,
              output: 30,
              cache_read: 0,
              cache_creation: 0,
              turns: 1,
            },
            today: {
              input: 120,
              output: 30,
              cache_read: 0,
              cache_creation: 0,
              turns: 1,
            },
            by_day: [],
          },
        }),
        timestamp: "2026-05-15T00:10:00Z",
      },
    });

    const [nodeA, nodeB] = useFleetStore.getState().agents;
    expect(nodeA.agent.sessionUsage?.sessionId).toBe("sid");
    expect(nodeA.agent.usageSummary?.today.turns).toBe(1);
    expect(nodeB.agent.sessionUsage).toBeUndefined();
  });

  it("removes only the remote node agent named by a burned event", () => {
    applyFleetAgentActivityEvent({
      kind: "agent_activity",
      node_id: "node-a",
      workspace_id: "room",
      agent_id: "cfo",
      received_at: "2026-05-15T00:10:00Z",
      event: {
        agent_id: "cfo",
        workspace_id: "room",
        event_type: "burned",
        detail: "",
        timestamp: "2026-05-15T00:10:00Z",
      },
    });

    const agents = useFleetStore.getState().agents;
    expect(agents).toHaveLength(1);
    expect(agents[0].nodeId).toBe("node-b");
    expect(agents[0].agent.id).toBe("cfo");
  });

  it("ignores fleet-forwarded Quick Session events before main-agent routing", () => {
    useFleetStore.getState().updateAgent("node-a", "room", "cfo", {
      status: "idle",
      sessionUsage: {
        sessionId: "main-session",
        usedPercent: 8,
        source: "runtime_estimated",
        updatedAt: "2026-05-15T00:00:00Z",
      },
    });
    const key = fleetActivityKey("node-a", "room", "cfo");

    for (const event_type of ["usage", "error", "burned"] as const) {
      applyFleetAgentActivityEvent({
        kind: "agent_activity",
        node_id: "node-a",
        workspace_id: "room",
        agent_id: "cfo",
        received_at: "2026-05-15T00:10:00Z",
        event: {
          agent_id: "cfo",
          workspace_id: "room",
          event_type,
          detail:
            event_type === "usage"
              ? JSON.stringify({
                  session_id: "quick-session-provider",
                  used_percent: 91,
                  source: "provider_reported",
                  updated_at: "2026-05-15T00:10:00Z",
                })
              : "quick session event",
          timestamp: "2026-05-15T00:10:00Z",
          scope: "quick_session",
          session_id: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
          attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY",
          session_revision: 3,
          context_generation: 1,
        },
      });
    }

    const nodeA = useFleetStore
      .getState()
      .agents.find((entry) => entry.nodeId === "node-a");
    expect(nodeA?.agent.status).toBe("idle");
    expect(nodeA?.agent.sessionUsage?.sessionId).toBe("main-session");
    expect(useAgentActivityStore.getState().activities[key]).toBeUndefined();
    expect(useFleetStore.getState().agents).toHaveLength(2);

    applyFleetAgentActivityEvent({
      kind: "agent_activity",
      node_id: "node-c",
      workspace_id: "room",
      agent_id: "new-agent",
      received_at: "2026-05-15T00:10:00Z",
      event: {
        agent_id: "new-agent",
        workspace_id: "room",
        event_type: "thinking",
        detail: "quick only",
        timestamp: "2026-05-15T00:10:00Z",
        scope: "quick_session",
        session_id: "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ",
        attempt_id: "qa-01JYYYYYYYYYYYYYYYYYYYYYYY",
        session_revision: 3,
        context_generation: 1,
      },
    });
    expect(useFleetStore.getState().agents).toHaveLength(2);
  });

  it("infers a remote agent from fleet activity and stores its event by node key", () => {
    applyFleetAgentActivityEvent({
      kind: "agent_activity",
      node_id: "mac-mini",
      node_name: "lewismac-mini",
      remote_workspace_id: "room",
      workspace_identity: "github.com/flame4/room",
      workspace_id: "room",
      agent_id: "glm51op",
      received_at: "2026-05-18T11:19:35Z",
      event: {
        agent_id: "glm51op",
        workspace_id: "room",
        event_type: "done",
        detail: "done (18.5s)",
        timestamp: "2026-05-18T11:19:35Z",
      },
    });

    const inferred = useFleetStore
      .getState()
      .agents.find((entry) => entry.agent.id === "glm51op");
    expect(inferred?.nodeName).toBe("lewismac-mini");
    expect(inferred?.agent.status).toBe("running");

    const events =
      useAgentActivityStore.getState().activities[
        fleetActivityKey("mac-mini", "room", "glm51op")
      ];
    expect(events?.[0]?.detail).toBe("done (18.5s)");
  });
});
