import { create } from "zustand";

import type { FlowDocument, FlowSummary } from "@/lib/types";

interface FlowState {
  flows: FlowSummary[];
  selectedSlug: string | null;
  selectedFlow: FlowDocument | null;

  setFlows: (flows: FlowSummary[]) => void;
  setSelectedSlug: (slug: string | null) => void;
  setSelectedFlow: (flow: FlowDocument | null) => void;
  resetForWorkspaceSwitch: () => void;
}

export const useFlowStore = create<FlowState>((set) => ({
  flows: [],
  selectedSlug: null,
  selectedFlow: null,

  setFlows: (flows) =>
    set((state) => {
      // Preserve selection if the selected flow still exists, otherwise clear.
      const keep =
        state.selectedSlug &&
        flows.some((f) => f.slug === state.selectedSlug)
          ? state.selectedSlug
          : null;
      const selectedSlug = keep ?? null;
      const selectedFlow =
        state.selectedFlow?.slug === selectedSlug ? state.selectedFlow : null;
      return { flows, selectedSlug, selectedFlow };
    }),

  setSelectedSlug: (slug) =>
    set((state) => ({
      selectedSlug: slug,
      // Clear detail if switching to a different flow.
      selectedFlow:
        state.selectedFlow?.slug === slug ? state.selectedFlow : null,
    })),

  setSelectedFlow: (flow) =>
    set({
      selectedFlow: flow,
      selectedSlug: flow?.slug ?? null,
    }),

  resetForWorkspaceSwitch: () =>
    set({
      flows: [],
      selectedSlug: null,
      selectedFlow: null,
    }),
}));
