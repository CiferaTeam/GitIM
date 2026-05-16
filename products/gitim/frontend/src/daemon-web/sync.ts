// Sync loop for daemon-web.
// Push-first strategy: try pushing local commits, fall back to fetch+merge on conflict.
// Conflict resolution uses parser-based renumbering (see conflict.ts).

import * as gitOps from "./git";
import { getState, setState } from "./state";
import { tokenAuth } from "./auth";
import { isAuthFailure } from "./auth-errors";
import { validateHandler } from "./paths";
import { withRepoLock } from "./repo-lock";

interface RunSyncOptions {
  forceNewCycle?: boolean;
}

export type SyncResultStatus =
  | "idle"
  | "pushed"
  | "fast_forwarded"
  | "rebased"
  | "reconnect_required";

export interface SyncResult {
  beforeHead: string;
  afterHead: string;
  changed: boolean;
  status: SyncResultStatus;
}

function isNonFastForward(e: unknown): boolean {
  const msg = String(
    (e as { message?: string })?.message ?? e ?? "",
  );
  return (
    msg.includes("not a simple fast-forward") ||
    msg.includes("non-fast-forward") ||
    msg.includes("rejected")
  );
}

function boardHandlerFromPath(path: string): string | null {
  const match = /^showboards\/([^/]+)\/board\.md$/.exec(path);
  if (!match) return null;
  return validateHandler(match[1]) ? null : match[1];
}

function parentPath(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx <= 0 ? "/" : path.slice(0, idx);
}

async function mkdirp(
  path: string,
  exists: (path: string) => Promise<boolean>,
  mkdir: (path: string) => Promise<void>,
): Promise<void> {
  const parts = path.split("/").filter(Boolean);
  let current = path.startsWith("/") ? "" : ".";
  for (const part of parts) {
    current = current === "" ? `/${part}` : `${current}/${part}`;
    if (!(await exists(current))) await mkdir(current);
  }
}

function syncResult(
  beforeHead: string,
  afterHead: string,
  status: SyncResultStatus,
): SyncResult {
  return {
    beforeHead,
    afterHead,
    changed: beforeHead !== afterHead,
    status,
  };
}

function postRepoChanged(
  commitId: string,
  reason: "fast_forward" | "rebase",
): void {
  postMessage({
    type: "repo_changed",
    commit_id: commitId,
    reason,
  });
}

function postReconnectRequired(commitId: string, error?: string): void {
  postMessage({
    type: "reconnect_required",
    commit_id: commitId,
    needs_token: true,
    error,
    error_code: "reconnect_required",
  });
}

