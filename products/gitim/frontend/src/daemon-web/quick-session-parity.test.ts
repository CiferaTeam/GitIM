import { beforeEach, describe, expect, it, vi } from "vitest";

const files = vi.hoisted(() => new Map<string, string>());
const directories = vi.hoisted(() => new Set<string>());
const commits = vi.hoisted(() => [] as Array<{
  adds: string[];
  removes: string[];
  message: string;
  author: string;
}>);
const stagedPaths = vi.hoisted(() => new Set<string>());
const committedIndexes = vi.hoisted(() => [] as string[][]);
const failures = vi.hoisted(() => ({
  commit: false,
  removePath: null as string | null,
}));
const runSyncMock = vi.hoisted(() => vi.fn(async () => undefined));

function parent(path: string): string | null {
  const index = path.lastIndexOf("/");
  return index <= 0 ? (path.startsWith("/") ? "/" : null) : path.slice(0, index);
}

function ensureDirectory(path: string): void {
  if (directories.has(path)) return;
  const parentPath = parent(path);
  if (parentPath && parentPath !== path) ensureDirectory(parentPath);
  directories.add(path);
}

function children(path: string): string[] {
  const prefix = path === "/" ? "/" : `${path}/`;
  const names = new Set<string>();
  for (const file of files.keys()) {
    if (!file.startsWith(prefix)) continue;
    const rest = file.slice(prefix.length);
    if (rest && !rest.includes("/")) names.add(rest);
  }
  for (const directory of directories) {
    if (!directory.startsWith(prefix)) continue;
    const rest = directory.slice(prefix.length);
    if (rest && !rest.includes("/")) names.add(rest);
  }
  return [...names].sort();
}

vi.mock("./storage", () => ({
  readFile: vi.fn(async (path: string) => {
    const content = files.get(path);
    if (content === undefined) throw new Error(`missing file: ${path}`);
    return content;
  }),
  writeFile: vi.fn(async (path: string, content: string) => {
    const parentPath = parent(path);
    if (parentPath) ensureDirectory(parentPath);
    files.set(path, content);
  }),
  readdir: vi.fn(async (path: string) => children(path)),
  exists: vi.fn(async (path: string) => files.has(path) || directories.has(path)),
  mkdir: vi.fn(async (path: string) => ensureDirectory(path)),
  removeFile: vi.fn(async (path: string) => {
    if (failures.removePath === path) {
      failures.removePath = null;
      throw new Error(`injected unlink failure: ${path}`);
    }
    if (!files.has(path)) throw new Error(`missing file: ${path}`);
    files.delete(path);
  }),
  removeDir: vi.fn(async (path: string) => {
    if (children(path).length > 0) throw new Error(`directory not empty: ${path}`);
    directories.delete(path);
  }),
  configureFs: vi.fn(),
  getActiveFsName: vi.fn(() => "gitim"),
}));

vi.mock("./git", () => ({
  addAndCommit: vi.fn(async (
    _repo: string,
    paths: string[],
    message: string,
    author: string,
  ) => {
    paths.forEach((path) => stagedPaths.add(path));
    if (failures.commit) {
      failures.commit = false;
      throw new Error("injected commit failure");
    }
    commits.push({ adds: paths, removes: [], message, author });
    committedIndexes.push([...stagedPaths].sort());
    stagedPaths.clear();
    return `commit-${commits.length}`;
  }),
  addAndCommitOnly: vi.fn(),
  addRemoveAndCommit: vi.fn(async (
    _repo: string,
    adds: string[],
    removes: string[],
    message: string,
    author: string,
  ) => {
    [...adds, ...removes].forEach((path) => stagedPaths.add(path));
    if (failures.commit) {
      failures.commit = false;
      throw new Error("injected commit failure");
    }
    commits.push({ adds, removes, message, author });
    committedIndexes.push([...stagedPaths].sort());
    stagedPaths.clear();
    return `commit-${commits.length}`;
  }),
  restoreIndexPaths: vi.fn(async (_repo: string, paths: string[]) => {
    paths.forEach((path) => stagedPaths.delete(path));
  }),
  checkout: vi.fn(),
  cloneRepo: vi.fn(),
  diffTrees: vi.fn(async () => []),
  fetchOrigin: vi.fn(),
  getCurrentBranch: vi.fn(async () => "main"),
  getOriginUrl: vi.fn(),
  push: vi.fn(),
  readFileAtCommit: vi.fn(),
  resetToRemote: vi.fn(),
  resolveHead: vi.fn(async () => "head"),
  resolveRemoteHead: vi.fn(async () => "head"),
}));

