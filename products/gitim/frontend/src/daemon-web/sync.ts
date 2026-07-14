// Sync loop for daemon-web.
// Push-first strategy: try pushing local commits, fall back to fetch+merge on conflict.
// Conflict resolution uses parser-based renumbering (see conflict.ts).

import * as gitOps from "./git";
import { getState, setState } from "./state";
import { tokenAuth } from "./auth";
import { isAuthFailure } from "./auth-errors";
import { validateHandler } from "./paths";
import { withRepoLock } from "./repo-lock";
import { ensureWasmReady } from "./wasm-ready";
import {
  mergeQuickSessionMeta,
  parseQuickSessionMeta,
  serializeQuickSessionMeta,
} from "gitim-wasm";

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

interface QuickSessionChangedPaths {
  id: string;
  activeMetaPath?: string;
  activeThreadPath?: string;
  archivedMetaPath?: string;
  archivedThreadPath?: string;
}

interface QuickSessionMove {
  targetMetaPath: string;
  targetThreadPath: string;
  sourceMetaPath: string;
  sourceThreadPath: string;
  targetMeta: string;
  targetThread: string;
  commitMessage: string;
}

interface QuickSessionRelocation {
  targetMetaPath: string;
  targetThreadPath: string;
  sourceMetaPath: string;
  sourceThreadPath: string;
}

function quickSessionFileFromPath(path: string): {
  archived: boolean;
  id: string;
  file: "meta" | "thread";
} | null {
  const match = /^(archive\/)?quick-sessions\/([^/]+)\/(session\.meta\.yaml|discussion\.thread)$/.exec(
    path,
  );
  if (!match) return null;
  return {
    archived: match[1] !== undefined,
    id: match[2],
    file: match[3] === "session.meta.yaml" ? "meta" : "thread",
  };
}

function parentPath(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx <= 0 ? "/" : path.slice(0, idx);
}

