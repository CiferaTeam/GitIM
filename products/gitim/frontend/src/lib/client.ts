/**
 * Unified client - all methods hit the real runtime HTTP API.
 * Agent methods fall back to mock if runtime is unreachable.
 *
 * Workspace-scoped methods take `slug` as the first parameter.
 * Global (unscoped) methods: health, listWorkspaces, createWorkspace,
 * deleteWorkspace, getWorkspace, preflightProvider.
 */
import type {
  Agent,
  ApiResponse,
  BoardReadResponse,
  BoardSummary,
  BoardWriteResponse,
  Card,
  CardStatus,
  Channel,
  CreateWorkspaceRequest,
  CronDetail,
  CronRunBody,
  CronRunEntry,
  CronSummary,
  CronTimelineResponse,
  Message,
  WorkspaceSummary,
} from "./types";
import type { PreflightResult, ProviderId } from "./providers";
import type { HermesLlmProvider, HermesLlmModelList } from "./hermes-llm";
import type {
  Backend,
  BoardBackend,
  CardBackend,
  ChannelArchiveBackend,
  DmArchiveBackend,
} from "./backend";
import { HttpBackend, LocalBackend } from "./backend";
import {
  clearAllBrowserWorkspaces,
  clearSessionToken,
  createBrowserWorkspace,
  forgetBrowserWorkspace,
  forgetBrowserWorkspaceAndWipeCache,
  getBrowserWorkspace,
  listBrowserWorkspaceSummaries,
  loadSessionToken,
  saveSessionToken,
  updateBrowserWorkspace,
  wipeAllBrowserWorkspaceCaches,
  wipeBrowserWorkspaceCache,
  type BrowserWorkspaceRecord,
} from "./browser-workspaces";
import * as mockClient from "./mock/client";
import { useConnectionStore } from "@/hooks/use-connection-store";

let activeBackend: Backend = new HttpBackend(() => baseUrl());
let activeLocalBackend: LocalBackend | null = null;
let activeBrowserWorkspaceId: string | null = null;
let localGeneration = 0;
let browserActivationAttempt = 0;

interface BrowserCacheActionResult extends Record<string, unknown> {
  activeAffected: boolean;
}

export function setBackend(backend: Backend): void {
  activeBackend = backend;
}

export function rememberBrowserToken(workspaceId: string, token: string): void {
  saveSessionToken(workspaceId, token);
}

export function clearBrowserToken(workspaceId: string): void {
  clearSessionToken(workspaceId);
}

export function resetAllBrowserWorkspaces(): Promise<ApiResponse<BrowserCacheActionResult>> {
  return startOverBrowserWorkspaces();
}

export function shutdownBrowserWorkspace(): void {
  browserActivationAttempt += 1;
  const backend = activeLocalBackend;
  if (!backend) return;

  backend.terminate();
  if (activeBackend === backend) {
    activeBackend = new HttpBackend(() => baseUrl());
  }
  activeLocalBackend = null;
  activeBrowserWorkspaceId = null;
}

