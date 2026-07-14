import { beforeEach, describe, expect, it, vi } from "vitest";

const dirs = vi.hoisted(() => new Set<string>());
const files = vi.hoisted(() => new Map<string, string>());
const gitMocks = vi.hoisted(() => ({
  addAndCommit: vi.fn(async () => "committed-head"),
  addRemoveAndCommit: vi.fn(async () => "committed-head"),
  diffTrees: vi.fn(async () => [] as string[]),
  fetchOrigin: vi.fn(async () => undefined),
  findMergeBase: vi.fn(async () => "base"),
  push: vi.fn(async () => undefined),
  readFileAtCommit: vi.fn(async (...args: [string, string, string]) => {
    if (args.length !== 3) throw new Error("invalid readFileAtCommit call");
    return null as string | null;
  }),
  resetToRemote: vi.fn(async () => undefined),
  resetToCommit: vi.fn(async () => undefined),
  resolveHead: vi.fn(async () => "local-head"),
  resolveRemoteHead: vi.fn(async () => "remote-head"),
}));
const storageMocks = vi.hoisted(() => ({
  removeFile: vi.fn(async (path: string) => {
    files.delete(path);
  }),
  writeFile: vi.fn(async (path: string, content: string) => {
    const parent = path.slice(0, path.lastIndexOf("/")) || "/";
    if (!dirs.has(parent)) {
      throw new Error(`missing parent dir: ${parent}`);
    }
    files.set(path, content);
  }),
}));
const postMessageMock = vi.hoisted(() => vi.fn());

vi.mock("./git", () => gitMocks);

vi.mock("./storage", () => ({
  exists: vi.fn(async (path: string) => dirs.has(path) || files.has(path)),
  mkdir: vi.fn(async (path: string) => {
    dirs.add(path);
  }),
  removeDir: vi.fn(async (path: string) => {
    dirs.delete(path);
  }),
  removeFile: storageMocks.removeFile,
  readFile: vi.fn(async (path: string) => {
    const value = files.get(path);
    if (value === undefined) throw new Error(`missing file: ${path}`);
    return value;
  }),
  writeFile: storageMocks.writeFile,
}));

vi.mock("./auth", () => ({
  tokenAuth: vi.fn((token: string) => ({ username: token })),
}));

vi.mock("./auth-errors", () => ({
  isAuthFailure: vi.fn((e: unknown) =>
    String((e as { message?: string })?.message ?? e).includes("401"),
  ),
}));

import { getState, initState, setState } from "./state";
import { withRepoLock } from "./repo-lock";
import { runSync } from "./sync";
import {
  parseQuickSessionMeta,
  serializeQuickSessionMeta,
} from "gitim-wasm";

const baseThread = "[L000001][P000000][@alice][20260317T120000Z] base\n";
const localThread =
  baseThread +
  "[L000002][P000001][@lewis][20260317T120100Z] local\n";
const remoteThread =
  baseThread +
  "[L000002][P000001][@alice][20260317T120050Z] remote\n";
const localBoard = [
  "---",
  "version: 1",
  "handler: lewis",
  "updated_at: 20260317T120100Z",
  "status: working",
  "summary: local board",
  "tags: []",
  "---",
  "## 当前状态",
  "",
  "local board",
  "",
].join("\n");
const sessionId = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const sessionDir = `/repo/quick-sessions/${sessionId}`;
const sessionMetaPath = `quick-sessions/${sessionId}/session.meta.yaml`;
const sessionThreadPath = `quick-sessions/${sessionId}/discussion.thread`;
const archivedSessionDir = `/repo/archive/quick-sessions/${sessionId}`;
const archivedSessionMetaPath =
  `archive/quick-sessions/${sessionId}/session.meta.yaml`;
const archivedSessionThreadPath =
  `archive/quick-sessions/${sessionId}/discussion.thread`;

function quickSessionMeta(overrides: Record<string, unknown> = {}): string {
  return serializeQuickSessionMeta({
    id: sessionId,
    title_source: "none",
    agent_id: "alice",
    created_by: "lewis",
    status: "needs_title",
    created_at: "20260711T010203Z",
    updated_at: "20260711T010203Z",
    last_message_preview: "base",
    last_human_line: 1,
    last_human_request_id: "request-base",
    revision: 2,
    ...overrides,
  });
}

function seedQuickSession(meta: string, thread: string): void {
  dirs.add("/repo/quick-sessions");
  dirs.add(sessionDir);
  files.set(`${sessionDir}/session.meta.yaml`, meta);
  files.set(`${sessionDir}/discussion.thread`, thread);
}

