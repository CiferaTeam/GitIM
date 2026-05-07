import { create } from "zustand";

export type ConnectionStatus =
  | "checking"       // trying stored port
  | "disconnected"   // no runtime found, show port form
  | "ready";         // runtime reachable, app can proceed

export type ConnectionMode = "remote" | "local";

const STORAGE_KEY = "gitim-runtime-port";
const MODE_KEY = "gitim-connection-mode";

interface ConnectionState {
  mode: ConnectionMode;
  status: ConnectionStatus;
  port: number | null;
  runtimeVersion: string | null;
  // Active workspace's current git HEAD commit, refreshed on every /im/poll.
  // Advances as sync_loop fetches/pushes — visible indicator that the
  // workspace is "live" and progressing. Null before first successful poll
  // or immediately after a workspace switch.
  headCommit: string | null;
  error: string | null;

  // Self-update state machine (Task 8). Set by `useVersionCheck.triggerUpdate`.
  // - isUpdating: update-and-restart accepted, polling /health
  // - isRestarting: /health unreachable (parent exiting / child rebinding)
  // - updateError: surfaced error string for the UI
  isUpdating: boolean;
  isRestarting: boolean;
  updateError: string | null;
  localReady: boolean;
  cloneProgress: string | null;

  setMode: (m: ConnectionMode) => void;
  setStatus: (s: ConnectionStatus) => void;
  setPort: (p: number) => void;
  setRuntimeVersion: (v: string | null) => void;
  setHeadCommit: (v: string | null) => void;
  setError: (e: string | null) => void;
  setIsUpdating: (v: boolean) => void;
  setIsRestarting: (v: boolean) => void;
  setUpdateError: (e: string | null) => void;
  setLocalReady: (v: boolean) => void;
  setCloneProgress: (v: string | null) => void;
  baseUrl: () => string;
}

function loadStoredPort(): number | null {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}

function loadStoredMode(): ConnectionMode {
  return localStorage.getItem(MODE_KEY) === "local" ? "local" : "remote";
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  mode: loadStoredMode(),
  status: "checking",
  port: loadStoredPort(),
  runtimeVersion: null,
  headCommit: null,
  error: null,

  isUpdating: false,
  isRestarting: false,
  updateError: null,
  localReady: false,
  cloneProgress: null,

  setMode: (m) => {
    localStorage.setItem(MODE_KEY, m);
    set({ mode: m, status: m === "local" ? "disconnected" : "checking", error: null });
  },
  setStatus: (s) => set({ status: s, error: null }),
  setPort: (p) => {
    localStorage.setItem(STORAGE_KEY, String(p));
    set({ port: p });
  },
  setRuntimeVersion: (v) => set({ runtimeVersion: v }),
  setHeadCommit: (v) => set({ headCommit: v }),
  setError: (e) => set({ error: e }),
  setIsUpdating: (v) => set({ isUpdating: v }),
  setIsRestarting: (v) => set({ isRestarting: v }),
  setUpdateError: (e) => set({ updateError: e }),
  setLocalReady: (v) => set({ localReady: v }),
  setCloneProgress: (v) => set({ cloneProgress: v }),
  baseUrl: () => `http://127.0.0.1:${get().port}`,
}));