export async function activateBrowserWorkspace(
  idOrSlug: string,
  options: {
    token?: string | null;
    onSyncReset?: () => void;
  } = {},
): Promise<ApiResponse<{
  workspace: BrowserWorkspaceRecord;
  needs_token?: boolean;
  sync_enabled?: boolean;
}>> {
  const attempt = browserActivationAttempt + 1;
  browserActivationAttempt = attempt;
  const record = findBrowserWorkspace(idOrSlug);
  if (!record) {
    return { ok: false, error: "Browser workspace not found", error_code: "not_found" };
  }

  const generation = localGeneration + 1;
  const backend = new LocalBackend({
    workspaceId: record.id,
    generation,
    onSyncReset: options.onSyncReset,
  });
  const token = options.token ?? loadSessionToken(record.id) ?? null;
  const result = await backend.init({
    workspaceId: record.id,
    remoteUrl: record.remoteUrl,
    corsProxy: record.corsProxy ?? "",
    token,
    handler: record.handler ?? "",
    storage: record.storage as { fsName: string; repoDir: "/repo" },
  });

  if (!result.ok) {
    backend.terminate();
    return result as ApiResponse<{
      workspace: BrowserWorkspaceRecord;
      needs_token?: boolean;
      sync_enabled?: boolean;
    }>;
  }

  if (!isCurrentBrowserActivation(attempt)) {
    return supersedeBrowserActivation(backend);
  }

  if (token) {
    saveSessionToken(record.id, token);
    try {
      await backend.startSync();
    } catch (error) {
      backend.terminate();
      return {
        ok: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }

  if (!isCurrentBrowserActivation(attempt)) {
    return supersedeBrowserActivation(backend);
  }

  activeLocalBackend?.terminate();
  localGeneration = generation;
  activeLocalBackend = backend;
  activeBrowserWorkspaceId = record.id;
  setBackend(backend);

  const data = result.data as Record<string, unknown> | undefined;
  const handler = typeof data?.handler === "string" ? data.handler : undefined;
  const displayName =
    typeof data?.display_name === "string" ? data.display_name : undefined;
  const shouldFillWorkspaceName = record.workspace_name.trim().length === 0;
  const updated =
    handler || shouldFillWorkspaceName
      ? updateBrowserWorkspace(record.id, {
          handler: handler ?? record.handler,
          ...(shouldFillWorkspaceName
            ? { workspaceName: displayName ?? handler ?? record.workspace_name }
            : {}),
        })
      : undefined;

  return {
    ok: true,
    data: {
      workspace: updated ?? record,
      needs_token: data?.needs_token === true,
      sync_enabled: data?.sync_enabled === true,
    },
  };
}

// --- Helpers ---

function baseUrl(): string {
  return useConnectionStore.getState().baseUrl();
}

function isLocalMode(): boolean {
  return useConnectionStore.getState().mode === "local";
}

function isCurrentBrowserActivation(attempt: number): boolean {
  return attempt === browserActivationAttempt && isLocalMode();
}

function supersedeBrowserActivation(
  backend: LocalBackend,
): ApiResponse<{
  workspace: BrowserWorkspaceRecord;
  needs_token?: boolean;
  sync_enabled?: boolean;
}> {
  backend.terminate();
  return {
    ok: false,
    error: "Browser workspace activation was superseded.",
    error_code: "activation_superseded",
  };
}

function findBrowserWorkspace(idOrSlug: string): BrowserWorkspaceRecord | undefined {
  return getBrowserWorkspace(idOrSlug);
}

function localCardBackend(): CardBackend {
  return activeBackend as Backend & CardBackend;
}

function localChannelArchiveBackend(): ChannelArchiveBackend {
  return activeBackend as Backend & ChannelArchiveBackend;
}

function localDmArchiveBackend(): DmArchiveBackend {
  return activeBackend as Backend & DmArchiveBackend;
}

function localBoardBackend(): BoardBackend {
  return activeBackend as Backend & BoardBackend;
}

function wsBase(slug: string): string {
  return `${baseUrl()}/workspaces/${encodeURIComponent(slug)}`;
}

// --- Health ---

// `cache: "no-store"` is load-bearing for the self-update restart poll:
// /health sets no Cache-Control, and browsers happily serve repeated fetches
// from the memory cache within a few seconds. During the restart window the
// first poll can latch onto the old process's {version: "0.5.x"} response and
// never see the new runtime's version no matter how many times we poll,
// causing the 30s timeout to fire even though the update actually succeeded.
// `signal` lets the caller cap a single in-flight request so the poll loop
// can move on if the old process is tearing down mid-fetch.
export async function health(signal?: AbortSignal): Promise<ApiResponse> {
  if (isLocalMode()) return activeBackend.health();
  const res = await fetch(`${baseUrl()}/health`, { cache: "no-store", signal });
  if (!res.ok) return { ok: false, error: `health check failed: ${res.status}` };
  const data = await res.json();
  return { ok: true, data };
}

// --- Runtime self-update ---

export interface UpdateAndRestartData {
  job_id: string;
  target_version: string;
  started_at: string;
}

/**
 * POST /runtime/update-and-restart — kicks off self-update (Task 6/7).
 * Returns 202 on accept. After accept, the runtime HTTP server will stop
 * responding until the new binary re-binds the port; callers are expected
 * to poll `health()` to detect the transition.
 */
export async function updateAndRestart(): Promise<ApiResponse<UpdateAndRestartData>> {
  if (isLocalMode()) {
    return { ok: false, error: "runtime update is unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${baseUrl()}/runtime/update-and-restart`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      return {
        ok: false,
        error: data.detail ?? data.error ?? `HTTP ${res.status}`,
        error_code: data.error_code,
      };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

// --- Workspace CRUD (global, no slug) ---

export async function listWorkspaces(): Promise<
  ApiResponse<{ workspaces: WorkspaceSummary[] }>
> {
  if (isLocalMode()) {
    return { ok: true, data: { workspaces: listBrowserWorkspaceSummaries() } };
  }
  try {
    const res = await fetch(`${baseUrl()}/workspaces`);
    const data = await res.json();
    if (!res.ok) {
      return { ok: false, error: data.error ?? `HTTP ${res.status}`, error_code: data.error_code };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function createWorkspace(
  req: CreateWorkspaceRequest,
): Promise<ApiResponse<{ slug: string; workspace_name: string; path: string; provider: string }>> {
  if (isLocalMode()) {
    if (req.git.provider !== "github") {
      return {
        ok: false,
        error: "Browser workspaces require a GitHub remote",
        error_code: "unsupported_provider",
      };
    }

    const record = createBrowserWorkspace({
      remoteUrl: req.git.remote_url,
      corsProxy: "https://cors.isomorphic-git.org",
      handler: "",
      workspaceName: req.workspace_name,
    });
    saveSessionToken(record.id, req.git.token);
    return {
      ok: true,
      data: {
        slug: record.slug,
        workspace_name: record.workspace_name,
        path: `indexeddb://${record.storage.fsName}${record.storage.repoDir}`,
        provider: "github",
      },
    };
  }
  try {
    const res = await fetch(`${baseUrl()}/workspaces`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    });
    const data = await res.json();
    if (!res.ok || data.ok === false) {
      return {
        ok: false,
        error: data.error ?? `HTTP ${res.status}`,
        error_code: data.error_code,
      };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function getWorkspace(slug: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    return {
      ok: true,
      data: {
        slug,
        workspace_name: "Browser",
        path: "indexeddb://gitim/browser",
        provider: "github",
        initialized: true,
      },
    };
  }
  try {
    const res = await fetch(wsBase(slug));
    const data = await res.json();
    if (!res.ok) {
      return { ok: false, error: data.error ?? `HTTP ${res.status}`, error_code: data.error_code };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function deleteWorkspace(slug: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    const record = findBrowserWorkspace(slug);
    if (!record) {
      return { ok: false, error: "Browser workspace not found", error_code: "not_found" };
    }
    forgetBrowserWorkspace(record.id);
    return { ok: true, data: {} };
  }
  try {
    const res = await fetch(wsBase(slug), { method: "DELETE" });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      return { ok: false, error: data.error ?? `HTTP ${res.status}`, error_code: data.error_code };
    }
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function resetBrowserWorkspaceCache(
  slug: string,
): Promise<ApiResponse<BrowserCacheActionResult>> {
  const record = findBrowserWorkspace(slug);
  if (!record) {
    return { ok: false, error: "Browser workspace not found", error_code: "not_found" };
  }

  const activeAffected = record.id === activeBrowserWorkspaceId;
  if (activeAffected) {
    shutdownBrowserWorkspace();
  }
  await wipeBrowserWorkspaceCache(record.id);
  return { ok: true, data: { activeAffected } };
}

export async function forgetBrowserWorkspaceAndCache(
  slug: string,
): Promise<ApiResponse<BrowserCacheActionResult>> {
  const record = findBrowserWorkspace(slug);
  if (!record) {
    return { ok: false, error: "Browser workspace not found", error_code: "not_found" };
  }

  const activeAffected = record.id === activeBrowserWorkspaceId;
  if (activeAffected) {
    shutdownBrowserWorkspace();
  }
  await forgetBrowserWorkspaceAndWipeCache(record.id);
  return { ok: true, data: { activeAffected } };
}

export async function startOverBrowserWorkspaces(): Promise<ApiResponse<BrowserCacheActionResult>> {
  const activeAffected = activeBrowserWorkspaceId !== null;
  shutdownBrowserWorkspace();
  await wipeAllBrowserWorkspaceCaches();
  clearAllBrowserWorkspaces();
  return { ok: true, data: { activeAffected } };
}

// --- IM methods: real runtime HTTP (all scoped to a workspace) ---

export async function me(slug: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return activeBackend.me();
  }
  const res = await fetch(`${wsBase(slug)}/im/me`);
  return await res.json();
}

export async function poll(slug: string, since?: string, signal?: AbortSignal): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void signal;
    return activeBackend.poll(since);
  }
  const res = await fetch(`${wsBase(slug)}/im/poll`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ since }),
    signal,
  });
  return await res.json();
}

export async function channels(slug: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return activeBackend.channels();
  }
  const res = await fetch(`${wsBase(slug)}/im/channels`);
  return await res.json();
}

export async function send(
  slug: string,
  channel: string,
  body: string,
  _author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    return activeBackend.send(channel, body, _author, replyTo);
  }
  const res = await fetch(`${wsBase(slug)}/im/send`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, body, reply_to: replyTo }),
  });
  return await res.json();
}

