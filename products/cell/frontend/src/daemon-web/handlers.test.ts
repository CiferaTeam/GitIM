import { beforeEach, describe, expect, it, vi } from "vitest";

const files = vi.hoisted(() => new Map<string, string>());
const dirs = vi.hoisted(() => new Map<string, string[]>());
const commits = vi.hoisted(() => [] as Array<{ filepaths: string[]; message: string }>);

vi.mock("./storage", () => ({
  readFile: vi.fn(async (path: string) => {
    const content = files.get(path);
    if (content === undefined) throw new Error(`missing file: ${path}`);
    return content;
  }),
  writeFile: vi.fn(async (path: string, content: string) => {
    files.set(path, content);
  }),
  readdir: vi.fn(async (path: string) => dirs.get(path) ?? []),
  exists: vi.fn(async (path: string) => files.has(path) || dirs.has(path)),
  mkdir: vi.fn(async (path: string) => {
    if (!dirs.has(path)) dirs.set(path, []);
  }),
}));

vi.mock("./git", () => ({
  addAndCommit: vi.fn(async (_dir: string, filepaths: string[], message: string) => {
    commits.push({ filepaths, message });
    return "new-head";
  }),
  checkout: vi.fn(async () => undefined),
  cloneRepo: vi.fn(async () => undefined),
  diffTrees: vi.fn(async () => []),
  fetchOrigin: vi.fn(async () => undefined),
  getCurrentBranch: vi.fn(async () => "main"),
  push: vi.fn(async () => undefined),
  resetToRemote: vi.fn(async () => undefined),
  resolveHead: vi.fn(async () => "head"),
  resolveRemoteHead: vi.fn(async () => "head"),
}));

vi.mock("./sync", () => ({
  runSync: vi.fn(async () => undefined),
}));

import { channels, joinChannel, read, send, thread } from "./handlers";
import { initState, setState } from "./state";

const generalThread =
  "[L000001][P000000][@alice][20260317T120000Z] hello\n" +
  "[L000002][P000001][@lewis][20260317T120100Z] reply\n";

const dmThread =
  "[L000001][P000000][@alice][20260317T120000Z] private\n";

function seedState() {
  files.clear();
  dirs.clear();
  commits.length = 0;

  dirs.set("/repo/channels", ["general.meta.yaml", "general.thread"]);
  dirs.set("/repo/dm", ["alice--lewis.thread"]);
  dirs.set("/repo/users", ["alice.meta.yaml", "lewis.meta.yaml"]);

  files.set(
    "/repo/channels/general.meta.yaml",
    [
      "display_name: General",
      "created_by: alice",
      "created_at: 20260317T120000Z",
      "introduction: Team chat",
      "members:",
      "  - alice",
      "  - lewis",
      "",
    ].join("\n"),
  );
  files.set("/repo/channels/general.thread", generalThread);
  files.set("/repo/dm/alice--lewis.thread", dmThread);

  initState({
    repoDir: "/repo",
    corsProxy: "",
    token: "token",
    handler: "lewis",
    displayName: "Lewis",
  });
  setState({ defaultBranch: "main", headCommit: "base" });
}

describe("daemon-web handlers", () => {
  beforeEach(seedState);

  it("lists channels from channels/*.meta.yaml and dms from dm/*.thread", async () => {
    const res = await channels();

    expect(res.ok).toBe(true);
    expect(res.data?.channels).toEqual([
      {
        name: "general",
        kind: "channel",
        unreadCount: 0,
        members: ["alice", "lewis"],
      },
      {
        name: "alice--lewis",
        kind: "dm",
        unreadCount: 0,
        members: ["alice", "lewis"],
      },
    ]);
  });

  it("returns entries from read to match the runtime API", async () => {
    const res = await read("general", 1);

    expect(res.ok).toBe(true);
    expect(res.data?.entries).toEqual([
      expect.objectContaining({
        line_number: 2,
        point_to: 1,
        author: "lewis",
        body: "reply",
      }),
    ]);
    expect(res.data).not.toHaveProperty("messages");
  });

  it("resolves dm: API names to dm/*.thread", async () => {
    const res = await read("dm:alice,lewis");

    expect(res.ok).toBe(true);
    expect(res.data?.entries).toEqual([
      expect.objectContaining({
        line_number: 1,
        author: "alice",
        body: "private",
      }),
    ]);
  });

  it("returns entries from thread to match the runtime API", async () => {
    const res = await thread("general", 1);

    expect(res.ok).toBe(true);
    expect(res.data?.entries).toHaveLength(2);
    expect(res.data).not.toHaveProperty("messages");
  });

  it("updates channels/<name>.meta.yaml when joining a channel", async () => {
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: alice",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "",
      ].join("\n"),
    );

    const res = await joinChannel("general");

    expect(res.ok).toBe(true);
    expect(files.get("/repo/channels/general.meta.yaml")).toContain("  - lewis\n");
    expect(commits[0].filepaths).toEqual([
      "channels/general.meta.yaml",
      "channels/general.thread",
    ]);
  });

  it("rejects invalid channel names before writing files", async () => {
    const res = await send("../evil", "bad");

    expect(res.ok).toBe(false);
    expect(res.error).toContain("invalid channel name");
    expect(files.has("/repo/channels/../evil.thread")).toBe(false);
  });
});