function configureQuickSessionSendReplay() {
  const baseSessionThread =
    "[L000001][P000000][@lewis][20260711T010203Z] base\n";
  const localSessionThread =
    baseSessionThread +
    "[L000002][P000001][@lewis][20260711T010303Z] local follow-up\n";
  const baseMeta = quickSessionMeta();
  const localMeta = quickSessionMeta({
    updated_at: "20260711T010303Z",
    last_message_preview: "local follow-up",
    last_human_line: 2,
    last_human_request_id: "request-local",
    revision: 3,
  });
  seedQuickSession(localMeta, localSessionThread);
  gitMocks.resolveHead.mockResolvedValue("local-head");
  gitMocks.push
    .mockRejectedValueOnce(new Error("non-fast-forward"))
    .mockResolvedValue(undefined);
  gitMocks.diffTrees.mockResolvedValueOnce([
    sessionMetaPath,
    sessionThreadPath,
  ]);
  gitMocks.readFileAtCommit.mockImplementation(
    async (...args: [string, string, string]) => {
      const path = args[2];
      if (path === sessionMetaPath) return baseMeta;
      if (path === sessionThreadPath) return baseSessionThread;
      return null;
    },
  );
  gitMocks.resetToRemote.mockImplementationOnce(async () => {
    files.set(`${sessionDir}/session.meta.yaml`, baseMeta);
    files.set(`${sessionDir}/discussion.thread`, baseSessionThread);
  });
  gitMocks.resetToCommit.mockImplementation(async () => {
    files.set(`${sessionDir}/session.meta.yaml`, localMeta);
    files.set(`${sessionDir}/discussion.thread`, localSessionThread);
  });
  return { localMeta, localSessionThread };
}

function initSyncState() {
  initState({
    workspaceId: "ws_phone",
    repoDir: "/repo",
    remoteUrl: "https://github.com/flame4/phone",
    fsName: "gitim-ws-phone",
    corsProxy: "https://cors.example",
    token: "token",
    handler: "lewis",
    displayName: "Lewis",
  });
  setState({ headCommit: "base", defaultBranch: "main" });
}