export async function createChannel(
  slug: string,
  name: string,
  displayName?: string,
  introduction?: string,
  invitees?: string[],
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void name;
    void displayName;
    void introduction;
    void invitees;
    return { ok: false, error: "channel creation is unavailable in browser mode" };
  }
  const payload: Record<string, unknown> = { name, display_name: displayName, introduction };
  if (invitees && invitees.length > 0) {
    payload.invitees = invitees;
  }
  const res = await fetch(`${wsBase(slug)}/im/create-channel`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export async function joinChannel(
  slug: string,
  channel: string,
  targets?: string[],
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void targets;
    return activeBackend.joinChannel(channel);
  }
  const payload: Record<string, unknown> = { channel };
  if (targets && targets.length > 0) {
    payload.targets = targets;
  }
  const res = await fetch(`${wsBase(slug)}/im/join`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export async function read(
  slug: string,
  channel: string,
  limit?: number,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return activeBackend.read(channel, limit);
  }
  const res = await fetch(`${wsBase(slug)}/im/read`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, limit }),
  });
  return await res.json();
}

export async function thread(
  slug: string,
  channel: string,
  line: number,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return activeBackend.thread(channel, line);
  }
  const res = await fetch(`${wsBase(slug)}/im/thread`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ channel, line }),
  });
  return await res.json();
}

export async function users(slug: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return activeBackend.users();
  }
  const res = await fetch(`${wsBase(slug)}/im/users`);
  return await res.json();
}

