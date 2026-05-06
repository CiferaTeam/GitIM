// In-memory state for the daemon-web Worker.
// Holds the current snapshot of the local git IM instance.

export interface ChannelMeta {
  display_name: string;
  created_by: string;
  created_at: string;
  introduction: string;
  members: string[];
}

export interface UserMeta {
  display_name: string;
  role: string;
  introduction: string;
}

export interface DaemonWebState {
  repoDir: string;
  corsProxy: string;
  token: string;
  me: { handler: string; display_name: string };
  channels: Map<string, ChannelMeta>;
  users: Map<string, UserMeta>;
  headCommit: string;
  syncStatus: "idle" | "syncing" | "error";
  defaultBranch: string;
}

let state: DaemonWebState | null = null;

export function getState(): DaemonWebState {
  if (!state) throw new Error("daemon-web not initialized");
  return state;
}

export function initState(config: {
  repoDir: string;
  corsProxy: string;
  token: string;
  handler: string;
  displayName: string;
}): DaemonWebState {
  state = {
    repoDir: config.repoDir,
    corsProxy: config.corsProxy,
    token: config.token,
    me: { handler: config.handler, display_name: config.displayName },
    channels: new Map(),
    users: new Map(),
    headCommit: "",
    syncStatus: "idle",
    defaultBranch: "main",
  };
  return state;
}

export function setState(partial: Partial<DaemonWebState>): void {
  if (!state) throw new Error("daemon-web not initialized");
  Object.assign(state, partial);
}
