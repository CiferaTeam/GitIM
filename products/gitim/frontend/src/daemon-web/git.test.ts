import { beforeEach, describe, expect, it, vi } from "vitest";

const fsMock = vi.hoisted(() => ({}));
const pushMock = vi.hoisted(() => vi.fn(async () => undefined));
const currentBranchMock = vi.hoisted(() => vi.fn(async () => "main"));
const resolveRefMock = vi.hoisted(() => vi.fn(async () => "local-head"));
const writeRefMock = vi.hoisted(() => vi.fn(async () => undefined));

vi.mock("isomorphic-git", () => ({
  default: {
    push: pushMock,
    currentBranch: currentBranchMock,
    resolveRef: resolveRefMock,
    writeRef: writeRefMock,
  },
}));

vi.mock("isomorphic-git/http/web", () => ({
  default: {},
}));

vi.mock("./storage", () => ({
  getFs: () => fsMock,
}));

import { push } from "./git";

describe("daemon-web git operations", () => {
  beforeEach(() => {
    pushMock.mockClear();
    resolveRefMock.mockClear();
    writeRefMock.mockClear();
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
});
