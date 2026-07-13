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

    for (const changed of quickSessionChanges.values()) {
      const hasArchivedPath =
        changed.archivedMetaPath !== undefined ||
        changed.archivedThreadPath !== undefined;
      if (hasArchivedPath) {
        if (
          !changed.activeMetaPath ||
          !changed.activeThreadPath ||
          !changed.archivedMetaPath ||
          !changed.archivedThreadPath
        ) {
          throw new Error(
            `Quick session ${changed.id} move transaction is incomplete`,
          );
        }

        const activeMetaPath = changed.activeMetaPath;
        const activeThreadPath = changed.activeThreadPath;
        const archivedMetaPath = changed.archivedMetaPath;
        const archivedThreadPath = changed.archivedThreadPath;
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

        const baseActiveState = quickSessionPairState(
          baseActiveMeta,
          baseActiveThread,
          `Quick session ${changed.id} baseline active source`,
        );
        const baseArchivedState = quickSessionPairState(
          baseArchivedMeta,
          baseArchivedThread,
          `Quick session ${changed.id} baseline archived source`,
        );
        const archive =
          baseActiveState === "present" && baseArchivedState === "absent";
        const unarchive =
          baseActiveState === "absent" && baseArchivedState === "present";
        if (!archive && !unarchive) {
          throw new Error(
            `Quick session ${changed.id} move baseline is ambiguous`,
          );
        }

        const sourceMetaPath = archive ? activeMetaPath : archivedMetaPath;
        const sourceThreadPath = archive ? activeThreadPath : archivedThreadPath;
        const targetMetaPath = archive ? archivedMetaPath : activeMetaPath;
        const targetThreadPath = archive ? archivedThreadPath : activeThreadPath;
        const baseSourceMeta = archive ? baseActiveMeta : baseArchivedMeta;
        const baseSourceThread = archive ? baseActiveThread : baseArchivedThread;
        const remoteSourceMeta = archive ? remoteActiveMeta : remoteArchivedMeta;
        const remoteSourceThread = archive
          ? remoteActiveThread
          : remoteArchivedThread;
        const remoteTargetMeta = archive ? remoteArchivedMeta : remoteActiveMeta;
        const remoteTargetThread = archive
          ? remoteArchivedThread
          : remoteActiveThread;

        if (
          remoteSourceMeta !== baseSourceMeta ||
          remoteSourceThread !== baseSourceThread
        ) {
          throw new Error(
            `Quick session ${changed.id} remote source changed or was deleted`,
          );
        }
        if (remoteTargetMeta !== null || remoteTargetThread !== null) {
          throw new Error(
            `Quick session ${changed.id} remote target already exists`,
          );
        }

        const sourceMetaAbs = `${s.repoDir}/${sourceMetaPath}`;
        const sourceThreadAbs = `${s.repoDir}/${sourceThreadPath}`;
        const targetMetaAbs = `${s.repoDir}/${targetMetaPath}`;
        const targetThreadAbs = `${s.repoDir}/${targetThreadPath}`;
        if (
          (await exists(sourceMetaAbs)) ||
          (await exists(sourceThreadAbs)) ||
          !(await exists(targetMetaAbs)) ||
          !(await exists(targetThreadAbs))
        ) {
          throw new Error(
            `Quick session ${changed.id} local move transaction is incomplete`,
          );
        }
        const [targetMeta, targetThread] = await Promise.all([
          readFile(targetMetaAbs),
          readFile(targetThreadAbs),
        ]);
        const parsedTargetMeta = parseQuickSessionMeta(targetMeta) as {
          status: string;
        };
        if (
          (archive && parsedTargetMeta.status !== "archived") ||
          (unarchive && parsedTargetMeta.status === "archived")
        ) {
          throw new Error(
            `Quick session ${changed.id} local move metadata is invalid`,
          );
        }
        quickSessionMoves.push({
          targetMetaPath,
          targetThreadPath,
          sourceMetaPath,
          sourceThreadPath,
          targetMeta,
          targetThread,
          commitMessage:
            `session: ${archive ? "archive" : "unarchive"} ${changed.id} by @${s.me.handler}`,
        });
        continue;
      }

      if (!changed.activeMetaPath) {
        throw new Error(
          `Quick session ${changed.id} changed without session.meta.yaml`,
        );
      }
      const threadPath =
        changed.activeThreadPath ??
        `quick-sessions/${changed.id}/discussion.thread`;
      const [localMeta, baseMeta, remoteMeta, baseThread, remoteThread] =
        await Promise.all([
          readFile(`${s.repoDir}/${changed.activeMetaPath}`),
          gitOps.readFileAtCommit(
            s.repoDir,
            s.headCommit,
            changed.activeMetaPath,
          ),
          gitOps.readFileAtCommit(
            s.repoDir,
            remoteHead,
            changed.activeMetaPath,
          ),
          gitOps.readFileAtCommit(s.repoDir, s.headCommit, threadPath),
          gitOps.readFileAtCommit(s.repoDir, remoteHead, threadPath),
        ]);

      if (baseMeta === null) {
        if (!changed.activeThreadPath) {
          throw new Error(
            `Quick session ${changed.id} create is missing discussion.thread`,
          );
        }
        if (remoteMeta !== null || remoteThread !== null || baseThread !== null) {
          throw new Error(
            `Quick session ${changed.id} create conflict requires manual resolution`,
          );
        }
        quickSessionCreates[changed.activeMetaPath] = localMeta;
        quickSessionCreates[threadPath] = await readFile(
          `${s.repoDir}/${threadPath}`,
        );
        continue;
      }

      if (remoteMeta === null || remoteThread === null || baseThread === null) {
        throw new Error(
          `Quick session ${changed.id} remote transaction is incomplete`,
        );
      }
      if (!remoteThread.startsWith(baseThread)) {
        throw new Error(
          `Quick session ${changed.id} remote thread changed outside append-only shape`,
        );
      }
      if (changed.activeThreadPath) {
        const localThread = await readFile(`${s.repoDir}/${threadPath}`);
        const additions = extractThreadAdditions(
          threadPath,
          localThread,
          baseThread,
        );
        if (additions.trim()) localAdditions[threadPath] = additions;
      }
      remoteContents[threadPath] = remoteThread;
      quickSessionMerges.push({
        metaPath: changed.activeMetaPath,
        threadPath,
        localMeta,
        baseMeta,
        remoteMeta,
        remoteThread,
      });
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
    for (const quick of quickSessionMerges) {
      const mergedThread = resolved.files[quick.threadPath] ?? quick.remoteThread;
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

    // Reset working tree to remote HEAD
    await gitOps.resetToRemote(
      s.repoDir,
      `refs/remotes/origin/${s.defaultBranch}`,
    );

    // Write resolved files back
    const { writeFile, mkdir, removeDir, removeFile } = await import("./storage");
    const filePaths: string[] = [];
    for (const [fp, content] of Object.entries(resolved.files)) {
      await writeFile(`${s.repoDir}/${fp}`, content);
      filePaths.push(fp);
    }
    for (const [fp, content] of Object.entries(quickSessionMetas)) {
      await writeFile(`${s.repoDir}/${fp}`, content);
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
    if (quickSessionMoves.length > 0) {
      const moveAddPaths = quickSessionMoves.flatMap((move) => [
        move.targetMetaPath,
        move.targetThreadPath,
      ]);
      const moveRemovePaths = quickSessionMoves.flatMap((move) => [
        move.sourceMetaPath,
        move.sourceThreadPath,
      ]);
      await gitOps.addRemoveAndCommit(
        s.repoDir,
        [...filePaths, ...moveAddPaths],
        moveRemovePaths,
        quickSessionMoves.map((move) => move.commitMessage).join("\n"),
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
