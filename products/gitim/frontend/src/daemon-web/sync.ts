// Sync loop for daemon-web.
// Push-first strategy: try pushing local commits, fall back to fetch+merge on conflict.
// Conflict resolution uses parser-based renumbering (see conflict.ts).

import * as gitOps from "./git";
import { getState, setState } from "./state";
import { tokenAuth } from "./auth";

interface RunSyncOptions {
  forceNewCycle?: boolean;
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

let syncInFlight: Promise<void> | null = null;

async function runSyncOnce(): Promise<void> {
  const s = getState();
  if (!s.token) {
    setState({ syncStatus: "reconnect_required" });
    return;
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
        return;
      } catch (e: unknown) {
        if (!isNonFastForward(e)) throw e;
        // Push rejected — need fetch+merge below
      }
    }

    // 2. Fetch from remote
    await gitOps.fetchOrigin(s.repoDir, s.corsProxy, onAuth);
    const remoteHead = await gitOps.resolveRemoteHead(s.repoDir);

    if (remoteHead === localHead) {
      setState({ syncStatus: "idle" });
      return;
    }

    // 3. No local unpushed commits — fast-forward to remote
    if (localHead === s.headCommit) {
      await gitOps.resetToRemote(
        s.repoDir,
        `refs/remotes/origin/${s.defaultBranch}`,
      );
      setState({ headCommit: remoteHead, syncStatus: "idle" });
      postMessage({ type: "sync_reset" });
      return;
    }

    // 4. Conflict: local changes AND new remote commits.
    //    Collect local additions, reset to remote, then re-apply with renumbering.
    const changedFiles = await gitOps.diffTrees(
      s.repoDir,
      s.headCommit,
      localHead,
    );

    const { readFile } = await import("./storage");
    const localAdditions: Record<string, string> = {};
    for (const fp of changedFiles) {
      try {
        localAdditions[fp] = await readFile(`${s.repoDir}/${fp}`);
      } catch {
        /* file deleted locally, skip */
      }
    }

    // Reset working tree to remote HEAD
    await gitOps.resetToRemote(
      s.repoDir,
      `refs/remotes/origin/${s.defaultBranch}`,
    );

    // Read remote versions for conflict resolution
    const remoteContents: Record<string, string> = {};
    for (const fp of changedFiles) {
      try {
        remoteContents[fp] = await readFile(`${s.repoDir}/${fp}`);
      } catch {
        /* new file on local side only */
      }
    }

    // Resolve via parser-based renumbering
    const { resolveConflicts } = await import("./conflict");
    const resolved = resolveConflicts(localAdditions, remoteContents);

    // Write resolved files back
    const { writeFile } = await import("./storage");
    const filePaths: string[] = [];
    for (const [fp, content] of Object.entries(resolved.files)) {
      await writeFile(`${s.repoDir}/${fp}`, content);
      filePaths.push(fp);
    }

    // Commit the merge result
    await gitOps.addAndCommit(
      s.repoDir,
      filePaths,
      resolved.commitMessage,
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
    postMessage({ type: "sync_reset" });
  } catch (e) {
    setState({ syncStatus: "error" });
    console.error("[daemon-web] sync error:", e);
    throw e;
  }
}

async function runSync(options: RunSyncOptions = {}): Promise<void> {
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
    await runSyncOnce();
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
