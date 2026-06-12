import { create } from "zustand";
import * as client from "@/lib/client";
import type { Project } from "@/lib/types";

interface ProjectStore {
  /** Loaded project list for the active workspace. Stable reference:
   *  only replaced by fetch/setProjects — never mutated in place. */
  projects: Project[];
  loading: boolean;
  error: string | null;
  /** Fetch projects for the given workspace slug from the runtime.
   *  In browser mode the client returns [] immediately (no daemon). */
  fetch: (workspace: string) => Promise<void>;
  /** Replace the project list wholesale — used for SSE push or
   *  optimistic local updates. */
  setProjects: (projects: Project[]) => void;
  /** Reset to initial state on workspace switch. */
  reset: () => void;
}

export const useProjectStore = create<ProjectStore>((set) => ({
  projects: [],
  loading: false,
  error: null,

  fetch: async (workspace) => {
    set({ loading: true, error: null });
    try {
      const projects = await client.listProjects(workspace);
      set({ projects, loading: false });
    } catch (e) {
      set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    }
  },

  setProjects: (projects) => {
    set({ projects });
  },

  reset: () => {
    set({ projects: [], loading: false, error: null });
  },
}));