vi.mock("./sync", () => ({ runSync: runSyncMock }));

import * as handlers from "./handlers";
import { classifyQuickSessionPollChange } from "./quick-session-handlers";
import { initState, setState } from "./state";
import {
  parseQuickSessionMeta,
  serializeQuickSessionMeta,
} from "gitim-wasm";

const SESSION_ID = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const OTHER_SESSION_ID = "qs-01JYYYYYYYYYYYYYYYYYYYYYYY";

type QuickSessionHandlers = typeof handlers & {
  createQuickSession(input: {
    session_id: string;
    agent_id: string;
    first_message: string;
  }): Promise<Record<string, unknown>>;
  listQuickSessions(query?: {
    archived?: boolean;
    agent_id?: string;
    actionable?: boolean;
    limit?: number;
  }): Promise<Record<string, unknown>>;
  readQuickSession(id: string): Promise<Record<string, unknown>>;
  sendQuickSessionMessage(
    id: string,
    input: { body: string; request_id: string },
  ): Promise<Record<string, unknown>>;
  archiveQuickSession(id: string): Promise<Record<string, unknown>>;
  unarchiveQuickSession(id: string): Promise<Record<string, unknown>>;
};

const quick = handlers as QuickSessionHandlers;

function seed(): void {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-07-11T01:02:03Z"));
  files.clear();
  directories.clear();
  commits.length = 0;
  stagedPaths.clear();
  committedIndexes.length = 0;
  failures.commit = false;
  failures.removePath = null;
  runSyncMock.mockReset();
  runSyncMock.mockResolvedValue(undefined);
  ensureDirectory("/repo/users");
  files.set(
    "/repo/users/lewis.meta.yaml",
    "display_name: Lewis\nrole: member\nintroduction: creator\n",
  );
  files.set(
    "/repo/users/alice.meta.yaml",
    "display_name: Alice\nrole: member\nintroduction: agent\n",
  );
  initState({
    workspaceId: "ws",
    repoDir: "/repo",
    remoteUrl: "https://github.com/acme/room",
    fsName: "gitim-ws",
    corsProxy: "",
    token: "token",
    handler: "lewis",
    displayName: "Lewis",
  });
  setState({ headCommit: "base", defaultBranch: "main" });
}

function responseData(response: Record<string, unknown>): Record<string, unknown> {
  expect(response.ok).toBe(true);
  return response.data as Record<string, unknown>;
}

function rustSerializedMeta(meta: unknown): string {
  return serializeQuickSessionMeta(meta);
}

function expectIndexClean(): void {
  expect([...stagedPaths]).toEqual([]);
}

function sessionFile(id: string, archived: boolean, name: string): string {
  return `/repo/${archived ? "archive/quick-sessions" : "quick-sessions"}/${id}/${name}`;
}

