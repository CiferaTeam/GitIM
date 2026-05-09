import { useEffect, useRef } from "react";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { AgentActivityEvent } from "../lib/types";
import { useConnectionStore } from "./use-connection-store";
import { useAgentStore } from "./use-agent-store";

const MAX_EVENTS_PER_AGENT = 20;

interface AgentActivityState {
  /** Per-agent activity buffer, newest first. */
  activities: Record<string, AgentActivityEvent[]>;
  /** Slug the persisted activities belong to; used to invalidate on switch. */
  lastSlug: string | null;
  push: (event: AgentActivityEvent) => void;
  /** If slug differs from lastSlug, wipe activities; always records lastSlug. */
  ensureSlug: (slug: string) => void;
  clear: () => void;
}

export const useAgentActivityStore = create<AgentActivityState>()(
  persist(
    (set) => ({
      activities: {},
      lastSlug: null,
      push: (event) =>
        set((state) => {
          const prev = state.activities[event.agent_id] ?? [];
          const next = [event, ...prev].slice(0, MAX_EVENTS_PER_AGENT);
          return { activities: { ...state.activities, [event.agent_id]: next } };
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
          try {
            const snap = JSON.parse(event.detail);
            useAgentStore.getState().updateAgent(event.agent_id, {
              sessionUsage: {
                sessionId: snap.session_id ?? "",
                inputTokens: snap.input_tokens,
                outputTokens: snap.output_tokens,
                maxTokens: snap.max_tokens,
                usedPercent: snap.used_percent ?? 0,
                source: snap.source ?? "provider_reported",
                updatedAt: snap.updated_at ?? "",
              },
            });
          } catch {
            // malformed usage payload — ignore
          }
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
      // EventSource auto-reconnects; no action needed
    };

    return () => {
      es.close();
      esRef.current = null;
    };
  }, [port, slug, push, ensureSlug]);
}