function quickSessionPairState(
  meta: string | null,
  thread: string | null,
  description: string,
): "present" | "absent" {
  if (meta === null && thread === null) return "absent";
  if (meta !== null && thread !== null) return "present";
  throw new Error(`${description} transaction is incomplete`);
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

/** Epoch fence, browser edition. Reads the working tree's
 *  gitim.epoch.yaml before publishing local work and after remote state is
 *  materialized. A redirected status latches `epochRedirected` (write
 *  handlers refuse from then on) and parks syncStatus on "epoch_redirected"
 *  for the UI banner. Returns true when latched. */
async function detectEpochRedirect(repoDir: string): Promise<boolean> {
  try {
    const { readFile } = await import("./storage");
    const yaml = await readFile(`${repoDir}/gitim.epoch.yaml`);
    if (/^status:\s*redirected/m.test(yaml)) {
      setState({ epochRedirected: true, syncStatus: "epoch_redirected" });
      postMessage({ type: "epoch_redirected" });
      return true;
    }
  } catch {
    // No epoch file (the normal case) or unreadable — stay open.
  }
  return false;
}

async function runSyncOnceLocked(): Promise<SyncResult> {
  const s = getState();
  const beforeHead = s.headCommit;
  let rollbackHead: string | null = null;
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
    const workingTreeRedirected = await detectEpochRedirect(s.repoDir);
    if (s.epochRedirected || workingTreeRedirected) {
      setState({ epochRedirected: true, syncStatus: "epoch_redirected" });
      return syncResult(beforeHead, localHead, "idle");
    }

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
      await detectEpochRedirect(s.repoDir);
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

    const { exists, readFile } = await import("./storage");
    const { extractThreadAdditions, resolveConflicts } = await import(
      "./conflict"
    );
    const localAdditions: Record<string, string> = {};
    const remoteContents: Record<string, string> = {};
    const localBoards: Record<string, string> = {};
    const quickSessionPaths = new Set<string>();
    const quickSessionChanges = new Map<string, QuickSessionChangedPaths>();

    for (const fp of changedFiles) {
      const quickFile = quickSessionFileFromPath(fp);
      if (!quickFile) {
        if (
          fp.startsWith("quick-sessions/") ||
          fp.startsWith("archive/quick-sessions/")
        ) {
          throw new Error(`Cannot auto-merge local browser sync change: ${fp}`);
        }
        continue;
      }
      quickSessionPaths.add(fp);
      const changed = quickSessionChanges.get(quickFile.id) ?? {
        id: quickFile.id,
      };
      if (quickFile.archived) {
        if (quickFile.file === "meta") changed.archivedMetaPath = fp;
        else changed.archivedThreadPath = fp;
      } else if (quickFile.file === "meta") {
        changed.activeMetaPath = fp;
      } else {
        changed.activeThreadPath = fp;
      }
      quickSessionChanges.set(quickFile.id, changed);
    }

    const quickSessionMerges: Array<{
      metaPath: string;
      threadPath: string;
      localMeta: string;
      baseMeta: string;
      remoteMeta: string;
      remoteThread: string;
    }> = [];
    const quickSessionCreates: Record<string, string> = {};
    const quickSessionMoves: QuickSessionMove[] = [];
    const quickSessionRelocations: QuickSessionRelocation[] = [];

    for (const changed of quickSessionChanges.values()) {
      const activeMetaPath = `quick-sessions/${changed.id}/session.meta.yaml`;
      const activeThreadPath = `quick-sessions/${changed.id}/discussion.thread`;
      const archivedMetaPath =
        `archive/quick-sessions/${changed.id}/session.meta.yaml`;
      const archivedThreadPath =
        `archive/quick-sessions/${changed.id}/discussion.thread`;
      const [
        baseActiveMeta,
        baseActiveThread,
        baseArchivedMeta,
        baseArchivedThread,
        remoteActiveMeta,
        remoteActiveThread,
        remoteArchivedMeta,
        remoteArchivedThread,
      ] = await Promise.all([
        gitOps.readFileAtCommit(s.repoDir, s.headCommit, activeMetaPath),
        gitOps.readFileAtCommit(s.repoDir, s.headCommit, activeThreadPath),
        gitOps.readFileAtCommit(s.repoDir, s.headCommit, archivedMetaPath),
        gitOps.readFileAtCommit(s.repoDir, s.headCommit, archivedThreadPath),
        gitOps.readFileAtCommit(s.repoDir, remoteHead, activeMetaPath),
        gitOps.readFileAtCommit(s.repoDir, remoteHead, activeThreadPath),
        gitOps.readFileAtCommit(s.repoDir, remoteHead, archivedMetaPath),
        gitOps.readFileAtCommit(s.repoDir, remoteHead, archivedThreadPath),
      ]);
      const localActiveState = quickSessionPairState(
        (await exists(`${s.repoDir}/${activeMetaPath}`))
          ? await readFile(`${s.repoDir}/${activeMetaPath}`)
          : null,
        (await exists(`${s.repoDir}/${activeThreadPath}`))
          ? await readFile(`${s.repoDir}/${activeThreadPath}`)
          : null,
        `Quick session ${changed.id} local active`,
      );
      const localArchivedState = quickSessionPairState(
        (await exists(`${s.repoDir}/${archivedMetaPath}`))
          ? await readFile(`${s.repoDir}/${archivedMetaPath}`)
          : null,
        (await exists(`${s.repoDir}/${archivedThreadPath}`))
          ? await readFile(`${s.repoDir}/${archivedThreadPath}`)
          : null,
        `Quick session ${changed.id} local archive`,
      );
      if (localActiveState === localArchivedState) {
        throw new Error(
          `Quick session ${changed.id} local canonical location is ambiguous`,
        );
      }
      const localArchived = localArchivedState === "present";
      const localMetaPath = localArchived ? archivedMetaPath : activeMetaPath;
      const localThreadPath = localArchived ? archivedThreadPath : activeThreadPath;
      const [localMeta, localThread] = await Promise.all([
        readFile(`${s.repoDir}/${localMetaPath}`),
        readFile(`${s.repoDir}/${localThreadPath}`),
      ]);

      const baseActiveState = quickSessionPairState(
        baseActiveMeta,
        baseActiveThread,
        `Quick session ${changed.id} baseline active`,
      );
      const baseArchivedState = quickSessionPairState(
        baseArchivedMeta,
        baseArchivedThread,
        `Quick session ${changed.id} baseline archive`,
      );
      if (baseActiveState === "absent" && baseArchivedState === "absent") {
        const remoteActiveState = quickSessionPairState(
          remoteActiveMeta,
          remoteActiveThread,
          `Quick session ${changed.id} remote active`,
        );
        const remoteArchivedState = quickSessionPairState(
          remoteArchivedMeta,
          remoteArchivedThread,
          `Quick session ${changed.id} remote archive`,
        );
        if (
          localArchived ||
          remoteActiveState === "present" ||
          remoteArchivedState === "present"
        ) {
          throw new Error(
            `Quick session ${changed.id} create conflict requires manual resolution`,
          );
        }
        quickSessionCreates[activeMetaPath] = localMeta;
        quickSessionCreates[activeThreadPath] = localThread;
        continue;
      }
      if (baseActiveState === baseArchivedState) {
        throw new Error(
          `Quick session ${changed.id} move baseline is ambiguous`,
        );
      }
      const baseArchived = baseArchivedState === "present";
      const baseMeta = baseArchived ? baseArchivedMeta : baseActiveMeta;
      const baseThread = baseArchived ? baseArchivedThread : baseActiveThread;
      if (baseMeta === null || baseThread === null) {
        throw new Error(
          `Quick session ${changed.id} baseline transaction is incomplete`,
        );
      }

      const remoteActiveState = quickSessionPairState(
        remoteActiveMeta,
        remoteActiveThread,
        `Quick session ${changed.id} remote active`,
      );
      const remoteArchivedState = quickSessionPairState(
        remoteArchivedMeta,
        remoteArchivedThread,
        `Quick session ${changed.id} remote archive`,
      );
      if (remoteActiveState === remoteArchivedState) {
        throw new Error(
          `Quick session ${changed.id} remote canonical location is ambiguous`,
        );
      }
      const remoteArchived = remoteArchivedState === "present";
      const remoteMeta = remoteArchived ? remoteArchivedMeta : remoteActiveMeta;
      const remoteThread = remoteArchived
        ? remoteArchivedThread
        : remoteActiveThread;
      if (remoteMeta === null || remoteThread === null) {
        throw new Error(
          `Quick session ${changed.id} remote transaction is incomplete`,
        );
      }
      const parsedLocalMeta = parseQuickSessionMeta(localMeta) as {
        status: string;
      };
      if ((parsedLocalMeta.status === "archived") !== localArchived) {
        throw new Error(
          `Quick session ${changed.id} local metadata does not match its canonical location`,
        );
      }

      // Preserve the exact local move when the remote source is untouched.
      // This keeps ordinary archive/unarchive retries byte-stable. Any
      // cross-node mutation falls through to the shared WASM merge below.
      if (
        localArchived !== baseArchived &&
        remoteArchived === baseArchived &&
        remoteMeta === baseMeta &&
        remoteThread === baseThread
      ) {
        const sourceMetaPath = baseArchived ? archivedMetaPath : activeMetaPath;
        const sourceThreadPath = baseArchived
          ? archivedThreadPath
          : activeThreadPath;
        quickSessionMoves.push({
          targetMetaPath: localMetaPath,
          targetThreadPath: localThreadPath,
          sourceMetaPath,
          sourceThreadPath,
          targetMeta: localMeta,
          targetThread: localThread,
          commitMessage:
            `session: ${localArchived ? "archive" : "unarchive"} ${changed.id} by @${s.me.handler}`,
        });
        continue;
      }

      if (!remoteThread.startsWith(baseThread)) {
        throw new Error(
          `Quick session ${changed.id} remote thread changed outside append-only shape`,
        );
      }
      const archiveWins = localArchived || remoteArchived;
      const targetMetaPath = archiveWins ? archivedMetaPath : activeMetaPath;
      const targetThreadPath = archiveWins
        ? archivedThreadPath
        : activeThreadPath;
      const additions = extractThreadAdditions(
        targetThreadPath,
        localThread,
        baseThread,
      );
      if (additions.trim()) localAdditions[targetThreadPath] = additions;
      remoteContents[targetThreadPath] = remoteThread;
      quickSessionMerges.push({
        metaPath: targetMetaPath,
        threadPath: targetThreadPath,
        localMeta,
        baseMeta,
        remoteMeta,
        remoteThread,
      });
      if (archiveWins !== remoteArchived) {
        quickSessionRelocations.push({
          targetMetaPath,
          targetThreadPath,
          sourceMetaPath: remoteArchived ? archivedMetaPath : activeMetaPath,
          sourceThreadPath: remoteArchived
            ? archivedThreadPath
            : activeThreadPath,
        });
      }
    }

    for (const fp of changedFiles) {
      if (quickSessionPaths.has(fp)) continue;
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

    // Resolve every content and metadata conflict before the destructive
    // reset. Quick Session metadata uses the same Rust merge function as the
    // native daemon, including title/claim/completion conflict rejection and
    // line-number translation.
    const resolved = resolveConflicts(localAdditions, remoteContents);
    const quickSessionMetas: Record<string, string> = {};
    const quickSessionThreads: Record<string, string> = {};
    for (const quick of quickSessionMerges) {
      const mergedThread = resolved.files[quick.threadPath] ?? quick.remoteThread;
      if (resolved.files[quick.threadPath] === undefined) {
        quickSessionThreads[quick.threadPath] = mergedThread;
      }
      const mappings = resolved.mappings.filter(
        (mapping) => mapping.file === quick.threadPath,
      );
      const localLinesUnchanged = mappings.every(
        (mapping) => mapping.old_line === mapping.new_line,
      );
      if (quick.remoteMeta === quick.baseMeta && localLinesUnchanged) {
        quickSessionMetas[quick.metaPath] = quick.localMeta;
        continue;
      }
      const mergedMeta = mergeQuickSessionMeta(
        parseQuickSessionMeta(quick.localMeta),
        parseQuickSessionMeta(quick.remoteMeta),
        mergedThread,
        mappings,
        quick.threadPath,
      );
      quickSessionMetas[quick.metaPath] = serializeQuickSessionMeta(mergedMeta);
    }

    // From this point until the replay commit succeeds, every failure must
    // restore the branch and working tree that still owns the local changes.
    rollbackHead = localHead;
    await gitOps.resetToRemote(
      s.repoDir,
      `refs/remotes/origin/${s.defaultBranch}`,
    );

    // Write resolved files back
    const { writeFile, mkdir, removeDir, removeFile } = await import("./storage");
    const filePaths: string[] = [];
    for (const [fp, content] of Object.entries(resolved.files)) {
      const absPath = `${s.repoDir}/${fp}`;
      await mkdirp(parentPath(absPath), exists, mkdir);
      await writeFile(absPath, content);
      filePaths.push(fp);
    }
    for (const [fp, content] of Object.entries(quickSessionThreads)) {
      const absPath = `${s.repoDir}/${fp}`;
      await mkdirp(parentPath(absPath), exists, mkdir);
      await writeFile(absPath, content);
      filePaths.push(fp);
    }
    for (const [fp, content] of Object.entries(quickSessionMetas)) {
      const absPath = `${s.repoDir}/${fp}`;
      await mkdirp(parentPath(absPath), exists, mkdir);
      await writeFile(absPath, content);
      filePaths.push(fp);
    }
    for (const [fp, content] of Object.entries(quickSessionCreates)) {
      const absPath = `${s.repoDir}/${fp}`;
      await mkdirp(parentPath(absPath), exists, mkdir);
      await writeFile(absPath, content);
      filePaths.push(fp);
    }
    for (const move of quickSessionMoves) {
      for (const [fp, content] of [
        [move.targetMetaPath, move.targetMeta],
        [move.targetThreadPath, move.targetThread],
      ] as const) {
        const absPath = `${s.repoDir}/${fp}`;
        await mkdirp(parentPath(absPath), exists, mkdir);
        await writeFile(absPath, content);
      }
      await removeFile(`${s.repoDir}/${move.sourceMetaPath}`);
      await removeFile(`${s.repoDir}/${move.sourceThreadPath}`);
      await removeDir(parentPath(`${s.repoDir}/${move.sourceMetaPath}`));
    }
    for (const relocation of quickSessionRelocations) {
      await removeFile(`${s.repoDir}/${relocation.sourceMetaPath}`);
      await removeFile(`${s.repoDir}/${relocation.sourceThreadPath}`);
      await removeDir(parentPath(`${s.repoDir}/${relocation.sourceMetaPath}`));
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
    const hasQuickSessionFiles =
      Object.keys(quickSessionMetas).length > 0 ||
      Object.keys(quickSessionCreates).length > 0;
    const commitMessage =
      hasBoardFiles && !hasThreadFiles && !hasQuickSessionFiles
        ? "board: sync after rebase"
        : hasQuickSessionFiles && !hasThreadFiles
          ? "session: sync after rebase"
          : resolved.commitMessage;
    if (quickSessionMoves.length > 0 || quickSessionRelocations.length > 0) {
      const moveAddPaths = [
        ...quickSessionMoves.flatMap((move) => [
          move.targetMetaPath,
          move.targetThreadPath,
        ]),
        ...quickSessionRelocations.flatMap((move) => [
          move.targetMetaPath,
          move.targetThreadPath,
        ]),
      ];
      const moveRemovePaths = [
        ...quickSessionMoves.flatMap((move) => [
          move.sourceMetaPath,
          move.sourceThreadPath,
        ]),
        ...quickSessionRelocations.flatMap((move) => [
          move.sourceMetaPath,
          move.sourceThreadPath,
        ]),
      ];
      await gitOps.addRemoveAndCommit(
        s.repoDir,
        [...filePaths, ...moveAddPaths],
        moveRemovePaths,
        quickSessionMoves.length > 0 && quickSessionRelocations.length === 0
          ? quickSessionMoves.map((move) => move.commitMessage).join("\n")
          : "session: sync after rebase",
        s.me.handler,
      );
    } else {
      await gitOps.addAndCommit(
        s.repoDir,
        filePaths,
        commitMessage,
        s.me.handler,
      );
    }
    rollbackHead = null;

    // Epoch fence (invariant 1): the reset just materialized remote state
    // in the working tree — if that includes a redirected gitim.epoch.yaml,
    // this branch is sealed. The resolve-commit above keeps the user's
    // messages safe locally; we just never publish them here. A Rust daemon
    // (or a future browser follow) migrates them onto the new epoch.
    if (await detectEpochRedirect(s.repoDir)) {
      const fencedHead = await gitOps.resolveHead(s.repoDir);
      return syncResult(beforeHead, fencedHead, "rebased");
    }

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
    let reportedError = e;
    if (rollbackHead !== null) {
      try {
        await gitOps.resetToCommit(s.repoDir, rollbackHead);
      } catch (restoreError) {
        reportedError = new Error(
          `${errorMessage(e)}; failed to restore local sync state: ${errorMessage(restoreError)}`,
          { cause: e },
        );
      }
    }
    const message = errorMessage(reportedError);
    if (isAuthFailure(e)) {
      setState({ token: null, syncStatus: "reconnect_required" });
      postReconnectRequired(getState().headCommit, message);
    } else {
      setState({ syncStatus: "error" });
      postMessage({ type: "sync_error", error: message });
    }
    console.error("[daemon-web] sync error:", reportedError);
    throw reportedError;
  }
}

async function runSync(options: RunSyncOptions = {}): Promise<SyncResult> {
  if (syncInFlight && !options.forceNewCycle) return syncInFlight;

  const previous = syncInFlight;
  const next = (async () => {
    // Conflict resolution and the board-path handler validator reach wasm;
    // this loop can fire on a timer independent of any handler, so gate here.
    await ensureWasmReady();
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