// --- Board API: real runtime HTTP (all scoped to a workspace) ---

export async function listBoards(
  slug: string,
): Promise<ApiResponse<{ boards: BoardSummary[] }>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().listBoards();
  }
  const res = await fetch(`${wsBase(slug)}/im/boards`);
  return await res.json();
}

export async function showBoard(
  slug: string,
  handler: string,
): Promise<ApiResponse<BoardReadResponse>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().showBoard(handler);
  }
  const res = await fetch(`${wsBase(slug)}/im/boards/${encodeURIComponent(handler)}`);
  return await res.json();
}

export async function initBoard(
  slug: string,
): Promise<ApiResponse<BoardWriteResponse>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().initBoard();
  }
  const res = await fetch(`${wsBase(slug)}/im/board/init`, { method: "POST" });
  return await res.json();
}

export async function publishBoard(
  slug: string,
  content?: string,
): Promise<ApiResponse<BoardWriteResponse>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().publishBoard(content);
  }
  const res = await fetch(`${wsBase(slug)}/im/board/publish`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ content }),
  });
  return await res.json();
}

export async function setBoard(
  slug: string,
  field: string,
  value: string,
): Promise<ApiResponse<BoardWriteResponse>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().setBoard(field, value);
  }
  const res = await fetch(`${wsBase(slug)}/im/board/field`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ field, value }),
  });
  return await res.json();
}

export async function setBoardSection(
  slug: string,
  section: string,
  value: string,
): Promise<ApiResponse<BoardWriteResponse>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().setBoardSection(section, value);
  }
  const res = await fetch(`${wsBase(slug)}/im/board/section/set`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ section, value }),
  });
  return await res.json();
}

export async function appendBoardSection(
  slug: string,
  section: string,
  value: string,
): Promise<ApiResponse<BoardWriteResponse>> {
  if (isLocalMode()) {
    void slug;
    return localBoardBackend().appendBoardSection(section, value);
  }
  const res = await fetch(`${wsBase(slug)}/im/board/section/append`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ section, value }),
  });
  return await res.json();
}

// --- Cron API: runtime HTTP read endpoints (Wave 3 consumer) ---
//
// All five routes are GET-only in v1 — calendar is view-only. Local (browser)
// mode has no cron engine, so these short-circuit to a friendly empty
// response shape rather than 404 / unreachable network errors.
//
// Wire contract: the runtime cron handlers (gitim-runtime::http.rs Phase 4
// typed-response style) return RAW typed JSON bodies on success — e.g.
// `{crons: [...]}` for list, `{entries: [...]}` for timeline, the
// `CronDetail` directly for show, `{body: "..."}` for a single run. They
// only emit the legacy `{ok, error, error_code}` envelope on failure, via
// `ErrorBody`. That asymmetry means we can't just `return await res.json()`
// and call it an `ApiResponse` (success bodies have no `ok` field, so
// `res.ok` reads as `undefined` and consumers treat every successful
// fetch as a failure — the bug Phase 6 was opened to fix).
//
// `cronRequest` re-wraps these raw success bodies in `{ok: true, data}` so
// the cron callers in WebUI keep their existing `ApiResponse` ergonomics,
// while still surfacing `error` / `error_code` on the failure path. The
// rest of the client (channels/dms/boards) returns daemon-shaped envelopes
// directly, so they don't need this helper.
const CRON_LOCAL_UNAVAILABLE: ApiResponse<never> = {
  ok: false,
  error: "Cron view requires the gitim runtime (not available in browser mode).",
  error_code: "runtime_required",
};

