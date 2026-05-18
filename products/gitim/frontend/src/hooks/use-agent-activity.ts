import { useEffect, useRef } from "react";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { mapBackendUsageSummary } from "../lib/client";
import type { Agent, AgentActivityEvent } from "../lib/types";
import { useConnectionStore } from "./use-connection-store";
import { useAgentStore } from "./use-agent-store";
import { useWorkspaceStore } from "./use-workspace-store";

const MAX_EVENTS_PER_AGENT = 20;

interface AgentActivityState {
  /** Per-agent activity buffer, newest first. */
  activities: Record<string, AgentActivityEvent[]>;
  /** Slug the persisted activities belong to; used to invalidate on switch. */
  lastSlug: string | null;
  push: (event: AgentActivityEvent) => void;
  pushForKey: (key: string, event: AgentActivityEvent) => void;
  /** If slug differs from lastSlug, wipe activities; always records lastSlug. */
  ensureSlug: (slug: string) => void;
  clear: () => void;
}

function pushActivity(
  activities: Record<string, AgentActivityEvent[]>,
  key: string,
  event: AgentActivityEvent,
) {
  const prev = activities[key] ?? [];
  const next = [event, ...prev].slice(0, MAX_EVENTS_PER_AGENT);
  return { ...activities, [key]: next };
}

export const useAgentActivityStore = create<AgentActivityState>()(
  persist(
    (set) => ({
      activities: {},
      lastSlug: null,
      push: (event) =>
        set((state) => {
          return {
            activities: pushActivity(
              state.activities,
              event.agent_id,
              event,
            ),
          };
        }),
      pushForKey: (key, event) =>
        set((state) => {
          return {
            activities: pushActivity(state.activities, key, event),
          };
        }),
      ensureSlug: (slug) =>
        set((state) =>
          state.lastSlug === slug
            ? { lastSlug: slug }
            : { activities: {}, lastSlug: slug },
        ),
      clear: () => set({ activities: {}, lastSlug: null }),
    }),
    {
      name: "gitim/agent-activity",
      partialize: (state) => ({
        activities: state.activities,
        lastSlug: state.lastSlug,
      }),
    },
  ),
);

export function applyUsageActivityEvent(event: AgentActivityEvent) {
  if (event.detail.trim() === "") {
    useAgentStore.getState().updateAgent(event.agent_id, {
      sessionUsage: undefined,
    });
    return;
  }

  try {
    const snap = JSON.parse(event.detail);
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
    useAgentStore.getState().updateAgent(event.agent_id, updates);
  } catch {
    // malformed usage payload — ignore
  }
}

/**
 * Connects to the SSE endpoint for agent activity events for the given
 * workspace. Closes and re-opens when `slug` changes so events from the
 * previous workspace don't leak into the new one.
 */
export function useAgentActivitySSE(slug: string | null) {
  const port = useConnectionStore((s) => s.port);
  const push = useAgentActivityStore((s) => s.push);
  const ensureSlug = useAgentActivityStore((s) => s.ensureSlug);
  const esRef = useRef<EventSource | null>(null);
  const refreshRef = useRef(false);

  useEffect(() => {
    if (!port || !slug) return;

    // Keep persisted activities if we're reloading into the same workspace;
    // wipe only on a real workspace switch.
    ensureSlug(slug);

    const url = `http://127.0.0.1:${port}/workspaces/${encodeURIComponent(slug)}/agents/events`;
    const es = new EventSource(url);
    esRef.current = es;

    es.onmessage = (e) => {
      try {
        const event: AgentActivityEvent = JSON.parse(e.data);
        if (event.event_type === "usage") {
          applyUsageActivityEvent(event);
          return; // do NOT push usage events to the activity log
        }
        if (event.event_type === "burned") {
          // archive-protocol terminal event: agent was burned (either via
          // the WebUI burn button or the B.4 self-departed self-heal).
          // Drop it from the active store so the agent disappears from
          // the management list. The detail-page burn dialog also calls
          // removeAgent eagerly to avoid a ghost row when SSE is delayed.
          useAgentStore.getState().removeAgent(event.agent_id);
          return; // skip activity-log push — the agent is gone
        }
        push(event);
      } catch {
        // ignore malformed events
      }
    };

    es.onerror = () => {
      if (refreshRef.current) return;
      refreshRef.current = true;
      void useWorkspaceStore
        .getState()
        .refreshAfterActiveUnavailable(slug)
        .finally(() => {
          refreshRef.current = false;
        });
    };

    return () => {
      es.close();
      esRef.current = null;
    };
  }, [port, slug, push, ensureSlug]);
}
