import { useEffect, useRef } from "react";
import { create } from "zustand";
import type { AgentActivityEvent } from "../lib/types";
import { useConnectionStore } from "./use-connection-store";

const MAX_EVENTS_PER_AGENT = 20;

interface AgentActivityState {
  /** Per-agent activity buffer, newest first. */
  activities: Record<string, AgentActivityEvent[]>;
  push: (event: AgentActivityEvent) => void;
  clear: () => void;
}

export const useAgentActivityStore = create<AgentActivityState>((set) => ({
  activities: {},
  push: (event) =>
    set((state) => {
      const prev = state.activities[event.agent_id] ?? [];
      const next = [event, ...prev].slice(0, MAX_EVENTS_PER_AGENT);
      return { activities: { ...state.activities, [event.agent_id]: next } };
    }),
  clear: () => set({ activities: {} }),
}));

/**
 * Connects to the SSE endpoint for agent activity events.
 * Call once at the app level (e.g., in App.tsx).
 */
export function useAgentActivitySSE(enabled = true) {
  const port = useConnectionStore((s) => s.port);
  const push = useAgentActivityStore((s) => s.push);
  const esRef = useRef<EventSource | null>(null);

  useEffect(() => {
    if (!enabled || !port) return;

    const url = `http://127.0.0.1:${port}/agents/events`;
    const es = new EventSource(url);
    esRef.current = es;

    es.onmessage = (e) => {
      try {
        const event: AgentActivityEvent = JSON.parse(e.data);
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
  }, [enabled, port, push]);
}
