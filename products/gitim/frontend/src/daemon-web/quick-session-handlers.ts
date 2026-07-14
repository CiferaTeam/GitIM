import * as gitOps from "./git";
import {
  exists,
  mkdir,
  readFile,
  readdir,
  removeDir,
  removeFile,
  writeFile,
} from "./storage";
import { getState, setState } from "./state";
import { parseThread, type ThreadEntry } from "./parser";
import { formatMessage } from "./formatter";
import { runSync } from "./sync";
import { isAuthFailure } from "./auth-errors";
import {
  applyQuickSessionTransition,
  parseQuickSessionMeta,
  serializeQuickSessionMeta,
  validateAppend,
  validateQuickSessionId,
} from "gitim-wasm";
import { ensureWasmReady } from "./wasm-ready";
import { validateHandler } from "./paths";
import { withRepoLock } from "./repo-lock";

export type ApiResponse = {
  ok: boolean;
  data?: Record<string, unknown>;
  error?: string;
  error_code?: string;
};

export interface CreateQuickSessionInput {
  session_id: string;
  agent_id: string;
  first_message: string;
}

export interface QuickSessionListQuery {
  archived?: boolean;
  agent_id?: string;
  actionable?: boolean;
  limit?: number;
}

export interface ReadQuickSessionQuery {
  limit?: number;
  since?: number;
}

export interface SendQuickSessionInput {
  body: string;
  request_id: string;
}

type QuickSessionStatus =
  | "needs_title"
  | "running"
  | "active"
  | "error"
  | "archived";

interface QuickSessionMeta {
  id: string;
  title?: string;
  title_source: "none" | "api_set";
  agent_id: string;
  created_by: string;
  status: QuickSessionStatus;
  created_at: string;
  updated_at: string;
  archived_at?: string;
  archived_from?: QuickSessionStatus;
  summary?: string;
  summary_updated_at?: string;
  last_message_preview: string;
  error?: string;
  processing_input_line?: number;
  processing_started_at?: string;
  attempt_id?: string;
  last_completed_attempt_id?: string;
  last_completed_input_line?: number;
  last_completed_line?: number;
  last_failed_attempt_id?: string;
  last_human_request_id?: string;
  last_human_line?: number;
  revision: number;
}

interface QuickSessionTransitionResult {
  meta: QuickSessionMeta;
  outcome: { kind: "applied" } | { kind: "duplicate"; line_number?: number };
}

interface LocatedQuickSession {
  relDir: string;
  absDir: string;
  archived: boolean;
}

interface QuickSessionMutation {
  data: Record<string, unknown>;
  committed: boolean;
}

export interface ClassifiedQuickSessionPollChange {
  channel: string;
  kind: "quick_session_meta" | "quick_session_thread";
  entries: Array<Record<string, unknown> | ThreadEntry>;
}

function ok(data: Record<string, unknown> = {}): ApiResponse {
  return { ok: true, data };
}

function err(error: string): ApiResponse {
  return { ok: false, error };
}

function errCode(error: string, error_code: string): ApiResponse & { error_code: string } {
  return { ok: false, error, error_code };
}

function assertNotRedirected(): ApiResponse | null {
  if (!getState().epochRedirected) return null;
  return errCode(
    "This workspace has rotated to a new epoch branch. Reload the page to reconnect.",
    "epoch_redirected",
  );
}

function reconnectRequired(): ApiResponse & { error_code: string } {
  return errCode(
    "Reconnect token to send from this browser workspace.",
    "reconnect_required",
  );
}

function errorMessage(error: unknown): string {
  return String((error as Error).message ?? error);
}

