import { create } from "zustand";

import type { FlowRunDetail, FlowRunSummary } from "@/lib/types";

interface FlowRunState {
  runsByChannel: Record<string, FlowRunSummary[]>;
  selectedRun: FlowRunDetail | null;

  setRunsForChannel: (channel: string, runs: FlowRunSummary[]) => void;
  setSelectedRun: (run: FlowRunDetail | null) => void;
  resetForWorkspaceSwitch: () => void;
}

export const useFlowRunStore = create<FlowRunState>((set) => ({
  runsByChannel: {},
  selectedRun: null,

  setRunsForChannel: (channel, runs) =>
    set((s) => ({
      runsByChannel: { ...s.runsByChannel, [channel]: runs },
    })),

  setSelectedRun: (run) => set({ selectedRun: run }),

  resetForWorkspaceSwitch: () =>
    set({ runsByChannel: {}, selectedRun: null }),
}));
