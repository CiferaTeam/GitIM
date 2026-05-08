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
  workspaceId: string;
  repoDir: string;
  remoteUrl: string;
  fsName: string;
  corsProxy: string;
  token: string | null;
  me: { handler: string; display_name: string };
  channels: Map<string, ChannelMeta>;
  users: Map<string, UserMeta>;
  headCommit: string;
  syncStatus: "idle" | "syncing" | "error" | "reconnect_required";
  defaultBranch: string;
}

let state: DaemonWebState | null = null;

export function getState(): DaemonWebState {
  if (!state) throw new Error("daemon-web not initialized");
  return state;
}

export function initState(config: {
  workspaceId: string;
  repoDir: string;
  remoteUrl: string;
  fsName: string;
  corsProxy: string;
  token: string | null;
  handler: string;
  displayName: string;
}): DaemonWebState {
  state = {
    workspaceId: config.workspaceId,
    repoDir: config.repoDir,
    remoteUrl: config.remoteUrl,
    fsName: config.fsName,
    corsProxy: config.corsProxy,
    token: config.token,
    me: { handler: config.handler, display_name: config.displayName },
    channels: new Map(),
    users: new Map(),
    headCommit: "",
    syncStatus: config.token ? "idle" : "reconnect_required",
    defaultBranch: "main",
  };
  return state;
}

export function setState(partial: Partial<DaemonWebState>): void {
  if (!state) throw new Error("daemon-web not initialized");
  Object.assign(state, partial);
}