function errorMessage(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

let syncInFlight: Promise<SyncResult> | null = null;

async function runSyncOnce(): Promise<SyncResult> {
  return withRepoLock(runSyncOnceLocked);
}

async function runSyncOnceLocked(): Promise<SyncResult> {
  const s = getState();
  const beforeHead = s.headCommit;
  if (!s.token) {
    setState({ syncStatus: "reconnect_required" });
    postReconnectRequired(beforeHead);
    return syncResult(beforeHead, beforeHead, "reconnect_required");
  }
  setState({ syncStatus: "syncing" });

  try {
    const onAuth = tokenAuth(s.token);

    // 1. Try push first (fast path: no conflicts)
    const localHead = await gitOps.resolveHead(s.repoDir);

    if (localHead !== s.headCommit) {
      try {
        await gitOps.push(s.repoDir, s.corsProxy, onAuth, s.defaultBranch);
        setState({ headCommit: localHead, syncStatus: "idle" });
        return syncResult(beforeHead, localHead, "pushed");
      } catch (e: unknown) {
        if (!isNonFastForward(e)) throw e;
        // Push rejected — need fetch+merge below
      }
    }

    // 2. Fetch from remote
    await gitOps.fetchOrigin(s.repoDir, s.corsProxy, onAuth);
    const remoteHead = await gitOps.resolveRemoteHead(s.repoDir);

    if (remoteHead === localHead) {
      setState({ headCommit: localHead, syncStatus: "idle" });
      return syncResult(beforeHead, localHead, "idle");
    }

    // 3. No local unpushed commits — fast-forward to remote
    if (localHead === s.headCommit) {
      await gitOps.resetToRemote(
        s.repoDir,
        `refs/remotes/origin/${s.defaultBranch}`,
      );
      setState({ headCommit: remoteHead, syncStatus: "idle" });
      postRepoChanged(remoteHead, "fast_forward");
      return syncResult(beforeHead, remoteHead, "fast_forwarded");
    }

    // 4. Conflict: local changes AND new remote commits.
    //    Collect append-only thread additions, reset to remote, then re-apply
    //    with renumbering. Non-thread conflicts fail safe: keep local commits
    //    in place and surface sync error instead of silently dropping changes.
    const changedFiles = await gitOps.diffTrees(
      s.repoDir,
      s.headCommit,
      localHead,
    );

    const { readFile } = await import("./storage");
    const { extractThreadAdditions } = await import("./conflict");
    const localAdditions: Record<string, string> = {};
    const remoteContents: Record<string, string> = {};
    const localBoards: Record<string, string> = {};
    for (const fp of changedFiles) {
      if (boardHandlerFromPath(fp)) {
        try {
          localBoards[fp] = await readFile(`${s.repoDir}/${fp}`);
        } catch {
          throw new Error(`Cannot auto-merge local browser sync change: ${fp}`);
        }
        continue;
      }

      try {
        const [localContent, baseContent, remoteContent] = await Promise.all([
          readFile(`${s.repoDir}/${fp}`),
          gitOps.readFileAtCommit(s.repoDir, s.headCommit, fp),
          gitOps.readFileAtCommit(s.repoDir, remoteHead, fp),
        ]);
        if (baseContent !== null && remoteContent === null) {
          throw new Error("remote file missing");
        }
        if (
          baseContent !== null &&
          remoteContent !== null &&
          !remoteContent.startsWith(baseContent)
        ) {
          throw new Error("remote file changed outside append-only shape");
        }
        const additions = extractThreadAdditions(fp, localContent, baseContent);
        if (additions.trim()) localAdditions[fp] = additions;
        if (remoteContent !== null) remoteContents[fp] = remoteContent;
      } catch {
        throw new Error(`Cannot auto-merge local browser sync change: ${fp}`);
      }
    }

    // Reset working tree to remote HEAD
    await gitOps.resetToRemote(
      s.repoDir,
      `refs/remotes/origin/${s.defaultBranch}`,
    );

    // Resolve via parser-based renumbering
    const { resolveConflicts } = await import("./conflict");
    const resolved = resolveConflicts(localAdditions, remoteContents);

    // Write resolved files back
    const { writeFile, exists, mkdir } = await import("./storage");
    const filePaths: string[] = [];
    for (const [fp, content] of Object.entries(resolved.files)) {
      await writeFile(`${s.repoDir}/${fp}`, content);
      filePaths.push(fp);
    }
    for (const [fp, content] of Object.entries(localBoards)) {
      const absPath = `${s.repoDir}/${fp}`;
      await mkdirp(parentPath(absPath), exists, mkdir);
      await writeFile(absPath, content);
      filePaths.push(fp);
    }

    // Commit the merge result
    const hasThreadFiles = Object.keys(resolved.files).length > 0;
    const hasBoardFiles = Object.keys(localBoards).length > 0;
    const commitMessage = hasBoardFiles && !hasThreadFiles
      ? "board: sync after rebase"
      : resolved.commitMessage;
    await gitOps.addAndCommit(
      s.repoDir,
      filePaths,
      commitMessage,
      s.me.handler,
    );

    // Push with retry (max 3 attempts for concurrent-write races)
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        await gitOps.push(s.repoDir, s.corsProxy, onAuth, s.defaultBranch);
        break;
      } catch (e: unknown) {
        if (attempt === 2 || !isNonFastForward(e)) throw e;
        await gitOps.fetchOrigin(s.repoDir, s.corsProxy, onAuth);
      }
    }

    const newHead = await gitOps.resolveHead(s.repoDir);
    setState({ headCommit: newHead, syncStatus: "idle" });
    postRepoChanged(newHead, "rebase");
    return syncResult(beforeHead, newHead, "rebased");
  } catch (e) {
    const message = errorMessage(e);
    if (isAuthFailure(e)) {
      setState({ token: null, syncStatus: "reconnect_required" });
      postReconnectRequired(getState().headCommit, message);
    } else {
      setState({ syncStatus: "error" });
      postMessage({ type: "sync_error", error: message });
    }
    console.error("[daemon-web] sync error:", e);
    throw e;
  }
}

async function runSync(options: RunSyncOptions = {}): Promise<SyncResult> {
  if (syncInFlight && !options.forceNewCycle) return syncInFlight;

  const previous = syncInFlight;
  const next = (async () => {
    if (previous) {
      try {
        await previous;
      } catch {
        /* A fresh cycle below reports its own result. */
      }
    }
    return await runSyncOnce();
  })();

  syncInFlight = next;
  next.then(
    () => {
      if (syncInFlight === next) syncInFlight = null;
    },
    () => {
      if (syncInFlight === next) syncInFlight = null;
    },
  );

  return next;
}

// --- Sync loop management ---

let syncTimer: ReturnType<typeof setInterval> | null = null;
const SYNC_INTERVAL_MS = 7_000;

export function startSyncLoop(): void {
  if (syncTimer) return;
  syncTimer = setInterval(() => {
    runSync().catch(console.error);
  }, SYNC_INTERVAL_MS);
}

export function stopSyncLoop(): void {
  if (syncTimer) {
    clearInterval(syncTimer);
    syncTimer = null;
  }
}

export { runSync };