async function syncAfterCommit(): Promise<{
  status: "pushed" | "commit_only";
  commit_id?: string;
  error?: string;
  error_code?: string;
  needs_token?: boolean;
}> {
  try {
    const result = await runSync({ forceNewCycle: true });
    const state = getState();
    if (state.epochRedirected) {
      return {
        status: "commit_only",
        error: "Workspace epoch is redirected; the local commit was not published.",
        error_code: "epoch_redirected",
      };
    }
    if (result.status === "reconnect_required") {
      return {
        status: "commit_only",
        error: "Reconnect token to publish the local commit.",
        error_code: "reconnect_required",
        needs_token: true,
      };
    }
    const published =
      result.status === "pushed" ||
      result.status === "rebased" ||
      (result.status === "idle" && result.changed);
    return published
      ? { status: "pushed", commit_id: result.afterHead }
      : { status: "commit_only" };
  } catch (error) {
    if (isAuthFailure(error)) {
      setState({ token: null, syncStatus: "reconnect_required" });
      return {
        status: "commit_only",
        error: errorMessage(error),
        error_code: "reconnect_required",
        needs_token: true,
      };
    }
    return { status: "commit_only", error: errorMessage(error) };
  }
}

// --- Quick Session handlers ---

export async function createQuickSession(
  input: CreateQuickSessionInput,
): Promise<ApiResponse> {
  try {
    const mutation = await withRepoLock(async (): Promise<QuickSessionMutation | ApiResponse> => {
      const writeError = await quickSessionWriteError();
      if (writeError) return writeError;
      const s = getState();
      const idError = quickSessionIdError(input.session_id);
      if (idError) return idError;
      const bodyError = validateQuickSessionBody(input.first_message);
      if (bodyError) return bodyError;
      const creatorError = await ensureActiveQuickSessionHandler(s.me.handler, "creator");
      if (creatorError) return creatorError;
      const agentError = await ensureActiveQuickSessionHandler(input.agent_id, "agent");
      if (agentError) return agentError;

      const creator = s.me.handler;
      const body = canonicalQuickSessionBody(creator, input.first_message);
      const existing = await locateQuickSession(input.session_id);
      if (existing) {
        const detail = await quickSessionDetail(existing);
        const first = detail.entries[0];
        const matches =
          detail.meta.id === input.session_id &&
          detail.meta.agent_id === input.agent_id &&
          detail.meta.created_by === creator &&
          first?.type === "message" &&
          first.line_number === 1 &&
          first.point_to === 0 &&
          first.author === creator &&
          first.body === body;
        if (!matches) {
          return errCode(
            "quick session id collides with a different object",
            "quick_session_id_collision",
          );
        }
        return {
          committed: false,
          data: {
            session: detail,
            line_number: 1,
            ref: `session:${input.session_id}`,
          },
        };
      }

      const now = utcTimestamp();
      const initial: QuickSessionMeta = {
        id: input.session_id,
        title_source: "none",
        agent_id: input.agent_id,
        created_by: creator,
        status: "needs_title",
        created_at: now,
        updated_at: now,
        last_message_preview: "",
        revision: 1,
      };
      const transitioned = applyQuickSessionTransition(
        initial,
        {
          kind: "human_message",
          actor: creator,
          line_number: 1,
          preview: body,
          now,
        },
      ) as QuickSessionTransitionResult;
      const thread = formatMessage(1, 0, creator, now, body);
      const activeHandlers = await quickSessionActiveHandlers();
      validateAppend("", thread, activeHandlers, [creator, input.agent_id]);

      const relDir = quickSessionRelDir(input.session_id, false);
      const absDir = `${s.repoDir}/${relDir}`;
      const metaRel = `${relDir}/session.meta.yaml`;
      const threadRel = `${relDir}/discussion.thread`;
      const transactionPaths = [metaRel, threadRel];
      await mkdirp(absDir);
      try {
        await writeFile(
          `${absDir}/session.meta.yaml`,
          serializeQuickSessionMeta(transitioned.meta),
        );
        await writeFile(`${absDir}/discussion.thread`, thread);
        await gitOps.addAndCommit(
          s.repoDir,
          transactionPaths,
          `session: create ${input.session_id} for @${input.agent_id} by @${creator}`,
          creator,
        );
      } catch (error) {
        await rollbackQuickSessionMutation(
          s.repoDir,
          transactionPaths,
          async () => removeQuickSessionDirectoryIfExists(absDir),
          error,
        );
      }

      return {
        committed: true,
        data: {
          session: {
            meta: transitioned.meta,
            entries: parseThread(thread).entries,
            archived: false,
          },
          line_number: 1,
          ref: `session:${input.session_id}`,
        },
      };
    });
    if ("ok" in mutation) return mutation;
    if (!mutation.committed) return ok(mutation.data);
    return ok({ ...mutation.data, ...(await syncAfterCommit()) });
  } catch (error) {
    return quickSessionErrorResponse(error);
  }
}