async function cronRequest<T>(
  url: string,
  signal?: AbortSignal,
): Promise<ApiResponse<T>> {
  let res: Response;
  try {
    res = await fetch(url, signal ? { signal } : undefined);
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    if (e instanceof Error && e.name === "AbortError") throw e;
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  // Parse the body once. Empty / malformed body on a 2xx is treated as an
  // error so consumers don't render with `undefined` data and crash deeper.
  let body: unknown;
  try {
    body = await res.json();
  } catch (e) {
    return {
      ok: false,
      error: res.ok
        ? `failed to parse response body: ${e instanceof Error ? e.message : String(e)}`
        : `HTTP ${res.status}`,
    };
  }
  if (!res.ok) {
    // ErrorBody shape from runtime: `{ok: false, error, error_code?}`.
    const obj = (body as Record<string, unknown>) ?? {};
    return {
      ok: false,
      error: typeof obj.error === "string" ? obj.error : `HTTP ${res.status}`,
      error_code: typeof obj.error_code === "string" ? obj.error_code : undefined,
    };
  }
  // 2xx — runtime returns the raw typed body. Wrap it.
  return { ok: true, data: body as T };
}

export async function listCrons(
  slug: string,
): Promise<ApiResponse<{ crons: CronSummary[] }>> {
  if (isLocalMode()) return CRON_LOCAL_UNAVAILABLE;
  return cronRequest<{ crons: CronSummary[] }>(`${wsBase(slug)}/crons`);
}

export async function showCron(
  slug: string,
  name: string,
  signal?: AbortSignal,
): Promise<ApiResponse<CronDetail>> {
  if (isLocalMode()) return CRON_LOCAL_UNAVAILABLE;
  return cronRequest<CronDetail>(
    `${wsBase(slug)}/crons/${encodeURIComponent(name)}`,
    signal,
  );
}

export async function listCronRuns(
  slug: string,
  name: string,
): Promise<ApiResponse<{ runs: CronRunEntry[] }>> {
  if (isLocalMode()) return CRON_LOCAL_UNAVAILABLE;
  return cronRequest<{ runs: CronRunEntry[] }>(
    `${wsBase(slug)}/crons/${encodeURIComponent(name)}/runs`,
  );
}

export async function getCronRunBody(
  slug: string,
  name: string,
  ts: string,
  signal?: AbortSignal,
): Promise<ApiResponse<CronRunBody>> {
  if (isLocalMode()) return CRON_LOCAL_UNAVAILABLE;
  return cronRequest<CronRunBody>(
    `${wsBase(slug)}/crons/${encodeURIComponent(name)}/runs/${encodeURIComponent(ts)}`,
    signal,
  );
}

export async function getCronTimeline(
  slug: string,
  from?: string,
  to?: string,
  signal?: AbortSignal,
): Promise<ApiResponse<CronTimelineResponse>> {
  if (isLocalMode()) return CRON_LOCAL_UNAVAILABLE;
  const params = new URLSearchParams();
  if (from) params.set("from", from);
  if (to) params.set("to", to);
  const qs = params.toString();
  const url = `${wsBase(slug)}/crons/timeline${qs ? `?${qs}` : ""}`;
  return cronRequest<CronTimelineResponse>(url, signal);
}

/** Sanitize a display name into a valid handler (a-z, 0-9, hyphens). */
export function toHandler(name: string): string {
  return name
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^a-z0-9-]/g, "")
    .replace(/-{2,}/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 39);
}

/** Validate a handler. Returns error message or null if valid. */
export function validateHandler(name: string): string | null {
  const handler = toHandler(name);
  if (!handler) return "Name must contain at least one letter or digit";
  if (handler === "system") return "\"system\" is reserved";
  return null;
}

/** Validate a channel name. Returns error message or null if valid. */
export function validateChannelName(name: string): string | null {
  if (!name) return "Channel name is required";
  if (name.length > 32) return "Channel name must be 32 characters or less";
  if (!/^[a-z0-9-]+$/.test(name)) return "Only lowercase letters, numbers, and hyphens";
  if (name.startsWith("-") || name.endsWith("-")) return "Cannot start or end with a hyphen";
  if (name.includes("--")) return "Cannot contain consecutive hyphens";
  return null;
}

// --- Card API: real runtime HTTP (all scoped to a workspace) ---

export interface CreateCardOpts {
  labels?: string[];
  assignee?: string | null;
  status?: CardStatus;
}

export async function createCard(
  slug: string,
  channel: string,
  title: string,
  opts: CreateCardOpts = {},
): Promise<ApiResponse<{ channel: string; card_id: string; title: string }>> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().createCard(channel, title, opts) as Promise<
      ApiResponse<{ channel: string; card_id: string; title: string }>
    >;
  }
  const payload: Record<string, unknown> = { channel, title };
  if (opts.labels && opts.labels.length > 0) payload.labels = opts.labels;
  if (opts.assignee) payload.assignee = opts.assignee;
  if (opts.status) payload.status = opts.status;
  const res = await fetch(`${wsBase(slug)}/im/cards`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  return await res.json();
}

export interface ListCardsQuery {
  channel?: string | null;
  labels?: string[];
  status?: CardStatus | null;
  assignee?: string | null;
}

export async function listCards(
  slug: string,
  query: ListCardsQuery = {},
): Promise<ApiResponse<{ cards: Card[] }>> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().listCards(query) as Promise<ApiResponse<{ cards: Card[] }>>;
  }
  const params = new URLSearchParams();
  if (query.channel) params.set("channel", query.channel);
  if (query.status) params.set("status", query.status);
  if (query.assignee) params.set("assignee", query.assignee);
  if (query.labels) {
    for (const l of query.labels) params.append("label", l);
  }
  const qs = params.toString();
  const url = qs ? `${wsBase(slug)}/im/cards?${qs}` : `${wsBase(slug)}/im/cards`;
  const res = await fetch(url);
  return await res.json();
}

export interface ReadCardQuery {
  limit?: number;
  since?: number;
}

export async function readCard(
  slug: string,
  channel: string,
  cardId: string,
  query: ReadCardQuery = {},
): Promise<ApiResponse<{ meta: Card; entries: Message[]; archived: boolean }>> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().readCard(channel, cardId, query) as Promise<
      ApiResponse<{ meta: Card; entries: Message[]; archived: boolean }>
    >;
  }
  const params = new URLSearchParams();
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.since != null) params.set("since", String(query.since));
  const qs = params.toString();
  const base = `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}`;
  const url = qs ? `${base}?${qs}` : base;
  const res = await fetch(url);
  return await res.json();
}

