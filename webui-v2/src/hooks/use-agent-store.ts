import { create } from "zustand";
import type { Agent } from "../lib/types";

interface AgentState {
  agents: Agent[];
  selectedAgentId: string | null;

  setAgents: (a: Agent[]) => void;
  addAgent: (a: Agent) => void;
  removeAgent: (id: string) => void;
  updateAgent: (id: string, updates: Partial<Agent>) => void;
  selectAgent: (id: string | null) => void;
}

export const useAgentStore = create<AgentState>((set) => ({
  agents: [],
  selectedAgentId: null,

  setAgents: (a) => set({ agents: a }),

  addAgent: (a) => set((state) => ({ agents: [...state.agents, a] })),

  removeAgent: (id) =>
    set((state) => ({
      agents: state.agents.filter((a) => a.id !== id),
      selectedAgentId: state.selectedAgentId === id ? null : state.selectedAgentId,
    })),

  updateAgent: (id, updates) =>
    set((state) => ({
      agents: state.agents.map((a) =>
        a.id === id ? { ...a, ...updates } : a
      ),
    })),

  selectAgent: (id) => set({ selectedAgentId: id }),
}));
