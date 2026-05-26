import { useEffect, useRef } from "react";
import { create } from "zustand";
import { mapBackendUsageSummary } from "../lib/client";
import { onWorkspaceSwitch } from "../lib/workspace-lifecycle";
import type {
  Agent,
  AgentStatus,
  FleetAgentActivityEnvelope,
  FleetAgentSnapshot,
  FleetEventEnvelope,
  FleetNodeStatus,
  FleetNodeStatusEnvelope,
} from "../lib/types";
import { useAgentActivityStore } from "./use-agent-activity";
import { useConnectionStore } from "./use-connection-store";
import { useWorkspaceStore } from "./use-workspace-store";

interface FleetState {
  agents: FleetAgentSnapshot[];
  statuses: FleetNodeStatus[];
  setAgents: (agents: FleetAgentSnapshot[]) => void;
  setStatuses: (statuses: FleetNodeStatus[]) => void;
  upsertStatus: (status: FleetNodeStatus) => void;
  upsertAgent: (snapshot: FleetAgentSnapshot) => void;
  updateAgent: (
    nodeId: string,
    workspaceId: string,
    agentId: string,
    updates: Partial<Agent>,
  ) => void;
  removeAgent: (nodeId: string, workspaceId: string, agentId: string) => void;
  resetForWorkspaceSwitch: () => void;
}

export const useFleetStore = create<FleetState>((set) => ({
  agents: [],
  statuses: [],

  setAgents: (agents) => set({ agents }),

  setStatuses: (statuses) => set({ statuses }),

  upsertStatus: (status) =>
    set((state) => {
      const key = fleetStatusKey(status);
      const next = state.statuses.filter((s) => fleetStatusKey(s) !== key);
      next.push(status);
      return { statuses: sortStatuses(next) };
    }),

  upsertAgent: (snapshot) =>
    set((state) => {
      const key = fleetAgentKey(snapshot);
      const next = state.agents.filter((s) => fleetAgentKey(s) !== key);
      next.push(snapshot);
      return { agents: sortSnapshots(next) };
    }),

  updateAgent: (nodeId, workspaceId, agentId, updates) =>
    set((state) => ({
      agents: state.agents.map((snapshot) =>
        snapshot.nodeId === nodeId &&
        snapshot.workspaceId === workspaceId &&
        snapshot.agent.id === agentId
          ? { ...snapshot, agent: { ...snapshot.agent, ...updates } }
          : snapshot,
      ),
    })),

  removeAgent: (nodeId, workspaceId, agentId) =>
    set((state) => ({
      agents: state.agents.filter(
        (snapshot) =>
          !(
            snapshot.nodeId === nodeId &&
            snapshot.workspaceId === workspaceId &&
            snapshot.agent.id === agentId
          ),
      ),
    })),

  resetForWorkspaceSwitch: () => set({ agents: [], statuses: [] }),
}));

onWorkspaceSwitch(() => {
  useFleetStore.getState().resetForWorkspaceSwitch();
});

export function applyFleetEventEnvelope(envelope: FleetEventEnvelope) {
  if (envelope.kind === "node_status") {
    useFleetStore.getState().upsertStatus(mapFleetStatusEnvelope(envelope));
    return;
  }
  applyFleetAgentActivityEvent(envelope);
}

export function applyFleetAgentActivityEvent(envelope: FleetAgentActivityEnvelope) {
  const { event } = envelope;
  const nodeId = envelope.node_id;
  const workspaceId = envelope.workspace_id;
  const agentId = envelope.agent_id;

  if (event.event_type === "burned") {
    useFleetStore.getState().removeAgent(nodeId, workspaceId, agentId);
    return;
  }

  ensureInferredAgent(envelope);

  if (event.event_type === "usage") {
    const updates = parseUsageUpdates(event.detail);
    if (updates) {
      useFleetStore.getState().updateAgent(nodeId, workspaceId, agentId, updates);
    }
    return;
  }

  useAgentActivityStore
    .getState()
    .pushForKey(fleetActivityKey(nodeId, workspaceId, agentId), event);

  useFleetStore.getState().updateAgent(nodeId, workspaceId, agentId, {
    lastActivity: event.timestamp || envelope.received_at,
    status: statusForActivityEvent(event.event_type),
    errorMessage: event.event_type === "error" ? event.detail : undefined,
  });
}

export function fleetActivityKey(
  nodeId: string,
  workspaceId: string,
  agentId: string,
) {
  return `fleet:${nodeId}\u0000${workspaceId}\u0000${agentId}`;
}