export async function sendCardMessage(
  slug: string,
  channel: string,
  cardId: string,
  body: string,
  replyTo?: number,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().sendCardMessage(channel, cardId, body, replyTo);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/messages`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ body, reply_to: replyTo }),
    },
  );
  return await res.json();
}

export interface UpdateCardPatch {
  status?: CardStatus;
  labels?: string[];
  assignee?: string | null;
}

export async function updateCard(
  slug: string,
  channel: string,
  cardId: string,
  patch: UpdateCardPatch,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().updateCard(channel, cardId, patch);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}`,
    {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    },
  );
  return await res.json();
}

// --- Archive API: runtime derives author from workspace me.json, so no body needed. ---

export async function archiveCard(
  slug: string,
  channel: string,
  cardId: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().archiveCard(channel, cardId);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/archive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function unarchiveCard(
  slug: string,
  channel: string,
  cardId: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().unarchiveCard(channel, cardId);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/cards/${encodeURIComponent(channel)}/${encodeURIComponent(cardId)}/unarchive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function listArchivedCards(
  slug: string,
  channel?: string,
): Promise<ApiResponse<{ cards: Card[] }>> {
  if (isLocalMode()) {
    void slug;
    return localCardBackend().listArchivedCards(channel) as Promise<ApiResponse<{ cards: Card[] }>>;
  }
  const qs = channel ? `?channel=${encodeURIComponent(channel)}` : "";
  const res = await fetch(`${wsBase(slug)}/im/cards/archived${qs}`);
  return await res.json();
}

export async function archiveChannel(
  slug: string,
  name: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localChannelArchiveBackend().archiveChannel(name);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/channels/${encodeURIComponent(name)}/archive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function unarchiveChannel(
  slug: string,
  name: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localChannelArchiveBackend().unarchiveChannel(name);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/channels/${encodeURIComponent(name)}/unarchive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function listArchivedChannels(
  slug: string,
): Promise<ApiResponse<{ channels: Channel[] }>> {
  if (isLocalMode()) {
    void slug;
    return localChannelArchiveBackend().listArchivedChannels() as Promise<
      ApiResponse<{ channels: Channel[] }>
    >;
  }
  const res = await fetch(`${wsBase(slug)}/im/channels/archived`);
  return await res.json();
}

export async function archiveDm(
  slug: string,
  peer: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localDmArchiveBackend().archiveDm(peer);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/dm/${encodeURIComponent(peer)}/archive`,
    { method: "POST" },
  );
  return await res.json();
}

export async function unarchiveDm(
  slug: string,
  peer: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return localDmArchiveBackend().unarchiveDm(peer);
  }
  const res = await fetch(
    `${wsBase(slug)}/im/dm/${encodeURIComponent(peer)}/unarchive`,
    { method: "POST" },
  );
  return await res.json();
}

/**
 * Each row is `{ peer, dm_pair_stem }` — the stem is the on-disk filename
 * (`<min>--<max>`) so we can synthesize a Channel-shaped record without
 * re-deriving the sort. Caller participation filtering is the daemon's job.
 */
export interface ArchivedDmEntry {
  peer: string;
  dm_pair_stem: string;
}

export async function listArchivedDms(
  slug: string,
): Promise<ApiResponse<{ dms: ArchivedDmEntry[] }>> {
  if (isLocalMode()) {
    void slug;
    return localDmArchiveBackend().listArchivedDms() as Promise<
      ApiResponse<{ dms: ArchivedDmEntry[] }>
    >;
  }
  const res = await fetch(`${wsBase(slug)}/im/dm/archived`);
  return await res.json();
}

// --- Preflight (global, no slug) ---

export async function preflightProvider(
  provider: ProviderId,
  opts?: { llmProvider?: string; llmModel?: string },
): Promise<ApiResponse<PreflightResult>> {
  if (isLocalMode()) {
    void provider;
    return { ok: false, error: "provider preflight is unavailable in browser mode" };
  }
  try {
    const params = new URLSearchParams();
    if (opts?.llmProvider) params.set("llm_provider", opts.llmProvider);
    if (opts?.llmModel) params.set("llm_model", opts.llmModel);
    const qs = params.size > 0 ? `?${params.toString()}` : "";
    const res = await fetch(`${baseUrl()}/preflight/${provider}${qs}`);
    const data = await res.json();
    if (res.ok) {
      return { ok: true, data };
    }
    return { ok: false, error: data.error ?? `HTTP ${res.status}` };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function listHermesLlmProviders(): Promise<ApiResponse<{ providers: HermesLlmProvider[] }>> {
  if (isLocalMode()) {
    return { ok: true, data: { providers: [] } };
  }
  try {
    const res = await fetch(`${baseUrl()}/hermes/llm/providers`);
    const data = await res.json();
    if (res.ok) {
      return { ok: true, data };
    }
    return { ok: false, error: data.error ?? `HTTP ${res.status}` };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function listHermesLlmModels(providerId: string): Promise<ApiResponse<HermesLlmModelList>> {
  if (isLocalMode()) {
    return {
      ok: true,
      data: { models: [], custom_allowed: true, error: null, fetched_at_ms: Date.now() },
    };
  }
  try {
    const res = await fetch(`${baseUrl()}/hermes/llm/providers/${encodeURIComponent(providerId)}/models`);
    const data = await res.json();
    // Backend always returns 200 for this endpoint; error field carries failure info
    return { ok: true, data };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

function mapBackendAgent(raw: Record<string, unknown>): Agent {
  const rawUsage = raw.session_usage as Record<string, unknown> | undefined;
  const sessionUsage: Agent["sessionUsage"] = rawUsage
    ? {
        sessionId: (rawUsage.session_id as string) ?? "",
        inputTokens: rawUsage.input_tokens as number | undefined,
        outputTokens: rawUsage.output_tokens as number | undefined,
        maxTokens: rawUsage.max_tokens as number | undefined,
        usedPercent: (rawUsage.used_percent as number) ?? 0,
        source: (rawUsage.source as "provider_reported" | "runtime_estimated") ?? "provider_reported",
        updatedAt: (rawUsage.updated_at as string) ?? "",
      }
    : undefined;

  const usageSummary = mapBackendUsageSummary(raw.usage_summary);

  return {
    id: (raw.id ?? raw.handler) as string,
    name: (raw.display_name ?? raw.handler) as string,
    status: ((raw.status as string) === "idle" ? "offline" : raw.status) as Agent["status"],
    provider: (raw.provider as ProviderId) ?? undefined,
    systemPrompt: (raw.system_prompt as string) ?? "",
    model: (raw.model as string) ?? undefined,
    introduction: (raw.introduction as string) ?? undefined,
    env: (raw.env as Record<string, string>) ?? undefined,
    repoPath: (raw.repo_path as string) ?? "",
    messagesProcessed: (raw.messages_processed as number) ?? 0,
    lastActivity: raw.last_activity as string | undefined,
    errorMessage: (raw.error_message as string) ?? undefined,
    sessionUsage,
    llmProvider: (raw.llm_provider as string) ?? undefined,
    llmModel: (raw.llm_model as string) ?? undefined,
    usageSummary,
  };
}

/** Convert the runtime's snake_case `usage_summary` payload into the
 *  camelCase shape the React side expects. Defensive against missing or
 *  malformed nested objects so an older runtime cannot crash the UI. */
export function mapBackendUsageSummary(raw: unknown): Agent["usageSummary"] {
  if (!raw || typeof raw !== "object") return undefined;
  const obj = raw as Record<string, unknown>;
  const totals = mapBucket(obj.totals);
  const today = mapBucket(obj.today);
  const byDay = Array.isArray(obj.by_day)
    ? obj.by_day
        .map((entry) => {
          if (!entry || typeof entry !== "object") return null;
          const e = entry as Record<string, unknown>;
          return {
            date: (e.date as string) ?? "",
            bucket: mapBucket(e.bucket),
          };
        })
        .filter((e): e is NonNullable<typeof e> => e !== null)
    : [];
  return {
    providerReportsUsage: (obj.provider_reports_usage as boolean) ?? true,
    firstSeen: (obj.first_seen as string) ?? "",
    lastUpdated: (obj.last_updated as string) ?? "",
    totals,
    today,
    byDay,
  };
}

function mapBucket(raw: unknown): {
  input: number;
  output: number;
  cacheRead: number;
  cacheCreation: number;
  turns: number;
} {
  if (!raw || typeof raw !== "object") {
    return { input: 0, output: 0, cacheRead: 0, cacheCreation: 0, turns: 0 };
  }
  const b = raw as Record<string, unknown>;
  return {
    input: (b.input as number) ?? 0,
    output: (b.output as number) ?? 0,
    cacheRead: (b.cache_read as number) ?? 0,
    cacheCreation: (b.cache_creation as number) ?? 0,
    turns: (b.turns as number) ?? 0,
  };
}

// --- Agent API: real runtime HTTP (all scoped to a workspace) ---

export async function listAgents(slug: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    return { ok: true, data: { agents: [] } };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents`);
    const data = await res.json();
    if (!data.ok) return data;
    const agents = (data.agents ?? []).map(mapBackendAgent);
    return { ok: true, data: { agents } };
  } catch {
    return mockClient.listAgents();
  }
}

export async function getAgent(slug: string, id: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void id;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents/${id}`);
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: mapBackendAgent(data.agent) } };
  } catch {
    return mockClient.getAgent(id);
  }
}

export async function addAgent(
  slug: string,
  name: string,
  provider: ProviderId,
  systemPrompt: string,
  model?: string,
  env?: Record<string, string>,
  introduction?: string,
  joinGeneral: boolean = true,
  llmProvider?: string,
  llmModel?: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void name;
    void provider;
    void systemPrompt;
    void model;
    void env;
    void introduction;
    void joinGeneral;
    void llmProvider;
    void llmModel;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const handler = toHandler(name);
    const res = await fetch(`${wsBase(slug)}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        handler,
        display_name: name,
        provider,
        model: model || undefined,
        system_prompt: systemPrompt || undefined,
        introduction: introduction && introduction.length > 0 ? introduction : undefined,
        env: env && Object.keys(env).length > 0 ? env : undefined,
        join_general: joinGeneral,
        llm_provider: llmProvider || undefined,
        llm_model: llmModel || undefined,
      }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    // Fetch the full agent info from backend (has repo_path etc.)
    const agentRes = await getAgent(slug, data.id ?? handler);
    if (agentRes.ok && agentRes.data?.agent) {
      return agentRes;
    }
    // Fallback: construct locally if fetch fails
    const agent: Agent = {
      id: data.id ?? handler,
      name,
      status: "offline",
      provider,
      systemPrompt,
      model,
      introduction,
      env,
      repoPath: "",
      messagesProcessed: 0,
      llmProvider,
      llmModel,
    };
    return { ok: true, data: { agent } };
  } catch {
    return mockClient.addAgent(name, provider, systemPrompt);
  }
}