describe("daemon-web sync", () => {
  beforeEach(() => {
    dirs.clear();
    dirs.add("/repo");
    dirs.add("/repo/channels");
    dirs.add("/repo/showboards");
    dirs.add("/repo/showboards/lewis");
    files.clear();
    postMessageMock.mockClear();
    Object.assign(globalThis, { postMessage: postMessageMock });
    vi.spyOn(console, "error").mockImplementation(() => undefined);

    gitMocks.addAndCommit.mockClear();
    gitMocks.addRemoveAndCommit.mockClear();
    gitMocks.diffTrees.mockReset();
    gitMocks.diffTrees.mockResolvedValue([]);
    gitMocks.fetchOrigin.mockClear();
    gitMocks.findMergeBase.mockReset();
    gitMocks.findMergeBase.mockResolvedValue("base");
    gitMocks.push.mockReset();
    gitMocks.push.mockResolvedValue(undefined);
    gitMocks.readFileAtCommit.mockReset();
    gitMocks.readFileAtCommit.mockResolvedValue(null);
    gitMocks.resetToRemote.mockReset();
    gitMocks.resetToRemote.mockResolvedValue(undefined);
    gitMocks.resetToCommit.mockReset();
    gitMocks.resetToCommit.mockResolvedValue(undefined);
    gitMocks.resolveHead.mockReset();
    gitMocks.resolveHead.mockResolvedValue("local-head");
    gitMocks.resolveRemoteHead.mockReset();
    gitMocks.resolveRemoteHead.mockResolvedValue("remote-head");
    storageMocks.removeFile.mockClear();
    storageMocks.removeFile.mockImplementation(async (path: string) => {
      files.delete(path);
    });
    storageMocks.writeFile.mockClear();
    storageMocks.writeFile.mockImplementation(async (path: string, content: string) => {
      const parent = path.slice(0, path.lastIndexOf("/")) || "/";
      if (!dirs.has(parent)) throw new Error(`missing parent dir: ${parent}`);
      files.set(path, content);
    });
    initSyncState();
  });

  it("rebases append-only local thread additions after remote changes", async () => {
    files.set("/repo/channels/general.thread", localThread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce(["channels/general.thread"]);
    gitMocks.readFileAtCommit
      .mockResolvedValueOnce(baseThread)
      .mockResolvedValueOnce(remoteThread);

    const result = await runSync({ forceNewCycle: true });

    expect(files.get("/repo/channels/general.thread")).toBe(
      remoteThread +
      "[L000003][P000001][@lewis][20260317T120100Z] local\n",
    );
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      ["channels/general.thread"],
      "msg: @lewis -> general L000003(rebased)",
      "lewis",
    );
    expect(result).toEqual({
      beforeHead: "base",
      afterHead: "merged-head",
      changed: true,
      status: "rebased",
    });
    expect(getState().headCommit).toBe("merged-head");
    expect(getState().syncStatus).toBe("idle");
  });

  it("returns fast_forwarded and emits repo_changed for remote-only changes", async () => {
    setState({ headCommit: "local-head", defaultBranch: "main" });
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.resolveRemoteHead.mockResolvedValueOnce("remote-head");

    const result = await runSync({ forceNewCycle: true });

    expect(result).toEqual({
      beforeHead: "local-head",
      afterHead: "remote-head",
      changed: true,
      status: "fast_forwarded",
    });
    expect(postMessageMock).toHaveBeenCalledWith({
      type: "repo_changed",
      commit_id: "remote-head",
      reason: "fast_forward",
    });
  });

  it("does not reset while a local writer holds the repo lock", async () => {
    let releaseWriter!: () => void;
    const writer = withRepoLock(
      () =>
        new Promise<void>((resolve) => {
          releaseWriter = resolve;
        }),
    );
    await Promise.resolve();

    setState({ headCommit: "local-head", defaultBranch: "main" });
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.resolveRemoteHead.mockResolvedValueOnce("remote-head");

    const sync = runSync({ forceNewCycle: true });
    await Promise.resolve();

    expect(gitMocks.resetToRemote).not.toHaveBeenCalled();

    releaseWriter();
    await writer;
    await sync;

    expect(gitMocks.resetToRemote).toHaveBeenCalledWith(
      "/repo",
      "refs/remotes/origin/main",
    );
  });

  it("shares an in-flight sync for concurrent non-forced calls", async () => {
    let releaseFetch!: () => void;
    gitMocks.fetchOrigin.mockImplementationOnce(
      () =>
        new Promise<undefined>((resolve) => {
          releaseFetch = () => resolve(undefined);
        }),
    );
    setState({ headCommit: "local-head" });
    gitMocks.resolveHead.mockResolvedValue("local-head");
    gitMocks.resolveRemoteHead.mockResolvedValue("local-head");

    const first = runSync();
    const second = runSync();
    await vi.waitFor(() => {
      expect(gitMocks.fetchOrigin).toHaveBeenCalledTimes(1);
    });
    releaseFetch();

    await expect(Promise.all([first, second])).resolves.toEqual([
      {
        beforeHead: "local-head",
        afterHead: "local-head",
        changed: false,
        status: "idle",
      },
      {
        beforeHead: "local-head",
        afterHead: "local-head",
        changed: false,
        status: "idle",
      },
    ]);
    expect(gitMocks.fetchOrigin).toHaveBeenCalledTimes(1);
  });

  it("advances the sync baseline when remote already has the local head", async () => {
    setState({ headCommit: "base" });
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.push.mockRejectedValueOnce(new Error("non-fast-forward"));
    gitMocks.resolveRemoteHead.mockResolvedValueOnce("local-head");

    const result = await runSync({ forceNewCycle: true });

    expect(result).toEqual({
      beforeHead: "base",
      afterHead: "local-head",
      changed: true,
      status: "idle",
    });
    expect(getState().headCommit).toBe("local-head");
  });

  it("fails safe before reset when local conflicts are not append-only threads", async () => {
    files.set("/repo/channels/general.meta.yaml", "display_name: Local\n");
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.push.mockRejectedValueOnce(new Error("non-fast-forward"));
    gitMocks.diffTrees.mockResolvedValueOnce(["channels/general.meta.yaml"]);
    gitMocks.readFileAtCommit
      .mockResolvedValueOnce("display_name: Base\n")
      .mockResolvedValueOnce("display_name: Remote\n");

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("Cannot auto-merge local browser sync change: channels/general.meta.yaml");

    expect(gitMocks.resetToRemote).not.toHaveBeenCalled();
    expect(gitMocks.addAndCommit).not.toHaveBeenCalled();
    expect(getState().syncStatus).toBe("error");
  });

  it("fails safe before reset when remote deleted a locally appended thread", async () => {
    files.set("/repo/channels/general.thread", localThread);
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.push.mockRejectedValueOnce(new Error("non-fast-forward"));
    gitMocks.diffTrees.mockResolvedValueOnce(["channels/general.thread"]);
    gitMocks.readFileAtCommit
      .mockResolvedValueOnce(baseThread)
      .mockResolvedValueOnce(null);

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("Cannot auto-merge local browser sync change: channels/general.thread");

    expect(gitMocks.resetToRemote).not.toHaveBeenCalled();
    expect(gitMocks.addAndCommit).not.toHaveBeenCalled();
    expect(files.get("/repo/channels/general.thread")).toBe(localThread);
    expect(getState().syncStatus).toBe("error");
  });

  it("fails safe before reset when shallow history has no merge base", async () => {
    files.set("/repo/channels/general.thread", localThread);
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.push.mockRejectedValueOnce(new Error("non-fast-forward"));
    gitMocks.findMergeBase.mockRejectedValueOnce(
      new Error("browser sync history has no unique merge base"),
    );

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("no unique merge base");

    expect(gitMocks.resetToRemote).not.toHaveBeenCalled();
    expect(files.get("/repo/channels/general.thread")).toBe(localThread);
  });

  it("rebases local board commits after remote changes", async () => {
    files.set("/repo/showboards/lewis/board.md", localBoard);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce(["showboards/lewis/board.md"]);

    const result = await runSync({ forceNewCycle: true });

    expect(files.get("/repo/showboards/lewis/board.md")).toBe(localBoard);
    expect(gitMocks.resetToRemote).toHaveBeenCalledWith(
      "/repo",
      "refs/remotes/origin/main",
    );
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      ["showboards/lewis/board.md"],
      "board: sync after rebase",
      "lewis",
    );
    expect(result.status).toBe("rebased");
    expect(getState().headCommit).toBe("merged-head");
    expect(getState().syncStatus).toBe("idle");
  });

  it("recreates board directories after reset for newly-created local boards", async () => {
    files.set("/repo/showboards/lewis/board.md", localBoard);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce(["showboards/lewis/board.md"]);
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      files.delete("/repo/showboards/lewis/board.md");
      dirs.delete("/repo/showboards/lewis");
    });

    const result = await runSync({ forceNewCycle: true });

    expect(dirs.has("/repo/showboards/lewis")).toBe(true);
    expect(files.get("/repo/showboards/lewis/board.md")).toBe(localBoard);
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      ["showboards/lewis/board.md"],
      "board: sync after rebase",
      "lewis",
    );
    expect(result.status).toBe("rebased");
    expect(getState().headCommit).toBe("merged-head");
    expect(getState().syncStatus).toBe("idle");
  });

  it("replays a local quick session transaction after an unrelated remote commit", async () => {
    const localMeta = quickSessionMeta({
      last_message_preview: "new session",
      last_human_request_id: "request-create",
    });
    const localSessionThread =
      "[L000001][P000000][@lewis][20260711T010203Z] new session\n";
    seedQuickSession(localMeta, localSessionThread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      files.delete(`${sessionDir}/session.meta.yaml`);
      files.delete(`${sessionDir}/discussion.thread`);
      dirs.delete(sessionDir);
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.get(`${sessionDir}/session.meta.yaml`)).toBe(localMeta);
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      [sessionMetaPath, sessionThreadPath],
      expect.any(String),
      "lewis",
    );
    expect(result.status).toBe("rebased");
  });

  it("preserves an exact local quick session send after an unrelated remote commit", async () => {
    const baseSessionThread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    const localSessionThread =
      baseSessionThread +
      "[L000002][P000001][@lewis][20260711T010303Z] local follow-up\n";
    const baseMeta = quickSessionMeta();
    const localMeta = quickSessionMeta({
      updated_at: "20260711T010303Z",
      last_message_preview: "local follow-up",
      last_human_line: 2,
      last_human_request_id: "request-local",
      revision: 3,
    });
    seedQuickSession(localMeta, localSessionThread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (...args: [string, string, string]) => {
        const path = args[2];
        if (path === sessionMetaPath) return baseMeta;
        if (path === sessionThreadPath) return baseSessionThread;
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      files.set(`${sessionDir}/session.meta.yaml`, baseMeta);
      files.set(`${sessionDir}/discussion.thread`, baseSessionThread);
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.get(`${sessionDir}/session.meta.yaml`)).toBe(localMeta);
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      [sessionThreadPath, sessionMetaPath],
      expect.any(String),
      "lewis",
    );
    expect(result.status).toBe("rebased");
  });

  it("restores the local quick session commit when replay storage fails", async () => {
    const { localMeta, localSessionThread } = configureQuickSessionSendReplay();
    storageMocks.writeFile.mockRejectedValueOnce(new Error("disk full"));

    await expect(runSync({ forceNewCycle: true })).rejects.toThrow("disk full");

    expect(gitMocks.resetToCommit).toHaveBeenCalledWith("/repo", "local-head");
    expect(files.get(`${sessionDir}/session.meta.yaml`)).toBe(localMeta);
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
    expect(getState().syncStatus).toBe("error");

    const retry = await runSync({ forceNewCycle: true });
    expect(retry.status).toBe("pushed");
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
  });

  it("restores the local quick session commit when replay commit creation fails", async () => {
    const { localMeta, localSessionThread } = configureQuickSessionSendReplay();
    gitMocks.addAndCommit.mockRejectedValueOnce(new Error("commit failed"));

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("commit failed");

    expect(gitMocks.resetToCommit).toHaveBeenCalledWith("/repo", "local-head");
    expect(files.get(`${sessionDir}/session.meta.yaml`)).toBe(localMeta);
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
    expect(getState().syncStatus).toBe("error");
  });

  it("keeps the replay error and appends a transactional restore failure", async () => {
    configureQuickSessionSendReplay();
    const original = new Error("commit failed");
    gitMocks.addAndCommit.mockRejectedValueOnce(original);
    gitMocks.resetToCommit.mockRejectedValueOnce(new Error("checkout failed"));

    const failure = await runSync({ forceNewCycle: true }).catch((error) => error);

    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toContain("commit failed");
    expect((failure as Error).message).toContain(
      "failed to restore local sync state: checkout failed",
    );
    expect((failure as Error & { cause?: unknown }).cause).toBe(original);
    expect(getState().syncStatus).toBe("error");
  });

  it("keeps the replay commit reachable when the following push fails", async () => {
    const { localSessionThread } = configureQuickSessionSendReplay();
    gitMocks.push.mockReset();
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockRejectedValueOnce(new Error("push offline"));

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("push offline");

    expect(gitMocks.addAndCommit).toHaveBeenCalled();
    expect(gitMocks.resetToCommit).not.toHaveBeenCalled();
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
    expect(getState().syncStatus).toBe("error");
  });

  it("recovers the replay base after worker reinitialization and a newer remote commit", async () => {
    const remoteFirst =
      baseThread +
      "[L000002][P000001][@alice][20260317T120050Z] remote first\n";
    const replayedFirst =
      remoteFirst +
      "[L000003][P000001][@lewis][20260317T120100Z] local\n";
    const remoteSecond =
      remoteFirst +
      "[L000003][P000001][@alice][20260317T120200Z] remote second\n";
    files.set("/repo/channels/general.thread", localThread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("replay-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.resolveRemoteHead
      .mockResolvedValueOnce("remote-first-head")
      .mockResolvedValueOnce("remote-second-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockRejectedValueOnce(new Error("push offline"))
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.findMergeBase
      .mockResolvedValueOnce("base")
      .mockResolvedValueOnce("remote-first-head");
    gitMocks.diffTrees.mockResolvedValue(["channels/general.thread"]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (_repo: string, ref: string, path: string) => {
        if (path !== "channels/general.thread") return null;
        if (ref === "base") return baseThread;
        if (ref === "remote-first-head") return remoteFirst;
        if (ref === "remote-second-head") return remoteSecond;
        return null;
      },
    );
    gitMocks.resetToRemote
      .mockImplementationOnce(async () => {
        files.set("/repo/channels/general.thread", remoteFirst);
      })
      .mockImplementationOnce(async () => {
        files.set("/repo/channels/general.thread", remoteSecond);
      });

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("push offline");
    expect(files.get("/repo/channels/general.thread")).toBe(replayedFirst);
    expect(gitMocks.fetchOrigin).toHaveBeenCalledTimes(2);

    // A restarted Worker reconstructs this from the fetched remote ref,
    // which advanced while the replay push was being retried.
    initSyncState();
    setState({ headCommit: "remote-second-head" });

    await expect(runSync({ forceNewCycle: true })).resolves.toMatchObject({
      afterHead: "merged-head",
      status: "rebased",
    });

    expect(gitMocks.diffTrees).toHaveBeenNthCalledWith(
      2,
      "/repo",
      "remote-first-head",
      "replay-head",
    );
    expect(files.get("/repo/channels/general.thread")).toBe(
      remoteSecond +
      "[L000004][P000001][@lewis][20260317T120100Z] local\n",
    );
    expect(gitMocks.findMergeBase).toHaveBeenNthCalledWith(
      2,
      "/repo",
      "replay-head",
      "remote-second-head",
    );
  });

  it("merges concurrent quick session sends and regenerates line metadata", async () => {
    const baseSessionThread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    const localSessionThread =
      baseSessionThread +
      "[L000002][P000001][@lewis][20260711T010303Z] local follow-up\n";
    const remoteSessionThread =
      baseSessionThread +
      "[L000002][P000001][@lewis][20260711T010253Z] remote follow-up\n";
    const baseMeta = quickSessionMeta();
    const localMeta = quickSessionMeta({
      updated_at: "20260711T010303Z",
      last_message_preview: "local follow-up",
      last_human_line: 2,
      last_human_request_id: "request-local",
      revision: 3,
    });
    const remoteMeta = quickSessionMeta({
      updated_at: "20260711T010253Z",
      last_message_preview: "remote follow-up",
      last_human_line: 2,
      last_human_request_id: "request-remote",
      revision: 3,
    });
    seedQuickSession(localMeta, localSessionThread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (_repo: string, ref: string, path: string) => {
        if (path === sessionMetaPath) {
          return ref === "base" ? baseMeta : remoteMeta;
        }
        if (path === sessionThreadPath) {
          return ref === "base" ? baseSessionThread : remoteSessionThread;
        }
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      files.set(`${sessionDir}/session.meta.yaml`, remoteMeta);
      files.set(`${sessionDir}/discussion.thread`, remoteSessionThread);
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(
      remoteSessionThread +
      "[L000003][P000001][@lewis][20260711T010303Z] local follow-up\n",
    );
    expect(parseQuickSessionMeta(files.get(`${sessionDir}/session.meta.yaml`)!))
      .toMatchObject({
        last_message_preview: "local follow-up",
        last_human_line: 3,
        last_human_request_id: "request-local",
        revision: 4,
      });
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      [sessionThreadPath, sessionMetaPath],
      expect.any(String),
      "lewis",
    );
    expect(result.status).toBe("rebased");
  });

  it("fails before reset when the same quick session gets conflicting titles", async () => {
    const baseSessionThread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    const localSessionThread =
      baseSessionThread +
      "[L000002][P000001][@lewis][20260711T010303Z] local follow-up\n";
    const remoteSessionThread =
      baseSessionThread +
      "[L000002][P000001][@lewis][20260711T010253Z] remote follow-up\n";
    const baseMeta = quickSessionMeta();
    const localMeta = quickSessionMeta({
      title: "Local title",
      title_source: "api_set",
      status: "active",
      updated_at: "20260711T010303Z",
      last_message_preview: "local follow-up",
      last_human_line: 2,
      last_human_request_id: "request-local",
      revision: 3,
    });
    const remoteMeta = quickSessionMeta({
      title: "Remote title",
      title_source: "api_set",
      status: "active",
      updated_at: "20260711T010253Z",
      last_message_preview: "remote follow-up",
      last_human_line: 2,
      last_human_request_id: "request-remote",
      revision: 3,
    });
    seedQuickSession(localMeta, localSessionThread);
    gitMocks.resolveHead.mockResolvedValueOnce("local-head");
    gitMocks.push.mockRejectedValueOnce(new Error("non-fast-forward"));
    gitMocks.diffTrees.mockResolvedValueOnce([
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (_repo: string, ref: string, path: string) => {
        if (path === sessionMetaPath) {
          return ref === "base" ? baseMeta : remoteMeta;
        }
        if (path === sessionThreadPath) {
          return ref === "base" ? baseSessionThread : remoteSessionThread;
        }
        return null;
      },
    );

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("concurrent titles differ");

    expect(gitMocks.resetToRemote).not.toHaveBeenCalled();
    expect(files.get(`${sessionDir}/session.meta.yaml`)).toBe(localMeta);
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(localSessionThread);
    expect(gitMocks.addAndCommit).not.toHaveBeenCalled();
  });

  it("replays a quick session archive with unrelated local thread and board changes", async () => {
    const activeMeta = quickSessionMeta();
    const archivedMeta = quickSessionMeta({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "needs_title",
      updated_at: "20260711T010303Z",
      revision: 3,
    });
    const thread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    dirs.add("/repo/archive");
    dirs.add("/repo/archive/quick-sessions");
    dirs.add(archivedSessionDir);
    files.set(`${archivedSessionDir}/session.meta.yaml`, archivedMeta);
    files.set(`${archivedSessionDir}/discussion.thread`, thread);
    files.set("/repo/channels/general.thread", localThread);
    files.set("/repo/showboards/lewis/board.md", localBoard);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-archive-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      archivedSessionMetaPath,
      archivedSessionThreadPath,
      "channels/general.thread",
      sessionMetaPath,
      sessionThreadPath,
      "showboards/lewis/board.md",
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (...args: [string, string, string]) => {
        const ref = args[1];
        const path = args[2];
        if (path === sessionMetaPath) return activeMeta;
        if (path === sessionThreadPath) return thread;
        if (path === "channels/general.thread") {
          return ref === "base" ? baseThread : remoteThread;
        }
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      dirs.add("/repo/quick-sessions");
      dirs.add(sessionDir);
      files.set(`${sessionDir}/session.meta.yaml`, activeMeta);
      files.set(`${sessionDir}/discussion.thread`, thread);
      files.delete(`${archivedSessionDir}/session.meta.yaml`);
      files.delete(`${archivedSessionDir}/discussion.thread`);
      dirs.delete(archivedSessionDir);
      files.set("/repo/channels/general.thread", remoteThread);
      files.delete("/repo/showboards/lewis/board.md");
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.get(`${archivedSessionDir}/session.meta.yaml`)).toBe(archivedMeta);
    expect(files.get(`${archivedSessionDir}/discussion.thread`)).toBe(thread);
    expect(files.has(`${sessionDir}/session.meta.yaml`)).toBe(false);
    expect(files.has(`${sessionDir}/discussion.thread`)).toBe(false);
    expect(files.get("/repo/channels/general.thread")).toBe(
      remoteThread +
      "[L000003][P000001][@lewis][20260317T120100Z] local\n",
    );
    expect(files.get("/repo/showboards/lewis/board.md")).toBe(localBoard);
    expect(gitMocks.addRemoveAndCommit).toHaveBeenCalledWith(
      "/repo",
      [
        "channels/general.thread",
        "showboards/lewis/board.md",
        archivedSessionMetaPath,
        archivedSessionThreadPath,
      ],
      [sessionMetaPath, sessionThreadPath],
      `session: archive ${sessionId} by @lewis`,
      "lewis",
    );
    expect(gitMocks.push).toHaveBeenCalledTimes(2);
    expect(result.status).toBe("rebased");
  });

  it("restores the local quick session archive when replay removal fails", async () => {
    const activeMeta = quickSessionMeta();
    const archivedMeta = quickSessionMeta({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "needs_title",
      updated_at: "20260711T010303Z",
      revision: 3,
    });
    const thread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    dirs.add("/repo/archive");
    dirs.add("/repo/archive/quick-sessions");
    dirs.add(archivedSessionDir);
    files.set(`${archivedSessionDir}/session.meta.yaml`, archivedMeta);
    files.set(`${archivedSessionDir}/discussion.thread`, thread);
    gitMocks.resolveHead.mockResolvedValue("local-archive-head");
    gitMocks.push.mockRejectedValueOnce(new Error("non-fast-forward"));
    gitMocks.diffTrees.mockResolvedValueOnce([
      archivedSessionMetaPath,
      archivedSessionThreadPath,
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (...args: [string, string, string]) => {
        const path = args[2];
        if (path === sessionMetaPath) return activeMeta;
        if (path === sessionThreadPath) return thread;
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      dirs.add("/repo/quick-sessions");
      dirs.add(sessionDir);
      files.set(`${sessionDir}/session.meta.yaml`, activeMeta);
      files.set(`${sessionDir}/discussion.thread`, thread);
      files.delete(`${archivedSessionDir}/session.meta.yaml`);
      files.delete(`${archivedSessionDir}/discussion.thread`);
      dirs.delete(archivedSessionDir);
    });
    gitMocks.resetToCommit.mockImplementationOnce(async () => {
      dirs.add(archivedSessionDir);
      files.set(`${archivedSessionDir}/session.meta.yaml`, archivedMeta);
      files.set(`${archivedSessionDir}/discussion.thread`, thread);
      files.delete(`${sessionDir}/session.meta.yaml`);
      files.delete(`${sessionDir}/discussion.thread`);
      dirs.delete(sessionDir);
    });
    storageMocks.removeFile.mockRejectedValueOnce(new Error("remove failed"));

    await expect(runSync({ forceNewCycle: true }))
      .rejects.toThrow("remove failed");

    expect(gitMocks.resetToCommit).toHaveBeenCalledWith(
      "/repo",
      "local-archive-head",
    );
    expect(files.get(`${archivedSessionDir}/session.meta.yaml`)).toBe(archivedMeta);
    expect(files.get(`${archivedSessionDir}/discussion.thread`)).toBe(thread);
    expect(files.has(`${sessionDir}/session.meta.yaml`)).toBe(false);
    expect(getState().syncStatus).toBe("error");
  });

  it("replays an exact quick session unarchive after an unrelated remote commit", async () => {
    const activeMeta = quickSessionMeta({
      updated_at: "20260711T010403Z",
      revision: 4,
    });
    const archivedMeta = quickSessionMeta({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "needs_title",
      updated_at: "20260711T010303Z",
      revision: 3,
    });
    const thread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    seedQuickSession(activeMeta, thread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-unarchive-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      archivedSessionMetaPath,
      archivedSessionThreadPath,
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (...args: [string, string, string]) => {
        const path = args[2];
        if (path === archivedSessionMetaPath) return archivedMeta;
        if (path === archivedSessionThreadPath) return thread;
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      dirs.add("/repo/archive");
      dirs.add("/repo/archive/quick-sessions");
      dirs.add(archivedSessionDir);
      files.set(`${archivedSessionDir}/session.meta.yaml`, archivedMeta);
      files.set(`${archivedSessionDir}/discussion.thread`, thread);
      files.delete(`${sessionDir}/session.meta.yaml`);
      files.delete(`${sessionDir}/discussion.thread`);
      dirs.delete(sessionDir);
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.get(`${sessionDir}/session.meta.yaml`)).toBe(activeMeta);
    expect(files.get(`${sessionDir}/discussion.thread`)).toBe(thread);
    expect(files.has(`${archivedSessionDir}/session.meta.yaml`)).toBe(false);
    expect(files.has(`${archivedSessionDir}/discussion.thread`)).toBe(false);
    expect(gitMocks.addRemoveAndCommit).toHaveBeenCalledWith(
      "/repo",
      [sessionMetaPath, sessionThreadPath],
      [archivedSessionMetaPath, archivedSessionThreadPath],
      `session: unarchive ${sessionId} by @lewis`,
      "lewis",
    );
    expect(gitMocks.push).toHaveBeenCalledTimes(2);
    expect(result.status).toBe("rebased");
  });

  it("converges a local archive with a remote agent reply", async () => {
    const activeMeta = quickSessionMeta();
    const completedMeta = quickSessionMeta({
      title: "Remote title",
      title_source: "api_set",
      status: "active",
      updated_at: "20260711T010403Z",
      last_message_preview: "agent completion",
      last_completed_attempt_id: "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ",
      last_completed_input_line: 1,
      last_completed_line: 2,
      revision: 4,
    });
    const archivedMeta = quickSessionMeta({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "needs_title",
      updated_at: "20260711T010303Z",
      revision: 3,
    });
    const thread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    const completedThread =
      thread +
      "[L000002][P000001][@alice][20260711T010403Z] agent completion\n";
    dirs.add("/repo/archive");
    dirs.add("/repo/archive/quick-sessions");
    dirs.add(archivedSessionDir);
    files.set(`${archivedSessionDir}/session.meta.yaml`, archivedMeta);
    files.set(`${archivedSessionDir}/discussion.thread`, thread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-archive-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      archivedSessionMetaPath,
      archivedSessionThreadPath,
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (...args: [string, string, string]) => {
        const ref = args[1];
        const path = args[2];
        if (path === sessionMetaPath) {
          if (ref === "base") return activeMeta;
          return completedMeta;
        }
        if (path === sessionThreadPath) {
          return ref === "base" ? thread : completedThread;
        }
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      dirs.add("/repo/quick-sessions");
      dirs.add(sessionDir);
      files.set(`${sessionDir}/session.meta.yaml`, completedMeta);
      files.set(`${sessionDir}/discussion.thread`, completedThread);
      files.delete(`${archivedSessionDir}/session.meta.yaml`);
      files.delete(`${archivedSessionDir}/discussion.thread`);
      dirs.delete(archivedSessionDir);
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.has(`${sessionDir}/session.meta.yaml`)).toBe(false);
    expect(files.has(`${sessionDir}/discussion.thread`)).toBe(false);
    expect(files.get(`${archivedSessionDir}/discussion.thread`)).toBe(
      completedThread,
    );
    const merged = parseQuickSessionMeta(
      files.get(`${archivedSessionDir}/session.meta.yaml`)!,
    ) as Record<string, unknown>;
    expect(merged).toMatchObject({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "active",
      last_completed_attempt_id: "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ",
      last_completed_input_line: 1,
      last_completed_line: 2,
    });
    expect(merged.attempt_id).toBeUndefined();
    expect(merged.processing_input_line).toBeUndefined();
    expect(merged.revision).toBeGreaterThan(4);
    expect(gitMocks.addRemoveAndCommit).toHaveBeenCalled();
    expect(result.status).toBe("rebased");
  });

  it("converges a local agent reply with a remote archive", async () => {
    const activeMeta = quickSessionMeta();
    const archivedMeta = quickSessionMeta({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "needs_title",
      updated_at: "20260711T010303Z",
      revision: 3,
    });
    const completedMeta = quickSessionMeta({
      title: "Local title",
      title_source: "api_set",
      status: "active",
      updated_at: "20260711T010403Z",
      last_message_preview: "agent completion",
      last_completed_attempt_id: "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ",
      last_completed_input_line: 1,
      last_completed_line: 2,
      revision: 4,
    });
    const thread =
      "[L000001][P000000][@lewis][20260711T010203Z] base\n";
    const completedThread =
      thread +
      "[L000002][P000001][@alice][20260711T010403Z] agent completion\n";
    seedQuickSession(completedMeta, completedThread);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-reply-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce([
      sessionMetaPath,
      sessionThreadPath,
    ]);
    gitMocks.readFileAtCommit.mockImplementation(
      async (...args: [string, string, string]) => {
        const ref = args[1];
        const path = args[2];
        if (path === sessionMetaPath) return ref === "base" ? activeMeta : null;
        if (path === sessionThreadPath) return ref === "base" ? thread : null;
        if (path === archivedSessionMetaPath) {
          return ref === "base" ? null : archivedMeta;
        }
        if (path === archivedSessionThreadPath) {
          return ref === "base" ? null : thread;
        }
        return null;
      },
    );
    gitMocks.resetToRemote.mockImplementationOnce(async () => {
      dirs.add("/repo/archive");
      dirs.add("/repo/archive/quick-sessions");
      dirs.add(archivedSessionDir);
      files.set(`${archivedSessionDir}/session.meta.yaml`, archivedMeta);
      files.set(`${archivedSessionDir}/discussion.thread`, thread);
      files.delete(`${sessionDir}/session.meta.yaml`);
      files.delete(`${sessionDir}/discussion.thread`);
      dirs.delete(sessionDir);
    });

    const result = await runSync({ forceNewCycle: true });

    expect(files.has(`${sessionDir}/session.meta.yaml`)).toBe(false);
    expect(files.has(`${sessionDir}/discussion.thread`)).toBe(false);
    expect(files.get(`${archivedSessionDir}/discussion.thread`)).toBe(
      completedThread,
    );
    const merged = parseQuickSessionMeta(
      files.get(`${archivedSessionDir}/session.meta.yaml`)!,
    ) as Record<string, unknown>;
    expect(merged).toMatchObject({
      status: "archived",
      archived_at: "20260711T010303Z",
      archived_from: "active",
      last_completed_attempt_id: "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ",
      last_completed_input_line: 1,
      last_completed_line: 2,
    });
    expect(merged.attempt_id).toBeUndefined();
    expect(merged.processing_input_line).toBeUndefined();
    expect(merged.revision).toBeGreaterThan(4);
    expect(gitMocks.addAndCommit).toHaveBeenCalled();
    expect(result.status).toBe("rebased");
  });

  it("latches a working-tree epoch redirect before attempting any push", async () => {
    seedQuickSession(
      quickSessionMeta(),
      "[L000001][P000000][@lewis][20260711T010203Z] local session\n",
    );
    files.set(
      "/repo/gitim.epoch.yaml",
      "epoch: 1\nstatus: redirected\nnext_branch: main-epoch-2\n",
    );
    gitMocks.resolveHead.mockResolvedValueOnce("local-quick-session-head");

    const result = await runSync({ forceNewCycle: true });

    expect(result).toEqual({
      beforeHead: "base",
      afterHead: "local-quick-session-head",
      changed: true,
      status: "idle",
    });
    expect(getState()).toMatchObject({
      epochRedirected: true,
      syncStatus: "epoch_redirected",
    });
    expect(gitMocks.push).not.toHaveBeenCalled();
    expect(gitMocks.fetchOrigin).not.toHaveBeenCalled();
    expect(gitMocks.resolveRemoteHead).not.toHaveBeenCalled();
  });
});