export async function listQuickSessions(
  query: QuickSessionListQuery = {},
): Promise<ApiResponse> {
  try {
    await ensureWasmReady();
    if (query.agent_id) {
      const invalid = validateHandler(query.agent_id);
      if (invalid) return err(`invalid agent: ${invalid}`);
    }
    return await withRepoLock(() => listQuickSessionsSnapshot(query));
  } catch (error) {
    return quickSessionErrorResponse(error);
  }
}

async function listQuickSessionsSnapshot(
  query: QuickSessionListQuery,
): Promise<ApiResponse> {
  const s = getState();
  const archived = query.archived ?? false;
  const root = `${s.repoDir}/${archived ? "archive/quick-sessions" : "quick-sessions"}`;
  const sessions: Array<Record<string, unknown>> = [];
  if (await exists(root)) {
    for (const id of await readdir(root)) {
      if (quickSessionIdError(id)) continue;
      const located: LocatedQuickSession = {
        relDir: quickSessionRelDir(id, archived),
        absDir: `${root}/${id}`,
        archived,
      };
      if (!(await exists(`${located.absDir}/session.meta.yaml`))) continue;
      try {
        const detail = await quickSessionDetail(located);
        const meta = detail.meta;
        if (query.agent_id && meta.agent_id !== query.agent_id) continue;
        if (query.actionable) {
          if (meta.status !== "needs_title" && meta.status !== "active") continue;
          const newestCreatorLine = detail.entries.reduce(
            (newest, entry) =>
              entry.type === "message" && entry.author === meta.created_by
                ? Math.max(newest, entry.line_number)
                : newest,
            0,
          );
          if (
            newestCreatorLine === 0 ||
            (meta.last_completed_input_line !== undefined &&
              newestCreatorLine <= meta.last_completed_input_line)
          ) {
            continue;
          }
        }
        sessions.push(quickSessionListItem(meta, archived));
      } catch {
        continue;
      }
    }
  }
  sessions.sort((left, right) => {
    const updated = String(right.updated_at).localeCompare(String(left.updated_at));
    return updated || String(left.id).localeCompare(String(right.id));
  });
  const limit = Math.min(100, Math.max(1, query.limit ?? 100));
  return ok({ sessions: sessions.slice(0, limit) });
}

export async function readQuickSession(
  sessionId: string,
  query: ReadQuickSessionQuery = {},
): Promise<ApiResponse> {
  try {
    await ensureWasmReady();
    const idError = quickSessionIdError(sessionId);
    if (idError) return idError;
    return await withRepoLock(async () => {
      const located = await locateQuickSession(sessionId);
      if (!located) return err("quick session not found");
      return ok({ session: await quickSessionDetail(located, query) });
    });
  } catch (error) {
    return quickSessionErrorResponse(error);
  }
}