export async function updateAgent(
  slug: string,
  agentId: string,
  patch: {
    system_prompt?: string | null;
    model?: string | null;
    introduction?: string | null;
    env?: Record<string, string>;
    dotenv?: string;
  },
): Promise<ApiResponse<{ agent: Agent }>> {
  if (isLocalMode()) {
    void slug;
    void agentId;
    void patch;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents/${agentId}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    });
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: mapBackendAgent(data.agent) } };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function removeAgent(
  slug: string,
  id: string,
  options: { hardDelete?: boolean } = {},
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void id;
    void options;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id, hard_delete: options.hardDelete === true }),
    });
    return await res.json();
  } catch {
    return mockClient.removeAgent(id);
  }
}

/**
 * archive-protocol burn (B.1) — full workspace-wide departure.
 * Replaces `removeAgent({ hardDelete: true })`. Daemon walks the
 * idempotent multi-commit phase chain (leave events → DM archive →
 * channel meta scrub → user.meta.yaml → archive/users/), runtime
 * deletes the clone + hermes profile, and broadcasts `burned` SSE.
 *
 * Failure modes (`error_code` in response body):
 * - `not_an_agent` (404)
 * - `daemon_unreachable` (500) — RPC IO / spawn failure; retry safe
 * - `daemon_depart_failed` (500) — daemon ok=false; retry resumes
 *   from first incomplete phase via the terminal-state check
 */
