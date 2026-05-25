import { create } from "zustand";

export type BrowserSyncStatus =
  | "unknown"
  | "idle"
  | "syncing"
  | "error"
  | "reconnect_required";

export type PollFailureKind =
  | "transport"
  | "workspace"
  | "token"
  | "activation";

interface PollDiagnostics {
  lastSuccessAt: string | null;
  lastErrorAt: string | null;
  lastError: string | null;
  lastFailureKind: PollFailureKind | null;
  consecutiveTransportFailures: number;
  consecutiveWorkspaceFailures: number;
  lastCommit: string | null;
}

interface BrowserSyncDiagnostics {
  status: BrowserSyncStatus;
  lastEventAt: string | null;
  lastErrorAt: string | null;
  lastError: string | null;
  needsToken: boolean;
  corsProxy: string | null;
  remoteUrl: string | null;
  headCommit: string | null;
}

interface BrowserSyncEvent {
  status: BrowserSyncStatus;
  error?: string | null;
  needsToken?: boolean;
  corsProxy?: string | null;
  remoteUrl?: string | null;
  headCommit?: string | null;
}

interface ConnectionDiagnosticsState {
  poll: PollDiagnostics;
  browserSync: BrowserSyncDiagnostics;
  recordPollSuccess: (commitId?: string | null) => void;
  recordPollFailure: (kind: PollFailureKind, error?: unknown) => void;
  recordBrowserSyncEvent: (event: BrowserSyncEvent) => void;
  reset: () => void;
}

const initialPoll: PollDiagnostics = {
  lastSuccessAt: null,
  lastErrorAt: null,
  lastError: null,
  lastFailureKind: null,
  consecutiveTransportFailures: 0,
  consecutiveWorkspaceFailures: 0,
  lastCommit: null,
};

const initialBrowserSync: BrowserSyncDiagnostics = {
  status: "unknown",
  lastEventAt: null,
  lastErrorAt: null,
  lastError: null,
  needsToken: false,
  corsProxy: null,
  remoteUrl: null,
  headCommit: null,
};

function nowIso(): string {
  return new Date().toISOString();
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return String(error ?? "Unknown error");
}

export const useConnectionDiagnosticsStore = create<ConnectionDiagnosticsState>(
  (set) => ({
    poll: initialPoll,
    browserSync: initialBrowserSync,

    recordPollSuccess: (commitId) =>
      set((state) => ({
        poll: {
          ...state.poll,
          lastSuccessAt: nowIso(),
          lastError: null,
          lastErrorAt: null,
          lastFailureKind: null,
          consecutiveTransportFailures: 0,
          consecutiveWorkspaceFailures: 0,
          lastCommit: commitId ?? state.poll.lastCommit,
        },
      })),

    recordPollFailure: (kind, error) =>
      set((state) => {
        const transportFailures =
          kind === "transport"
            ? state.poll.consecutiveTransportFailures + 1
            : state.poll.consecutiveTransportFailures;
        const workspaceFailures =
          kind === "workspace"
            ? state.poll.consecutiveWorkspaceFailures + 1
            : state.poll.consecutiveWorkspaceFailures;

        return {
          poll: {
            ...state.poll,
            lastErrorAt: nowIso(),
            lastError: errorMessage(error),
            lastFailureKind: kind,
            consecutiveTransportFailures: transportFailures,
            consecutiveWorkspaceFailures: workspaceFailures,
          },
        };
      }),

    recordBrowserSyncEvent: (event) =>
      set((state) => {
        const error = event.error ?? null;
        return {
          browserSync: {
            ...state.browserSync,
            status: event.status,
            lastEventAt: nowIso(),
            lastError: error,
            lastErrorAt: error ? nowIso() : state.browserSync.lastErrorAt,
            needsToken: event.needsToken ?? state.browserSync.needsToken,
            corsProxy: event.corsProxy ?? state.browserSync.corsProxy,
            remoteUrl: event.remoteUrl ?? state.browserSync.remoteUrl,
            headCommit: event.headCommit ?? state.browserSync.headCommit,
          },
        };
      }),

    reset: () =>
      set({
        poll: initialPoll,
        browserSync: initialBrowserSync,
      }),
  }),
);
