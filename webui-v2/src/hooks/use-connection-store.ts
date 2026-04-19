import { create } from "zustand";

export type ConnectionStatus =
  | "checking"       // trying stored port
  | "disconnected"   // no runtime found, show port form
  | "ready";         // runtime reachable, app can proceed

const STORAGE_KEY = "gitim-runtime-port";

interface ConnectionState {
  status: ConnectionStatus;
  port: number | null;
  runtimeVersion: string | null;
  error: string | null;

  setStatus: (s: ConnectionStatus) => void;
  setPort: (p: number) => void;
  setRuntimeVersion: (v: string | null) => void;
  setError: (e: string | null) => void;
  baseUrl: () => string;
}

function loadStoredPort(): number | null {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) ? n : null;
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  status: "checking",
  port: loadStoredPort(),
  runtimeVersion: null,
  error: null,

  setStatus: (s) => set({ status: s, error: null }),
  setPort: (p) => {
    localStorage.setItem(STORAGE_KEY, String(p));
    set({ port: p });
  },
  setRuntimeVersion: (v) => set({ runtimeVersion: v }),
  setError: (e) => set({ error: e }),
  baseUrl: () => `http://127.0.0.1:${get().port}`,
}));
