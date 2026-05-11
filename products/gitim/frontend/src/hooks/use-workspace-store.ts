import { create } from "zustand";
import * as client from "@/lib/client";
import { activeWorkspaceStorageKey, workspaceIdentity } from "@/lib/workspace-key";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { clearUiState } from "@/lib/ui-state";
import type { CreateWorkspaceRequest, WorkspaceSummary } from "@/lib/types";

function currentActiveKey(): string {
  return activeWorkspaceStorageKey(useConnectionStore.getState().mode);
}

function loadStoredSlug(key = currentActiveKey()): string | null {
  return localStorage.getItem(key);
}

function persistSlug(slug: string | null, key = currentActiveKey()) {
  if (slug) localStorage.setItem(key, slug);
  else localStorage.removeItem(key);
}

let fetchAllRequestId = 0;

interface WorkspaceStore {
  workspaces: WorkspaceSummary[];
  activeSlug: string | null;
  loading: boolean;
  error: string | null;
  errorCode: string | null;

  fetchAll: () => Promise<void>;
  refreshAfterActiveUnavailable: (slug: string) => Promise<void>;
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
    const requestId = fetchAllRequestId + 1;
    fetchAllRequestId = requestId;
    const mode = useConnectionStore.getState().mode;
    const activeKey = activeWorkspaceStorageKey(mode);
    set({ loading: true, error: null, errorCode: null });
    const res = await client.listWorkspaces();
    if (requestId !== fetchAllRequestId || useConnectionStore.getState().mode !== mode) {
      return;
    }
    if (!res.ok || !res.data) {
      set({
        loading: false,
        error: res.error ?? "Failed to list workspaces",
        errorCode: res.error_code ?? null,
      });
      return;
    }
    const workspaces = res.data.workspaces ?? [];
    const current = loadStoredSlug(activeKey);
    let nextActive = current;
    if (!current || !workspaces.some((w) => w.slug === current)) {
      nextActive = workspaces[0]?.slug ?? null;
    }
    if (nextActive !== current) persistSlug(nextActive, activeKey);
    set({ workspaces, activeSlug: nextActive, loading: false });
  },

  refreshAfterActiveUnavailable: async (slug) => {
    if (get().activeSlug !== slug) return;
    await get().fetchAll();
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
    // Capture workspace and mode before the await so the lookup is deterministic.
    const workspace = get().workspaces.find((w) => w.slug === slug);
    const mode = useConnectionStore.getState().mode;
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
    // Clear before fetchAll so loadStoredSlug doesn't pick up a stale active slug.
    if (workspace) clearUiState(workspaceIdentity(mode, workspace));
    await get().fetchAll();
    return true;
  },
}));