export async function sendQuickSessionMessage(
  sessionId: string,
  input: SendQuickSessionInput,
): Promise<ApiResponse> {
  try {
    const mutation = await withRepoLock(async (): Promise<QuickSessionMutation | ApiResponse> => {
      const writeError = await quickSessionWriteError();
      if (writeError) return writeError;
      const idError = quickSessionIdError(sessionId);
      if (idError) return idError;
      const bodyError = validateQuickSessionBody(input.body);
      if (bodyError) return bodyError;
      if (!input.request_id?.trim()) {
        return errCode(
          "creator messages require request_id",
          "invalid_quick_session_message",
        );
      }

      const s = getState();
      const creatorError = await ensureActiveQuickSessionHandler(s.me.handler, "author");
      if (creatorError) return creatorError;
      const located = await locateQuickSession(sessionId);
      if (!located) return err("quick session not found");
      if (located.archived) {
        return errCode(
          "quick session transition is not valid from the current state",
          "quick_session_invalid_state",
        );
      }
      const meta = await loadQuickSessionMeta(located);
      if (meta.created_by !== s.me.handler) {
        return errCode(
          "quick session actor is not authorized for this transition",
          "quick_session_forbidden",
        );
      }
      const oldThread = await readFile(`${located.absDir}/discussion.thread`);
      const entries = parseThread(oldThread).entries;
      const nextLine = (entries.at(-1)?.line_number ?? 0) + 1;
      const now = utcTimestamp();
      const transitioned = applyQuickSessionTransition(
        meta,
        {
          kind: "human_message",
          actor: s.me.handler,
          line_number: nextLine,
          request_id: input.request_id,
          preview: input.body,
          now,
        },
      ) as QuickSessionTransitionResult;
      if (transitioned.outcome.kind === "duplicate") {
        return {
          committed: false,
          data: quickSessionSendResponse(
            transitioned.meta,
            transitioned.outcome.line_number ?? nextLine,
          ),
        };
      }

      const line = formatMessage(nextLine, 0, s.me.handler, now, input.body);
      validateAppend(
        oldThread,
        line,
        await quickSessionActiveHandlers(),
        [meta.created_by, meta.agent_id],
      );
      const newThread = `${oldThread}${line}`;
      const metaPath = `${located.absDir}/session.meta.yaml`;
      const threadPath = `${located.absDir}/discussion.thread`;
      const transactionPaths = [
        `${located.relDir}/session.meta.yaml`,
        `${located.relDir}/discussion.thread`,
      ];
      const oldMeta = await readFile(metaPath);
      try {
        await writeFile(metaPath, serializeQuickSessionMeta(transitioned.meta));
        await writeFile(threadPath, newThread);
        await gitOps.addAndCommit(
          s.repoDir,
          transactionPaths,
          `session-msg: @${s.me.handler} -> ${sessionId} L${String(nextLine).padStart(6, "0")}`,
          s.me.handler,
        );
      } catch (error) {
        await rollbackQuickSessionMutation(
          s.repoDir,
          transactionPaths,
          async () => {
            await writeFile(metaPath, oldMeta);
            await writeFile(threadPath, oldThread);
          },
          error,
        );
      }
      return {
        committed: true,
        data: quickSessionSendResponse(transitioned.meta, nextLine),
      };
    });
    if ("ok" in mutation) return mutation;
    if (!mutation.committed) return ok(mutation.data);
    return ok({
      ...mutation.data,
      ...quickSessionSyncFields(await syncAfterCommit()),
    });
  } catch (error) {
    return quickSessionErrorResponse(error);
  }
}

export async function archiveQuickSession(sessionId: string): Promise<ApiResponse> {
  return moveQuickSession(sessionId, true);
}

export async function unarchiveQuickSession(sessionId: string): Promise<ApiResponse> {
  return moveQuickSession(sessionId, false);
}