export async function agentsBurn(
  slug: string,
  id: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void id;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents/burn`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    return await res.json();
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

/**
 * Archived-user entry from daemon `list_archived_users`. Only the
 * handler is structurally guaranteed — display_name is best-effort
 * because the runtime no longer holds the agent metadata after burn,
 * and the daemon reads it from `archive/users/<handler>.meta.yaml` if
 * the file is well-formed. WebUI must render gracefully when
 * display_name is absent.
 */
export interface ArchivedUserEntry {
  handler: string;
  display_name?: string;
}

export async function listArchivedUsers(
  slug: string,
): Promise<ApiResponse<{ users: ArchivedUserEntry[] }>> {
  if (isLocalMode()) {
    void slug;
    return { ok: true, data: { users: [] } };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/users/archived`);
    const data = await res.json();
    if (!data.ok) return data;
    // Daemon returns `{ users: [{handler, display_name?}, ...] }`.
    // Pre-archive-protocol the daemon emitted bare handler strings; we
    // still tolerate that shape so a stale WebUI talking to a fresh
    // daemon (or vice versa) doesn't crash on the row map.
    const raw = (data.data?.users ?? data.users ?? []) as unknown[];
    const users: ArchivedUserEntry[] = raw.map((u) =>
      typeof u === "string"
        ? { handler: u }
        : (u as ArchivedUserEntry),
    );
    return { ok: true, data: { users } };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

/**
 * Move `archive/users/<handler>.meta.yaml` back to `users/`. Recovery
 * action exposed on the show-archived view of the agent list — reverses
 * the daemon side of `agentsBurn` only; the agent runtime instance is
 * not respawned (operator must re-add the agent if they want it live).
 */
export async function unarchiveUser(
  slug: string,
  handler: string,
): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void handler;
    return { ok: false, error: "user archive is unavailable in browser mode" };
  }
  try {
    const res = await fetch(
      `${wsBase(slug)}/users/${encodeURIComponent(handler)}/unarchive`,
      { method: "POST" },
    );
    return await res.json();
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export async function startAgent(slug: string, id: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void id;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: { status: "running" } } };
  } catch {
    return mockClient.startAgent(id);
  }
}

export async function stopAgent(slug: string, id: string): Promise<ApiResponse> {
  if (isLocalMode()) {
    void slug;
    void id;
    return { ok: false, error: "agents are unavailable in browser mode" };
  }
  try {
    const res = await fetch(`${wsBase(slug)}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
    const data = await res.json();
    if (!data.ok) return data;
    return { ok: true, data: { agent: { status: "offline" } } };
  } catch {
    return mockClient.stopAgent(id);
  }
}
