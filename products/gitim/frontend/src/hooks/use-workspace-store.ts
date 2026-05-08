import { create } from "zustand";
import * as client from "@/lib/client";
import { activeWorkspaceStorageKey } from "@/lib/workspace-key";
import { useConnectionStore } from "@/hooks/use-connection-store";
import type { CreateWorkspaceRequest, WorkspaceSummary } from "@/lib/types";

function currentActiveKey(): string {
  return activeWorkspaceStorageKey(useConnectionStore.getState().mode);
}

function loadStoredSlug(): string | null {
  return localStorage.getItem(currentActiveKey());
}

function persistSlug(slug: string | null) {
  const key = currentActiveKey();
  if (slug) localStorage.setItem(key, slug);
  else localStorage.removeItem(key);
}

interface WorkspaceStore {
  workspaces: WorkspaceSummary[];
  activeSlug: string | null;
  loading: boolean;
  error: string | null;
  errorCode: string | null;

  fetchAll: () => Promise<void>;
  setActive: (slug: string) => void;
  create: (req: CreateWorkspaceRequest) => Promise<WorkspaceSummary | null>;
  remove: (slug: string) => Promise<boolean>;
  clearError: () => void;
}

export const useWorkspaceStore = create<WorkspaceStore>((set, get) => ({
  workspaces: [],
  activeSlug: loadStoredSlug(),
  loading: false,
  error: null,
  errorCode: null,

  clearError: () => set({ error: null, errorCode: null }),

  fetchAll: async () => {
    set({ loading: true, error: null, errorCode: null });
    const res = await client.listWorkspaces();
    if (!res.ok || !res.data) {
      set({
        loading: false,
        error: res.error ?? "Failed to list workspaces",
        errorCode: res.error_code ?? null,
      });
      return;
    }
    const workspaces = res.data.workspaces ?? [];
    const current = loadStoredSlug();
    let nextActive = current;
    if (!current || !workspaces.some((w) => w.slug === current)) {
      nextActive = workspaces[0]?.slug ?? null;
    }
    if (nextActive !== current) persistSlug(nextActive);
    set({ workspaces, activeSlug: nextActive, loading: false });
  },

  setActive: (slug) => {
    const exists = get().workspaces.some((w) => w.slug === slug);
    if (!exists) return;
    persistSlug(slug);
    set({ activeSlug: slug });
  },

  create: async (req) => {
    set({ loading: true, error: null, errorCode: null });
    const res = await client.createWorkspace(req);
    if (!res.ok || !res.data) {
      set({
        loading: false,
        error: res.error ?? "Failed to create workspace",
        errorCode: res.error_code ?? null,
      });
      return null;
    }
    // Backend returned the new workspace stub; re-list to pick up the full
    // record (including initialized status) and keep the store authoritative.
    await get().fetchAll();
    const created = get().workspaces.find((w) => w.slug === res.data!.slug);
    if (created) {
      persistSlug(created.slug);
      set({ activeSlug: created.slug });
    }
    return created ?? null;
  },

  remove: async (slug) => {
    set({ loading: true, error: null, errorCode: null });
    const res = await client.deleteWorkspace(slug);
    if (!res.ok) {
      set({
        loading: false,
        error: res.error ?? "Failed to delete workspace",
        errorCode: res.error_code ?? null,
      });
      return false;
    }
    await get().fetchAll();
    return true;
  },
}));