async function moveQuickSession(sessionId: string, archive: boolean): Promise<ApiResponse> {
  try {
    const mutation = await withRepoLock(async (): Promise<QuickSessionMutation | ApiResponse> => {
      const writeError = await quickSessionWriteError();
      if (writeError) return writeError;
      const idError = quickSessionIdError(sessionId);
      if (idError) return idError;
      const s = getState();
      const creatorError = await ensureActiveQuickSessionHandler(s.me.handler, "creator");
      if (creatorError) return creatorError;
      const located = await locateQuickSession(sessionId);
      if (!located) return err("quick session not found");
      if (located.archived === archive) {
        return errCode(
          "quick session transition is not valid from the current state",
          "quick_session_invalid_state",
        );
      }
      const meta = await loadQuickSessionMeta(located);
      const now = utcTimestamp();
      const transitioned = applyQuickSessionTransition(
        meta,
        {
          kind: archive ? "archive" : "unarchive",
          actor: s.me.handler,
          now,
        },
      ) as QuickSessionTransitionResult;
      const targetRel = quickSessionRelDir(sessionId, archive);
      const targetAbs = `${s.repoDir}/${targetRel}`;
      if (await exists(`${targetAbs}/session.meta.yaml`)) {
        return err("quick session target already exists");
      }
      const sourceMetaPath = `${located.absDir}/session.meta.yaml`;
      const sourceThreadPath = `${located.absDir}/discussion.thread`;
      const oldMeta = await readFile(sourceMetaPath);
      const thread = await readFile(sourceThreadPath);
      const addPaths = [
        `${targetRel}/session.meta.yaml`,
        `${targetRel}/discussion.thread`,
      ];
      const removePaths = [
        `${located.relDir}/session.meta.yaml`,
        `${located.relDir}/discussion.thread`,
      ];
      const transactionPaths = [...addPaths, ...removePaths];
      await mkdirp(targetAbs);
      try {
        await writeFile(
          `${targetAbs}/session.meta.yaml`,
          serializeQuickSessionMeta(transitioned.meta),
        );
        await writeFile(`${targetAbs}/discussion.thread`, thread);
        await removeQuickSessionDirectoryRequired(located.absDir);
        await gitOps.addRemoveAndCommit(
          s.repoDir,
          addPaths,
          removePaths,
          `session: ${archive ? "archive" : "unarchive"} ${sessionId} by @${s.me.handler}`,
          s.me.handler,
        );
      } catch (error) {
        await rollbackQuickSessionMutation(
          s.repoDir,
          transactionPaths,
          async () => {
            await mkdirp(located.absDir);
            await writeFile(sourceMetaPath, oldMeta);
            await writeFile(sourceThreadPath, thread);
            await removeQuickSessionDirectoryIfExists(targetAbs);
          },
          error,
        );
      }
      return {
        committed: true,
        data: archive
          ? {
              session_id: sessionId,
              status: transitioned.meta.status,
              revision: transitioned.meta.revision,
              archived_at: transitioned.meta.archived_at ?? "",
            }
          : {
              session_id: sessionId,
              status: transitioned.meta.status,
              revision: transitioned.meta.revision,
            },
      };
    });
    if ("ok" in mutation) return mutation;
    return ok({
      ...mutation.data,
      ...quickSessionSyncFields(await syncAfterCommit()),
    });
  } catch (error) {
    return quickSessionErrorResponse(error);
  }
}


async function quickSessionWriteError(): Promise<ApiResponse | null> {
  await ensureWasmReady();
  const fenced = assertNotRedirected();
  if (fenced) return fenced;
  return getState().token ? null : reconnectRequired();
}

function quickSessionIdError(sessionId: string): ApiResponse | null {
  try {
    validateQuickSessionId(sessionId);
    return null;
  } catch (error) {
    return errCode(errorMessage(error), "invalid_quick_session_id");
  }
}

function validateQuickSessionBody(body: string): ApiResponse | null {
  if (body.trim().length === 0) {
    return errCode(
      "quick session message cannot be empty",
      "invalid_quick_session_message",
    );
  }
  if (new TextEncoder().encode(body).length > 64 * 1024) {
    return errCode(
      "quick session message exceeds 64 KB",
      "invalid_quick_session_message",
    );
  }
  return null;
}

function quickSessionErrorResponse(error: unknown): ApiResponse {
  const message = errorMessage(error);
  if (message.includes("not authorized")) {
    return errCode(message, "quick_session_forbidden");
  }
  if (
    message.includes("not valid from the current state") ||
    message.includes("not newer than the completed input") ||
    message.includes("must target the claimed input line") ||
    message.includes("line number is invalid")
  ) {
    return errCode(message, "quick_session_invalid_state");
  }
  if (message.includes("invalid quick session id")) {
    return errCode(message, "invalid_quick_session_id");
  }
  return err(message);
}

async function ensureActiveQuickSessionHandler(
  handler: string,
  role: string,
): Promise<ApiResponse | null> {
  const invalid = validateHandler(handler);
  if (invalid) return err(`invalid ${role}: ${invalid}`);
  const repoDir = getState().repoDir;
  if (await exists(`${repoDir}/archive/users/${handler}.meta.yaml`)) {
    return err(`${role} @${handler} is departed`);
  }
  if (!(await exists(`${repoDir}/users/${handler}.meta.yaml`))) {
    return err(`unknown ${role}: ${handler}`);
  }
  return null;
}

async function quickSessionActiveHandlers(): Promise<string[]> {
  const root = `${getState().repoDir}/users`;
  if (!(await exists(root))) return [];
  return (await readdir(root))
    .filter((name) => name.endsWith(".meta.yaml"))
    .map((name) => name.slice(0, -".meta.yaml".length))
    .filter((handler) => validateHandler(handler) === null)
    .sort();
}

