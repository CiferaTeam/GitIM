import { beforeEach, describe, expect, it, vi } from "vitest";

const fsMock = vi.hoisted(() => ({}));
const pushMock = vi.hoisted(() => vi.fn(async () => undefined));
const currentBranchMock = vi.hoisted(() => vi.fn(async () => "main"));
const resolveRefMock = vi.hoisted(() => vi.fn(async () => "local-head"));
const writeRefMock = vi.hoisted(() => vi.fn(async () => undefined));
const readBlobMock = vi.hoisted(() => vi.fn());
const walkMock = vi.hoisted(() => vi.fn());
const treeMock = vi.hoisted(() => vi.fn((input: unknown) => input));

vi.mock("isomorphic-git", () => ({
  default: {
    push: pushMock,
    currentBranch: currentBranchMock,
    readBlob: readBlobMock,
    resolveRef: resolveRefMock,
    TREE: treeMock,
    walk: walkMock,
    writeRef: writeRefMock,
  },
}));

vi.mock("isomorphic-git/http/web", () => ({
  default: {},
}));

vi.mock("./storage", () => ({
  getFs: () => fsMock,
}));

import { diffTrees, push, readFileAtCommit } from "./git";

function entry(type: "blob" | "tree", oid: string) {
  return {
    type: vi.fn(async () => type),
    oid: vi.fn(async () => oid),
  };
}

describe("daemon-web git operations", () => {
  beforeEach(() => {
    pushMock.mockClear();
    resolveRefMock.mockClear();
    writeRefMock.mockClear();
    readBlobMock.mockReset();
    walkMock.mockReset();
    treeMock.mockClear();
    currentBranchMock.mockReset();
    currentBranchMock.mockResolvedValue("main");
  });

  it("passes the current branch as the push ref", async () => {
    const onAuth = vi.fn();

    await push("/repo", "https://cors.example", onAuth);

    expect(pushMock).toHaveBeenCalledWith(
      expect.objectContaining({
        fs: fsMock,
        dir: "/repo",
        corsProxy: "https://cors.example",
        onAuth,
        remote: "origin",
        ref: "main",
      }),
    );
  });

  it("points the pushed branch at HEAD before pushing", async () => {
    const onAuth = vi.fn();

    await push("/repo", "https://cors.example", onAuth, "trunk");

    expect(resolveRefMock).toHaveBeenCalledWith({
      fs: fsMock,
      dir: "/repo",
      ref: "HEAD",
    });
    expect(writeRefMock).toHaveBeenCalledWith({
      fs: fsMock,
      dir: "/repo",
      ref: "refs/heads/trunk",
      value: "local-head",
      force: true,
    });
    expect(pushMock).toHaveBeenCalledWith(
      expect.objectContaining({ ref: "trunk" }),
    );
  });

  it("returns only changed file entries from tree diffs", async () => {
    walkMock.mockImplementation(async ({ map }) => {
      await map(".", [undefined, undefined]);
      await map("channels", [entry("tree", "old-tree"), entry("tree", "new-tree")]);
      await map(
        "channels/general.thread",
        [entry("blob", "old-thread"), entry("blob", "new-thread")],
      );
      await map(
        "users/alice.meta.yaml",
        [entry("blob", "same-meta"), entry("blob", "same-meta")],
      );
    });

    await expect(diffTrees("/repo", "old", "new")).resolves.toEqual([
      "channels/general.thread",
    ]);
  });

  it("reads text content from a commit", async () => {
    readBlobMock.mockResolvedValueOnce({
      blob: new TextEncoder().encode("hello\n"),
    });

    await expect(readFileAtCommit("/repo", "base", "channels/general.thread"))
      .resolves.toBe("hello\n");
    expect(readBlobMock).toHaveBeenCalledWith({
      fs: fsMock,
      dir: "/repo",
      oid: "base",
      filepath: "channels/general.thread",
    });
  });

  it("returns null only when a file is missing at the commit", async () => {
    const missing = new Error("missing");
    missing.name = "NotFoundError";
    readBlobMock.mockRejectedValueOnce(missing);

    await expect(readFileAtCommit("/repo", "base", "missing.thread"))
      .resolves.toBeNull();
  });

  it("propagates non-missing read errors from a commit", async () => {
    readBlobMock.mockRejectedValueOnce(new Error("corrupt object"));

    await expect(readFileAtCommit("/repo", "base", "channels/general.thread"))
      .rejects.toThrow("corrupt object");
  });
});