describe("daemon-web Quick Session parity", () => {
  beforeEach(seed);

  it("creates the canonical Rust object idempotently from a client id", async () => {
    const input = {
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "hello: browser\n  # preserved literally",
    };

    const created = responseData(await quick.createQuickSession(input));

    expect(created).toMatchObject({
      line_number: 1,
      ref: `session:${SESSION_ID}`,
      session: {
        archived: false,
        meta: {
          id: SESSION_ID,
          agent_id: "alice",
          created_by: "lewis",
          status: "needs_title",
          revision: 2,
        },
      },
    });
    expect(files.get(`/repo/quick-sessions/${SESSION_ID}/discussion.thread`)).toBe(
      [
        "[L000001][P000000][@lewis][20260711T010203Z] hello: browser",
        "  # preserved literally",
        "",
      ].join("\n"),
    );
    const yaml = files.get(`/repo/quick-sessions/${SESSION_ID}/session.meta.yaml`);
    const createdMeta = (created.session as { meta: unknown }).meta;
    expect(yaml).toBe(rustSerializedMeta(createdMeta));
    expect(parseQuickSessionMeta(yaml as string)).toEqual(createdMeta);
    expect(commits).toEqual([
      {
        adds: [
          `quick-sessions/${SESSION_ID}/session.meta.yaml`,
          `quick-sessions/${SESSION_ID}/discussion.thread`,
        ],
        removes: [],
        message: `session: create ${SESSION_ID} for @alice by @lewis`,
        author: "lewis",
      },
    ]);

    const retried = responseData(await quick.createQuickSession(input));
    expect(retried.line_number).toBe(1);
    expect(commits).toHaveLength(1);

    const canonicalRetry = responseData(await quick.createQuickSession({
      ...input,
      first_message: `${input.first_message}\n`,
    }));
    expect(canonicalRetry.line_number).toBe(1);
    expect(commits).toHaveLength(1);

    const collision = await quick.createQuickSession({
      ...input,
      first_message: "different immutable message",
    });
    expect(collision).toMatchObject({
      ok: false,
      error_code: "quick_session_id_collision",
    });
    expect(commits).toHaveLength(1);
  });

  it("uses Rust serde bytes for multiline and special metadata scalars", () => {
    const fixtures = [
      {
        id: SESSION_ID,
        title_source: "none",
        agent_id: "alice",
        created_by: "lewis",
        status: "needs_title",
        created_at: "20260711T010203Z",
        updated_at: "20260711T010203Z",
        last_message_preview: "line one\nline: two # literal",
        last_human_line: 1,
        revision: 2,
      },
      {
        id: OTHER_SESSION_ID,
        title: "Investigate: \"quoted\" value\nnext line",
        title_source: "api_set",
        agent_id: "alice",
        created_by: "lewis",
        status: "active",
        created_at: "20260711T010203Z",
        updated_at: "20260711T020304Z",
        summary: "First paragraph\n\n- key: value\n- # literal marker",
        summary_updated_at: "20260711T020304Z",
        last_message_preview: " leading and trailing ",
        last_human_request_id: "request:with:specials",
        last_human_line: 7,
        revision: 9,
      },
    ];

    const yamlDocuments = fixtures.map((fixture) => rustSerializedMeta(fixture));

    expect(yamlDocuments).toHaveLength(2);
    expect(yamlDocuments[0]).not.toBe(yamlDocuments[1]);
    expect(yamlDocuments.map((yaml) => parseQuickSessionMeta(yaml)))
      .toEqual(fixtures);
  });

  it("validates ids and requires active creator and agent handlers", async () => {
    const invalid = await quick.createQuickSession({
      session_id: "../escape",
      agent_id: "alice",
      first_message: "hello",
    });
    expect(invalid).toMatchObject({ ok: false, error_code: "invalid_quick_session_id" });

    files.delete("/repo/users/alice.meta.yaml");
    ensureDirectory("/repo/archive/users");
    files.set("/repo/archive/users/alice.meta.yaml", "display_name: Alice\n");
    const departed = await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "hello",
    });
    expect(departed).toMatchObject({ ok: false });
    expect(String(departed.error)).toContain("departed");
    expect(commits).toHaveLength(0);
  });

  it("deduplicates request ids and serializes concurrent human sends", async () => {
    await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "first",
    });
    commits.length = 0;

    const sent = responseData(await quick.sendQuickSessionMessage(SESSION_ID, {
      body: "second",
      request_id: "request-2",
    }));
    expect(sent.line_number).toBe(2);

    const duplicate = responseData(await quick.sendQuickSessionMessage(SESSION_ID, {
      body: "must not append",
      request_id: "request-2",
    }));
    expect(duplicate.line_number).toBe(2);
    expect(commits).toHaveLength(1);

    const concurrent = await Promise.all([
      quick.sendQuickSessionMessage(SESSION_ID, {
        body: "third",
        request_id: "request-3",
      }),
      quick.sendQuickSessionMessage(SESSION_ID, {
        body: "fourth",
        request_id: "request-4",
      }),
    ]);
    expect(concurrent.map((response) => responseData(response).line_number)).toEqual([3, 4]);
    expect(files.get(`/repo/quick-sessions/${SESSION_ID}/discussion.thread`)).toBe(
      [
        "[L000001][P000000][@lewis][20260711T010203Z] first",
        "[L000002][P000000][@lewis][20260711T010203Z] second",
        "[L000003][P000000][@lewis][20260711T010203Z] third",
        "[L000004][P000000][@lewis][20260711T010203Z] fourth",
        "",
      ].join("\n"),
    );
    expect(commits).toHaveLength(3);
  });

  it("lists, reads, archives, and restores the creator-owned object", async () => {
    await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "first",
    });

    expect(responseData(await quick.listQuickSessions()).sessions).toHaveLength(1);
    const archived = responseData(await quick.archiveQuickSession(SESSION_ID));
    expect(archived).toMatchObject({
      session_id: SESSION_ID,
      status: "archived",
      revision: 3,
      archived_at: "20260711T010203Z",
    });
    expect(responseData(await quick.listQuickSessions()).sessions).toEqual([]);
    expect(responseData(await quick.listQuickSessions({ archived: true })).sessions)
      .toEqual([expect.objectContaining({ id: SESSION_ID, archived: true })]);
    expect(responseData(await quick.readQuickSession(SESSION_ID)).session).toMatchObject({
      archived: true,
      meta: { status: "archived" },
    });
    const archivedMetaChange = await classifyQuickSessionPollChange(
      `archive/quick-sessions/${SESSION_ID}/session.meta.yaml`,
    );
    const archivedThreadChange = await classifyQuickSessionPollChange(
      `archive/quick-sessions/${SESSION_ID}/discussion.thread`,
    );
    expect(archivedMetaChange).toMatchObject({
      kind: "quick_session_meta",
      entries: [expect.not.objectContaining({ recipients: expect.anything() })],
    });
    expect(archivedThreadChange).toMatchObject({
      kind: "quick_session_thread",
      entries: [expect.not.objectContaining({ recipients: expect.anything() })],
    });
    expect(await quick.sendQuickSessionMessage(SESSION_ID, {
      body: "blocked while archived",
      request_id: "request-2",
    })).toMatchObject({ ok: false, error_code: "quick_session_invalid_state" });

    const restored = responseData(await quick.unarchiveQuickSession(SESSION_ID));
    expect(restored).toMatchObject({
      session_id: SESSION_ID,
      status: "needs_title",
      revision: 4,
    });
    expect(responseData(await quick.listQuickSessions()).sessions)
      .toEqual([expect.objectContaining({ id: SESSION_ID, archived: false })]);

    setState({ me: { handler: "alice", display_name: "Alice" } });
    const forbidden = await quick.archiveQuickSession(SESSION_ID);
    expect(forbidden).toMatchObject({
      ok: false,
      error_code: "quick_session_forbidden",
    });
  });

  it("restores worktree and index after every Quick Session commit failure", async () => {
    failures.commit = true;
    expect(await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "create fails",
    })).toMatchObject({ ok: false, error: "injected commit failure" });
    expect(files.has(sessionFile(SESSION_ID, false, "session.meta.yaml"))).toBe(false);
    expect(files.has(sessionFile(SESSION_ID, false, "discussion.thread"))).toBe(false);
    expectIndexClean();

    await quick.createQuickSession({
      session_id: OTHER_SESSION_ID,
      agent_id: "alice",
      first_message: "unrelated succeeds",
    });
    expect(committedIndexes.at(-1)).toEqual([
      `quick-sessions/${OTHER_SESSION_ID}/discussion.thread`,
      `quick-sessions/${OTHER_SESSION_ID}/session.meta.yaml`,
    ]);

    await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "original",
    });
    const activeMetaPath = sessionFile(SESSION_ID, false, "session.meta.yaml");
    const activeThreadPath = sessionFile(SESSION_ID, false, "discussion.thread");
    const originalActiveMeta = files.get(activeMetaPath);
    const originalActiveThread = files.get(activeThreadPath);

    failures.commit = true;
    expect(await quick.sendQuickSessionMessage(SESSION_ID, {
      body: "must roll back",
      request_id: "request-failed",
    })).toMatchObject({ ok: false, error: "injected commit failure" });
    expect(files.get(activeMetaPath)).toBe(originalActiveMeta);
    expect(files.get(activeThreadPath)).toBe(originalActiveThread);
    expectIndexClean();

    failures.commit = true;
    expect(await quick.archiveQuickSession(SESSION_ID)).toMatchObject({
      ok: false,
      error: "injected commit failure",
    });
    expect(files.get(activeMetaPath)).toBe(originalActiveMeta);
    expect(files.get(activeThreadPath)).toBe(originalActiveThread);
    expect(files.has(sessionFile(SESSION_ID, true, "session.meta.yaml"))).toBe(false);
    expect(files.has(sessionFile(SESSION_ID, true, "discussion.thread"))).toBe(false);
    expectIndexClean();

    await quick.archiveQuickSession(SESSION_ID);
    const archivedMetaPath = sessionFile(SESSION_ID, true, "session.meta.yaml");
    const archivedThreadPath = sessionFile(SESSION_ID, true, "discussion.thread");
    const originalArchivedMeta = files.get(archivedMetaPath);
    const originalArchivedThread = files.get(archivedThreadPath);

    failures.commit = true;
    expect(await quick.unarchiveQuickSession(SESSION_ID)).toMatchObject({
      ok: false,
      error: "injected commit failure",
    });
    expect(files.get(archivedMetaPath)).toBe(originalArchivedMeta);
    expect(files.get(archivedThreadPath)).toBe(originalArchivedThread);
    expect(files.has(activeMetaPath)).toBe(false);
    expect(files.has(activeThreadPath)).toBe(false);
    expectIndexClean();

    await quick.unarchiveQuickSession(SESSION_ID);
    expect(committedIndexes.at(-1)).toEqual([
      `archive/quick-sessions/${SESSION_ID}/discussion.thread`,
      `archive/quick-sessions/${SESSION_ID}/session.meta.yaml`,
      `quick-sessions/${SESSION_ID}/discussion.thread`,
      `quick-sessions/${SESSION_ID}/session.meta.yaml`,
    ]);
  });

  it("rolls archive and unarchive back when source cleanup fails", async () => {
    await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "original",
    });
    const activeMetaPath = sessionFile(SESSION_ID, false, "session.meta.yaml");
    const activeThreadPath = sessionFile(SESSION_ID, false, "discussion.thread");
    const originalActiveMeta = files.get(activeMetaPath);
    const originalActiveThread = files.get(activeThreadPath);

    failures.removePath = activeThreadPath;
    expect(await quick.archiveQuickSession(SESSION_ID)).toMatchObject({
      ok: false,
      error: `injected unlink failure: ${activeThreadPath}`,
    });
    expect(files.get(activeMetaPath)).toBe(originalActiveMeta);
    expect(files.get(activeThreadPath)).toBe(originalActiveThread);
    expect(files.has(sessionFile(SESSION_ID, true, "session.meta.yaml"))).toBe(false);
    expect(files.has(sessionFile(SESSION_ID, true, "discussion.thread"))).toBe(false);
    expect(commits).toHaveLength(1);
    expectIndexClean();

    await quick.archiveQuickSession(SESSION_ID);
    const archivedMetaPath = sessionFile(SESSION_ID, true, "session.meta.yaml");
    const archivedThreadPath = sessionFile(SESSION_ID, true, "discussion.thread");
    const originalArchivedMeta = files.get(archivedMetaPath);
    const originalArchivedThread = files.get(archivedThreadPath);

    failures.removePath = archivedThreadPath;
    expect(await quick.unarchiveQuickSession(SESSION_ID)).toMatchObject({
      ok: false,
      error: `injected unlink failure: ${archivedThreadPath}`,
    });
    expect(files.get(archivedMetaPath)).toBe(originalArchivedMeta);
    expect(files.get(archivedThreadPath)).toBe(originalArchivedThread);
    expect(files.has(activeMetaPath)).toBe(false);
    expect(files.has(activeThreadPath)).toBe(false);
    expect(commits).toHaveLength(2);
    expectIndexClean();
  });

  it("preserves Rust-serialized title and summary bytes across archive", async () => {
    const created = responseData(await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "first",
    }));
    const activeMetaPath = sessionFile(SESSION_ID, false, "session.meta.yaml");
    const activeMeta = {
      ...(created.session as { meta: Record<string, unknown> }).meta,
      title: "Title: \"quoted\"\ncontinued",
      title_source: "api_set",
      status: "active",
      summary: "Summary line\n\n- key: value\n- # literal",
      summary_updated_at: "20260711T010203Z",
      last_message_preview: "preview: value\n# literal",
      revision: 7,
    };
    files.set(activeMetaPath, rustSerializedMeta(activeMeta));

    await quick.archiveQuickSession(SESSION_ID);

    const archivedMetaPath = sessionFile(SESSION_ID, true, "session.meta.yaml");
    const archivedYaml = files.get(archivedMetaPath) as string;
    const archivedMeta = parseQuickSessionMeta(archivedYaml);
    expect(archivedMeta).toMatchObject({
      title: activeMeta.title,
      summary: activeMeta.summary,
      last_message_preview: activeMeta.last_message_preview,
      status: "archived",
    });
    expect(archivedYaml).toBe(rustSerializedMeta(archivedMeta));
  });

  it("honors the epoch fence and reports a committed reconnect result", async () => {
    setState({ epochRedirected: true });
    const fenced = await quick.createQuickSession({
      session_id: SESSION_ID,
      agent_id: "alice",
      first_message: "first",
    });
    expect(fenced).toMatchObject({ ok: false, error_code: "epoch_redirected" });
    expect(commits).toHaveLength(0);

    setState({ epochRedirected: false });
    runSyncMock.mockRejectedValueOnce(new Error("HTTP Error: 401 Unauthorized"));
    const committed = await quick.createQuickSession({
      session_id: OTHER_SESSION_ID,
      agent_id: "alice",
      first_message: "committed locally",
    });
    expect(committed).toMatchObject({
      ok: true,
      data: {
        status: "commit_only",
        error_code: "reconnect_required",
        needs_token: true,
      },
    });
  });
});