function canonicalQuickSessionBody(author: string, body: string): string {
  const normalized = parseThread(
    formatMessage(1, 0, author, "19700101T000000Z", body),
  ).entries[0];
  if (normalized?.type !== "message") {
    throw new Error("failed to normalize message");
  }
  return normalized.body;
}

function quickSessionRelDir(sessionId: string, archived: boolean): string {
  validateQuickSessionId(sessionId);
  return `${archived ? "archive/quick-sessions" : "quick-sessions"}/${sessionId}`;
}

async function locateQuickSession(
  sessionId: string,
): Promise<LocatedQuickSession | null> {
  validateQuickSessionId(sessionId);
  const repoDir = getState().repoDir;
  const activeRel = quickSessionRelDir(sessionId, false);
  const active: LocatedQuickSession = {
    relDir: activeRel,
    absDir: `${repoDir}/${activeRel}`,
    archived: false,
  };
  if (await exists(`${active.absDir}/session.meta.yaml`)) return active;
  const archivedRel = quickSessionRelDir(sessionId, true);
  const archived: LocatedQuickSession = {
    relDir: archivedRel,
    absDir: `${repoDir}/${archivedRel}`,
    archived: true,
  };
  return (await exists(`${archived.absDir}/session.meta.yaml`)) ? archived : null;
}

async function loadQuickSessionMeta(
  located: LocatedQuickSession,
): Promise<QuickSessionMeta> {
  return parseQuickSessionMeta(
    await readFile(`${located.absDir}/session.meta.yaml`),
  ) as QuickSessionMeta;
}

async function quickSessionDetail(
  located: LocatedQuickSession,
  query: ReadQuickSessionQuery = {},
): Promise<{
  meta: QuickSessionMeta;
  entries: ThreadEntry[];
  archived: boolean;
}> {
  const meta = await loadQuickSessionMeta(located);
  const thread = await readFile(`${located.absDir}/discussion.thread`);
  const entries = boundedQuickSessionEntries(parseThread(thread).entries, query);
  return { meta, entries, archived: located.archived };
}

function boundedQuickSessionEntries(
  entries: ThreadEntry[],
  query: ReadQuickSessionQuery,
): ThreadEntry[] {
  const since = query.since;
  const filtered =
    since === undefined
      ? entries
      : entries.filter((entry) => entry.line_number > since);
  if (query.limit === undefined) return filtered;
  const limit = Math.max(0, Math.trunc(query.limit));
  if (limit === 0) return [];
  return query.since === undefined
    ? filtered.slice(-limit)
    : filtered.slice(0, limit);
}

function quickSessionListItem(
  meta: QuickSessionMeta,
  archived: boolean,
): Record<string, unknown> {
  return {
    id: meta.id,
    title: meta.title ?? null,
    agent_id: meta.agent_id,
    created_by: meta.created_by,
    status: meta.status,
    updated_at: meta.updated_at,
    last_message_preview: meta.last_message_preview,
    revision: meta.revision,
    archived,
    ref: `session:${meta.id}`,
  };
}

function quickSessionSendResponse(
  meta: QuickSessionMeta,
  lineNumber: number,
): Record<string, unknown> {
  return {
    session_id: meta.id,
    line_number: lineNumber,
    status: meta.status,
    revision: meta.revision,
    ref: `session:${meta.id}`,
  };
}

function quickSessionSyncFields(
  sync: Awaited<ReturnType<typeof syncAfterCommit>>,
): Record<string, unknown> {
  return {
    sync_status: sync.status,
    ...(sync.commit_id ? { commit_id: sync.commit_id } : {}),
    ...(sync.error ? { sync_error: sync.error } : {}),
    ...(sync.error_code ? { error_code: sync.error_code } : {}),
    ...(sync.needs_token ? { needs_token: true } : {}),
  };
}

async function removeQuickSessionDirectoryRequired(absDir: string): Promise<void> {
  for (const name of ["session.meta.yaml", "discussion.thread"]) {
    await removeFile(`${absDir}/${name}`);
  }
  await removeDir(absDir);
}

