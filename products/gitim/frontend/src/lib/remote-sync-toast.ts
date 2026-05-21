import { toast } from "sonner";
import type { Message, PollChange } from "./types";

export interface PendingRemoteSync {
  scope: string;
  author: string;
  body: string;
  lineNumber?: number;
}

type RemoteEntry = {
  scope: string;
  message: Message;
};

const pendingByWorkspace = new Map<string, PendingRemoteSync[]>();

function toastId(workspaceKey: string): string {
  return `remote-sync:${workspaceKey}`;
}

export function remoteSyncFailure(
  data: Record<string, unknown> | undefined,
): string | null {
  if (!data) return null;
  const status = typeof data.status === "string" ? data.status : data.sync_status;
  const error = typeof data.error === "string"
    ? data.error
    : typeof data.sync_error === "string"
      ? data.sync_error
      : null;
  return status === "commit_only" || error ? error ?? "Sync failed" : null;
}

export function recordRemoteSyncPending(
  workspaceKey: string | null,
  item: PendingRemoteSync,
  description: string,
): void {
  if (!workspaceKey) return;
  const pending = pendingByWorkspace.get(workspaceKey) ?? [];
  pending.push(item);
  pendingByWorkspace.set(workspaceKey, pending);
  toast.info("Saved locally, syncing to remote…", {
    id: toastId(workspaceKey),
    description,
  });
}

function changeScope(change: PollChange): string {
  return change.channel;
}

function remoteEntries(changes: PollChange[]): RemoteEntry[] {
  const entries: RemoteEntry[] = [];
  for (const change of changes) {
    if (!change.entries?.length) continue;
    const scope = changeScope(change);
    for (const entry of change.entries as Message[]) {
      entries.push({ scope, message: entry });
    }
  }
  return entries;
}

function matchesPending(item: PendingRemoteSync, entry: RemoteEntry): boolean {
  if (entry.scope !== item.scope) return false;
  if (entry.message.author !== item.author) return false;
  if (
    item.lineNumber !== undefined &&
    entry.message.line_number === item.lineNumber
  ) {
    return true;
  }
  return entry.message.body === item.body;
}

export function resolveRemoteSyncFromChanges(
  workspaceKey: string | null,
  changes: PollChange[],
): number {
  if (!workspaceKey) return 0;
  const pending = pendingByWorkspace.get(workspaceKey);
  if (!pending?.length) return 0;

  const entries = remoteEntries(changes);
  if (!entries.length) return 0;

  const consumed = new Set<number>();
  const remaining: PendingRemoteSync[] = [];
  let resolved = 0;

  for (const item of pending) {
    const matchIndex = entries.findIndex((entry, index) =>
      !consumed.has(index) && matchesPending(item, entry)
    );
    if (matchIndex === -1) {
      remaining.push(item);
      continue;
    }
    consumed.add(matchIndex);
    resolved += 1;
  }

  if (resolved === 0) return 0;

  if (remaining.length > 0) {
    pendingByWorkspace.set(workspaceKey, remaining);
    toast.info("Remote sync progressing…", {
      id: toastId(workspaceKey),
      description: `${resolved} uploaded, ${remaining.length} still waiting.`,
    });
  } else {
    pendingByWorkspace.delete(workspaceKey);
    toast.success("Synced to remote", {
      id: toastId(workspaceKey),
      description: "Queued local changes uploaded after retry.",
    });
  }

  return resolved;
}

export function resetRemoteSyncToastState(): void {
  pendingByWorkspace.clear();
}
