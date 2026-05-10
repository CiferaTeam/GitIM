import { beforeEach, describe, expect, it, vi } from "vitest";

const dirs = vi.hoisted(() => new Set<string>());
const files = vi.hoisted(() => new Map<string, string>());
const gitMocks = vi.hoisted(() => ({
  addAndCommit: vi.fn(async () => "committed-head"),
  diffTrees: vi.fn(async () => [] as string[]),
  fetchOrigin: vi.fn(async () => undefined),
  push: vi.fn(async () => undefined),
  readFileAtCommit: vi.fn(async () => null as string | null),
  resetToRemote: vi.fn(async () => undefined),
  resolveHead: vi.fn(async () => "local-head"),
  resolveRemoteHead: vi.fn(async () => "remote-head"),
}));
const postMessageMock = vi.hoisted(() => vi.fn());

vi.mock("./git", () => gitMocks);

vi.mock("./storage", () => ({
  exists: vi.fn(async (path: string) => dirs.has(path) || files.has(path)),
  mkdir: vi.fn(async (path: string) => {
    dirs.add(path);
  }),
  readFile: vi.fn(async (path: string) => {
    const value = files.get(path);
    if (value === undefined) throw new Error(`missing file: ${path}`);
    return value;
  }),
  writeFile: vi.fn(async (path: string, content: string) => {
    const parent = path.slice(0, path.lastIndexOf("/")) || "/";
    if (!dirs.has(parent)) {
      throw new Error(`missing parent dir: ${parent}`);
    }
    files.set(path, content);
  }),
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
import { runSync } from "./sync";

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
    gitMocks.diffTrees.mockReset();
    gitMocks.diffTrees.mockResolvedValue([]);
    gitMocks.fetchOrigin.mockClear();
    gitMocks.push.mockReset();
    gitMocks.push.mockResolvedValue(undefined);
    gitMocks.readFileAtCommit.mockReset();
    gitMocks.readFileAtCommit.mockResolvedValue(null);
    gitMocks.resetToRemote.mockReset();
    gitMocks.resetToRemote.mockResolvedValue(undefined);
    gitMocks.resolveHead.mockReset();
    gitMocks.resolveHead.mockResolvedValue("local-head");
    gitMocks.resolveRemoteHead.mockReset();
    gitMocks.resolveRemoteHead.mockResolvedValue("remote-head");
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

    await runSync({ forceNewCycle: true });

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
    expect(getState().headCommit).toBe("merged-head");
    expect(getState().syncStatus).toBe("idle");
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

  it("rebases local board commits after remote changes", async () => {
    files.set("/repo/showboards/lewis/board.md", localBoard);
    gitMocks.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("merged-head");
    gitMocks.push
      .mockRejectedValueOnce(new Error("non-fast-forward"))
      .mockResolvedValueOnce(undefined);
    gitMocks.diffTrees.mockResolvedValueOnce(["showboards/lewis/board.md"]);

    await runSync({ forceNewCycle: true });

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

    await runSync({ forceNewCycle: true });

    expect(dirs.has("/repo/showboards/lewis")).toBe(true);
    expect(files.get("/repo/showboards/lewis/board.md")).toBe(localBoard);
    expect(gitMocks.addAndCommit).toHaveBeenCalledWith(
      "/repo",
      ["showboards/lewis/board.md"],
      "board: sync after rebase",
      "lewis",
    );
    expect(getState().headCommit).toBe("merged-head");
    expect(getState().syncStatus).toBe("idle");
  });
});