async function removeQuickSessionDirectoryIfExists(absDir: string): Promise<void> {
  for (const name of ["session.meta.yaml", "discussion.thread"]) {
    const path = `${absDir}/${name}`;
    if (await exists(path)) await removeFile(path);
  }
  if (await exists(absDir)) await removeDir(absDir);
}

async function rollbackQuickSessionMutation(
  repoDir: string,
  paths: string[],
  restoreWorktree: () => Promise<void>,
  cause: unknown,
): Promise<never> {
  const rollbackErrors: string[] = [];
  try {
    await restoreWorktree();
  } catch (error) {
    rollbackErrors.push(`worktree: ${errorMessage(error)}`);
  }
  try {
    await gitOps.restoreIndexPaths(repoDir, paths);
  } catch (error) {
    rollbackErrors.push(`index: ${errorMessage(error)}`);
  }
  if (rollbackErrors.length > 0) {
    throw new Error(
      `${errorMessage(cause)}; quick session rollback failed (${rollbackErrors.join("; ")})`,
    );
  }
  throw cause;
}

async function mkdirp(path: string): Promise<void> {
  const parts = path.split("/").filter(Boolean);
  let current = path.startsWith("/") ? "" : ".";
  for (const part of parts) {
    current = current === "" ? `/${part}` : `${current}/${part}`;
    if (!(await exists(current))) await mkdir(current);
  }
}

function utcTimestamp(): string {
  const now = new Date();
  const pad = (value: number, length = 2) => String(value).padStart(length, "0");
  return (
    `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}` +
    `T${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}Z`
  );
}


function quickSessionChangeFromPath(path: string): {
  archived: boolean;
  sessionId: string;
  file: "meta" | "thread";
} | null {
  const match = path.match(
    /^(archive\/)?quick-sessions\/([^/]+)\/(session\.meta\.yaml|discussion\.thread)$/,
  );
  if (!match || quickSessionIdError(match[2])) return null;
  return {
    archived: match[1] !== undefined,
    sessionId: match[2],
    file: match[3] === "session.meta.yaml" ? "meta" : "thread",
  };
}


export async function classifyQuickSessionPollChange(
  path: string,
  baseline?: string,
): Promise<ClassifiedQuickSessionPollChange | null | undefined> {
  await ensureWasmReady();
  const parsed = quickSessionChangeFromPath(path);
  if (!parsed) return undefined;

  const s = getState();
  const relDir = quickSessionRelDir(parsed.sessionId, parsed.archived);
  const located: LocatedQuickSession = {
    relDir,
    absDir: `${s.repoDir}/${relDir}`,
    archived: parsed.archived,
  };
  if (!(await exists(`${located.absDir}/session.meta.yaml`))) return null;

  let meta: QuickSessionMeta;
  try {
    meta = await loadQuickSessionMeta(located);
  } catch {
    return null;
  }
  const routesToAgent = !located.archived && meta.status !== "archived";
  if (parsed.file === "meta") {
    return {
      channel: meta.id,
      kind: "quick_session_meta",
      entries: [{
        type: "quick_session_meta",
        session_id: meta.id,
        agent_id: meta.agent_id,
        status: meta.status,
        revision: meta.revision,
        ...(routesToAgent ? { recipients: [meta.agent_id] } : {}),
      }],
    };
  }

  if (!(await exists(`${located.absDir}/discussion.thread`))) return null;
  const currentThread = await readFile(`${located.absDir}/discussion.thread`);
  let addedThread = currentThread;
  if (baseline) {
    const baselineThread = await gitOps.readFileAtCommit(
      s.repoDir,
      baseline,
      `${relDir}/discussion.thread`,
    );
    if (baselineThread !== null) {
      if (!currentThread.startsWith(baselineThread)) {
        throw new Error(
          `Quick session ${meta.id} transcript changed outside append-only shape`,
        );
      }
      addedThread = currentThread.slice(baselineThread.length);
    }
  }
  const thread = parseThread(addedThread);
  if (thread.entries.length === 0) return null;
  return {
    channel: meta.id,
    kind: "quick_session_thread",
    entries: thread.entries.map((entry) =>
      routesToAgent && entry.type === "message"
        ? { ...entry, recipients: [meta.agent_id] }
        : entry
    ),
  };
}