export function useFleetSSE(slug: string | null) {
  const port = useConnectionStore((s) => s.port);
  const esRef = useRef<EventSource | null>(null);

  useEffect(() => {
    if (!port || !slug) return;

    const url = `http://127.0.0.1:${port}/fleet/events`;
    const es = new EventSource(url);
    esRef.current = es;

    es.onmessage = (e) => {
      try {
        const envelope = JSON.parse(e.data) as FleetEventEnvelope;
        if ("workspace_id" in envelope && envelope.workspace_id !== slug) return;
        applyFleetEventEnvelope(envelope);
      } catch {
        // ignore malformed fleet events
      }
    };

    es.onerror = () => {
      void useWorkspaceStore.getState().refreshAfterActiveUnavailable(slug);
    };

    return () => {
      es.close();
      esRef.current = null;
    };
  }, [port, slug]);
}

function parseUsageUpdates(detail: string): Partial<Agent> | null {
  if (detail.trim() === "") {
    return { sessionUsage: undefined };
  }
  try {
    const snap = JSON.parse(detail);
    const updates: Partial<Agent> = {
      sessionUsage: {
        sessionId: snap.session_id ?? "",
        inputTokens: snap.input_tokens,
        outputTokens: snap.output_tokens,
        maxTokens: snap.max_tokens,
        usedPercent: snap.used_percent ?? 0,
        source: snap.source ?? "provider_reported",
        updatedAt: snap.updated_at ?? "",
      },
    };
    const summary = mapBackendUsageSummary(snap.usage_summary);
    if (summary) {
      updates.usageSummary = summary;
    }
    return updates;
  } catch {
    return null;
  }
}

function ensureInferredAgent(envelope: FleetAgentActivityEnvelope) {
  const state = useFleetStore.getState();
  const exists = state.agents.some(
    (snapshot) =>
      snapshot.nodeId === envelope.node_id &&
      snapshot.workspaceId === envelope.workspace_id &&
      snapshot.agent.id === envelope.agent_id,
  );
  if (exists) return;

  state.upsertAgent({
    nodeId: envelope.node_id,
    nodeIp: envelope.node_ip,
    nodeName: envelope.node_name,
    remoteWorkspaceId: envelope.remote_workspace_id,
    workspaceIdentity: envelope.workspace_identity,
    workspaceId: envelope.workspace_id,
    agent: {
      id: envelope.agent_id,
      name: envelope.agent_id,
      status: statusForActivityEvent(envelope.event.event_type),
      systemPrompt: "",
      repoPath: "",
      lastActivity: envelope.event.timestamp || envelope.received_at,
      messagesProcessed: 0,
    },
  });
}

function statusForActivityEvent(eventType: string): AgentStatus {
  if (eventType === "error") return "error";
  return "running";
}

function mapFleetStatusEnvelope(envelope: FleetNodeStatusEnvelope): FleetNodeStatus {
  return {
    nodeId: envelope.node_id,
    nodeIp: envelope.node_ip,
    nodeName: envelope.node_name,
    remoteWorkspaceId: envelope.remote_workspace_id,
    workspaceIdentity: envelope.workspace_identity,
    workspaceId: envelope.workspace_id,
    status: envelope.status,
    lastConnectedAt: envelope.last_connected_at,
    lastEventAt: envelope.last_event_at,
    lastError: envelope.last_error,
    retryCount: envelope.retry_count,
  };
}

function fleetStatusKey(status: FleetNodeStatus) {
  return `${status.nodeId}\u0000${status.workspaceId}\u0000${status.remoteWorkspaceId ?? ""}`;
}

function fleetAgentKey(snapshot: FleetAgentSnapshot) {
  return `${snapshot.nodeId}\u0000${snapshot.workspaceId}\u0000${snapshot.agent.id}`;
}

function sortSnapshots(snapshots: FleetAgentSnapshot[]) {
  return [...snapshots].sort((a, b) =>
    a.nodeId.localeCompare(b.nodeId) ||
    a.workspaceId.localeCompare(b.workspaceId) ||
    a.agent.id.localeCompare(b.agent.id),
  );
}

function sortStatuses(statuses: FleetNodeStatus[]) {
  return [...statuses].sort((a, b) =>
    a.nodeId.localeCompare(b.nodeId) ||
    a.workspaceId.localeCompare(b.workspaceId),
  );
}
