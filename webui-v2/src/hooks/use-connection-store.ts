import { create } from "zustand";

export type ConnectionStatus =
  | "checking" // trying stored port
  | "disconnected" // no runtime found, show port form
  | "connected" // health OK, need workspace
  | "workspace_set" // workspace configured, need git provider
  | "ready"; // git initialized, app can proceed

export type ConnectionMode = "remote" | "local";

const STORAGE_KEY = "gitim-runtime-port";
const MODE_KEY = "gitim-mode";

interface ConnectionState {
  mode: ConnectionMode;
  status: ConnectionStatus;
  port: number | null;
  runtimeVersion: string | null;
  workspacePath: string | null;
  error: string | null;
  // Local mode state
  localReady: boolean;
  cloneProgress: string | null;

  setMode: (m: ConnectionMode) => void;
  setStatus: (s: ConnectionStatus) => void;
  setPort: (p: number) => void;
  setRuntimeVersion: (v: string) => void;
  setWorkspacePath: (p: string) => void;
  setError: (e: string | null) => void;
  setLocalReady: (v: boolean) => void;
  setCloneProgress: (p: string | null) => void;
  baseUrl: () => string;
}

function loadStoredPort(): number | null {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}

function loadStoredMode(): ConnectionMode {
  return (localStorage.getItem(MODE_KEY) as ConnectionMode) || "remote";
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  mode: loadStoredMode(),
  status: "checking",
  port: loadStoredPort(),
  runtimeVersion: null,
  workspacePath: null,
  error: null,
  localReady: false,
  cloneProgress: null,

  setMode: (m) => {
    localStorage.setItem(MODE_KEY, m);
    set({ mode: m });
  },
  setStatus: (s) => set({ status: s, error: null }),
  setPort: (p) => {
    localStorage.setItem(STORAGE_KEY, String(p));
    set({ port: p });
  },
  setRuntimeVersion: (v) => set({ runtimeVersion: v }),
  setWorkspacePath: (p) => set({ workspacePath: p }),
  setError: (e) => set({ error: e }),
  setLocalReady: (v) => set({ localReady: v }),
  setCloneProgress: (p) => set({ cloneProgress: p }),
  baseUrl: () => `http://127.0.0.1:${get().port}`,
}));
