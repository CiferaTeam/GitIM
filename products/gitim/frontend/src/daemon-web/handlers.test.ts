import { beforeEach, describe, expect, it, vi } from "vitest";

const files = vi.hoisted(() => new Map<string, string>());
const dirs = vi.hoisted(() => new Map<string, string[]>());
const commits = vi.hoisted(() => [] as Array<{ filepaths: string[]; message: string }>);
const runSyncMock = vi.hoisted(() => vi.fn(async () => undefined));
const activeFsName = vi.hoisted(() => ({ value: "gitim" }));
const readdirFailures = vi.hoisted(() => new Map<string, string>());

function parentDir(path: string): string | null {
  const idx = path.lastIndexOf("/");
  if (idx <= 0) return path.startsWith("/") ? "/" : null;
  return path.slice(0, idx);
}

function basename(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx < 0 ? path : path.slice(idx + 1);
}

function registerDir(path: string): void {
  if (!dirs.has(path)) dirs.set(path, []);
  const parent = parentDir(path);
  if (!parent || parent === path) return;
  const entries = dirs.get(parent);
  if (entries && !entries.includes(basename(path))) entries.push(basename(path));
}

function registerFile(path: string): void {
  const parent = parentDir(path);
  if (!parent) return;
  const entries = dirs.get(parent);
  if (entries && !entries.includes(basename(path))) entries.push(basename(path));
}

function unregisterPath(path: string): void {
  const parent = parentDir(path);
  if (!parent) return;
  const entries = dirs.get(parent);
  if (!entries) return;
  dirs.set(parent, entries.filter((entry) => entry !== basename(path)));
}

vi.mock("gitim-wasm", () => ({
  default: vi.fn(async () => undefined),
  appendBoardSection: vi.fn((doc: Record<string, unknown>, section: string, value: string) => ({
    ...doc,
    body: `${doc.body as string}\n## ${section}\n\n${value}\n`,
  })),
  defaultBoard: vi.fn((handler: string, timestamp: string) => ({
    meta: {
      version: 1,
      handler,
      updated_at: timestamp,
      status: "idle",
      summary: "",
      tags: [],
    },
    body: "## 当前状态\n\n## 关注事项\n",
  })),
  parseCardMeta: vi.fn((yaml: string) => {
    const out: Record<string, unknown> = { labels: [], assignee: null };
    let listKey: string | null = null;
    for (const rawLine of yaml.split("\n")) {
      const line = rawLine.trim();
      if (!line) continue;
      if (line.startsWith("- ") && listKey) {
        (out[listKey] as string[]).push(line.slice(2));
        continue;
      }
      const idx = line.indexOf(":");
      if (idx < 0) continue;
      const key = line.slice(0, idx);
      const value = line.slice(idx + 1).trim().replace(/^["']|["']$/g, "");
      if (value === "") {
        listKey = key;
        out[key] = [];
      } else if (value === "null") {
        listKey = null;
        out[key] = null;
      } else {
        listKey = null;
        out[key] = value;
      }
    }
    return out;
  }),
  parseBoardMarkdown: vi.fn((markdown: string) => {
    const match = markdown.match(/^---\n([\s\S]*?)---\n([\s\S]*)$/);
    if (!match) throw new Error("invalid board");
    const meta = {} as Record<string, unknown>;
    let listKey: string | null = null;
    for (const rawLine of match[1].split("\n")) {
      const line = rawLine.trim();
      if (!line) continue;
      if (line.startsWith("- ") && listKey) {
        (meta[listKey] as string[]).push(line.slice(2));
        continue;
      }
      const idx = line.indexOf(":");
      if (idx < 0) continue;
      const key = line.slice(0, idx);
      const value = line.slice(idx + 1).trim().replace(/^["']|["']$/g, "");
      if (value === "[]") {
        meta[key] = [];
        listKey = null;
      } else if (value === "") {
        meta[key] = [];
        listKey = key;
      } else if (key === "version") {
        meta[key] = Number(value);
        listKey = null;
      } else {
        meta[key] = value;
        listKey = null;
      }
    }
    return { meta, body: match[2] };
  }),
  setBoardField: vi.fn((doc: Record<string, unknown>, field: string, value: string) => ({
    ...doc,
    meta: {
      ...(doc.meta as Record<string, unknown>),
      [field]: field === "tags" ? value.split(",").map((tag) => tag.trim()).filter(Boolean) : value,
    },
  })),
  setBoardSection: vi.fn((doc: Record<string, unknown>, section: string, value: string) => ({
    ...doc,
    body: `## ${section}\n\n${value}\n`,
  })),
  stringifyBoardMarkdown: vi.fn((doc: Record<string, unknown>) => {
    const meta = doc.meta as Record<string, unknown>;
    const tags = Array.isArray(meta.tags) ? meta.tags : [];
    return [
      "---",
      `version: ${meta.version}`,
      `handler: ${meta.handler}`,
      `updated_at: ${meta.updated_at}`,
      `status: ${meta.status}`,
      `summary: ${meta.summary}`,
      "tags:",
      ...tags.map((tag) => `  - ${tag}`),
      "---",
      doc.body as string,
    ].join("\n");
  }),
  stringifyCardMeta: vi.fn((meta: Record<string, unknown>) => {
    const labels = Array.isArray(meta.labels) ? meta.labels : [];
    const lines = [
      `title: ${meta.title}`,
      `channel: ${meta.channel}`,
      `status: ${meta.status}`,
      "labels:",
      ...labels.map((label) => `  - ${label}`),
      `assignee: ${meta.assignee ?? "null"}`,
      `created_by: ${meta.created_by}`,
      `created_at: ${meta.created_at}`,
      `updated_at: ${meta.updated_at}`,
    ];
    if (meta.archived_via !== undefined) {
      lines.push(`archived_via: ${meta.archived_via}`);
    }
    lines.push("");
    return lines.join("\n");
  }),
  validateCardId: vi.fn((cardId: string) => {
    if (!/^[0-9a-f-]{1,20}$/.test(cardId)) throw new Error("invalid card_id");
  }),
  validateCardLabels: vi.fn((labels: string[]) => {
    for (const label of labels) {
      if (!/^[a-z0-9_-]{1,32}$/.test(label)) throw new Error("invalid label");
    }
  }),
  validateCardMeta: vi.fn(() => undefined),
}));

vi.mock("./storage", () => ({
  readFile: vi.fn(async (path: string) => {
    const content = files.get(path);
    if (content === undefined) throw new Error(`missing file: ${path}`);
    return content;
  }),
  writeFile: vi.fn(async (path: string, content: string) => {
    files.set(path, content);
    registerFile(path);
  }),
  readdir: vi.fn(async (path: string) => {
    const failure = readdirFailures.get(path);
    if (failure) throw new Error(failure);
    return dirs.get(path) ?? [];
  }),
  exists: vi.fn(async (path: string) => files.has(path) || dirs.has(path)),
  mkdir: vi.fn(async (path: string) => {
    registerDir(path);
  }),
  removeFile: vi.fn(async (path: string) => {
    files.delete(path);
    unregisterPath(path);
  }),
  removeDir: vi.fn(async (path: string) => {
    dirs.delete(path);
    unregisterPath(path);
  }),
  configureFs: vi.fn((fsName: string) => {
    activeFsName.value = fsName;
  }),
  getActiveFsName: vi.fn(() => activeFsName.value),
}));

vi.mock("./git", () => ({
  addAndCommit: vi.fn(async (_dir: string, filepaths: string[], message: string) => {
    commits.push({ filepaths, message });
    return "new-head";
  }),
  addAndCommitOnly: vi.fn(async (_dir: string, filepath: string, message: string) => {
    commits.push({ filepaths: [filepath], message });
    return "new-head";
  }),
  addRemoveAndCommit: vi.fn(async (
    _dir: string,
    addFilepaths: string[],
    removeFilepaths: string[],
    message: string,
  ) => {
    for (const filepath of removeFilepaths) {
      files.delete(`/repo/${filepath}`);
      unregisterPath(`/repo/${filepath}`);
    }
    commits.push({ filepaths: [...addFilepaths, ...removeFilepaths], message });
    return "new-head";
  }),
  checkout: vi.fn(async () => undefined),
  cloneRepo: vi.fn(async () => undefined),
  diffTrees: vi.fn(async () => []),
  fetchOrigin: vi.fn(async () => undefined),
  getCurrentBranch: vi.fn(async () => "main"),
  getOriginUrl: vi.fn(async () => undefined),
  push: vi.fn(async () => undefined),
  readFileAtCommit: vi.fn(async () => null),
  resetToRemote: vi.fn(async () => undefined),
  resolveHead: vi.fn(async () => "head"),
  resolveRemoteHead: vi.fn(async () => "head"),
}));

vi.mock("./sync", () => ({
  runSync: runSyncMock,
}));

import {
  archiveChannel,
  archiveCard,
  appendBoardSectionValue,
  channels,
  createCard,
  initBoard,
  init,
  listArchivedChannels,
  listArchivedCards,
  listBoards,
  listCards,
  poll,
  publishBoard,
  read,
  readCard,
  send,
  sendCardMessage,
  setBoard,
  setBoardSectionValue,
  showBoard,
  thread,
  unarchiveChannel,
  updateCard,
  joinChannel,
  listArchivedDms,
  unarchiveCard,
  reconcileOrphanCards,
} from "./handlers";
import { getState, initState, setState } from "./state";
import { getActiveFsName } from "./storage";
import { withRepoLock } from "./repo-lock";

const generalThread =
  "[L000001][P000000][@alice][20260317T120000Z] hello\n" +
  "[L000002][P000001][@lewis][20260317T120100Z] reply\n";

const dmThread =
  "[L000001][P000000][@alice][20260317T120000Z] private\n";

function seedState() {
  files.clear();
  dirs.clear();
  commits.length = 0;
  activeFsName.value = "gitim";
  readdirFailures.clear();
  runSyncMock.mockReset();
  runSyncMock.mockResolvedValue(undefined);

  dirs.set("/repo/channels", ["general.meta.yaml", "general.thread"]);
  dirs.set("/repo/channels/general/cards", ["20260317-120000-abc"]);
  dirs.set("/repo/channels/general/cards/20260317-120000-abc", [
    "card.meta.yaml",
    "discussion.thread",
  ]);
  dirs.set("/repo/dm", ["alice--lewis.thread", "cfo--flame4.thread"]);
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
  files.set(
    "/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml",
    [
      "title: Existing card",
      "channel: general",
      "status: todo",
      "labels:",
      "  - mobile",
      "assignee: lewis",
      "created_by: alice",
      "created_at: 20260317T120000Z",
      "updated_at: 20260317T120000Z",
      "",
    ].join("\n"),
  );
  files.set(
    "/repo/channels/general/cards/20260317-120000-abc/discussion.thread",
    "[L000001][P000000][@alice][20260317T120000Z] card note\n",
  );
  files.set(
    "/repo/users/alice.meta.yaml",
    "display_name: Alice\nrole: member\nintroduction: hi\n",
  );
  files.set(
    "/repo/users/lewis.meta.yaml",
    "display_name: Lewis\nrole: member\nintroduction: hi\n",
  );
  files.set("/repo/dm/alice--lewis.thread", dmThread);
  files.set("/repo/dm/cfo--flame4.thread", dmThread);

  initState({
    workspaceId: "ws_default",
    repoDir: "/repo",
    remoteUrl: "https://github.com/acme/room",
    fsName: "gitim",
    corsProxy: "",
    token: "token",
    handler: "lewis",
    displayName: "Lewis",
  });
  setState({ defaultBranch: "main", headCommit: "base" });
}

function boardMarkdown(handler: string, body = "## 当前状态\n\n在线\n"): string {
  return [
    "---",
    "version: 1",
    `handler: ${handler}`,
    "updated_at: 20260509T120000Z",
    "status: working",
    "summary: Browser board",
    "tags:",
    "  - mobile",
    "---",
    body,
  ].join("\n");
}

describe("daemon-web handlers", () => {
  beforeEach(seedState);

  it("initializes an existing cached repo without a token", async () => {
    dirs.set("/repo/.git", []);

    const res = await init({
      workspaceId: "ws_cached",
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      storage: { fsName: "gitim-ws-ws_cached", repoDir: "/repo" },
    });

    expect(res.ok).toBe(true);
    expect(res.data).toEqual(expect.objectContaining({
      handler: "lewis",
      display_name: "Lewis",
      sync_enabled: false,
      needs_token: true,
    }));
  });

  it("preserves the remote sync baseline when cached local commits are ahead", async () => {
    const git = vi.mocked(await import("./git"));
    dirs.set("/repo/.git", []);
    git.resolveHead.mockResolvedValueOnce("local-unsynced-head");
    git.resolveRemoteHead.mockResolvedValueOnce("remote-synced-head");

    const res = await init({
      workspaceId: "ws_cached",
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      token: "new-token",
      handler: "lewis",
      storage: { fsName: "gitim-ws-ws_cached", repoDir: "/repo" },
    });

    expect(res.ok).toBe(true);
    expect(getState().headCommit).toBe("remote-synced-head");
  });

  it("rejects a cached browser repo when the requested remote changed", async () => {
    const git = vi.mocked(await import("./git"));
    dirs.set("/repo/.git", []);
    git.getOriginUrl.mockResolvedValueOnce("https://github.com/acme/old-room");
    const cloneCallsBefore = git.cloneRepo.mock.calls.length;

    const res = await init({
      workspaceId: "ws_cached",
      remoteUrl: "https://github.com/acme/new-room",
      corsProxy: "https://proxy.example",
      token: "new-token",
      handler: "lewis",
      storage: { fsName: "gitim-ws-ws_cached", repoDir: "/repo" },
    });

    expect(res).toEqual({
      ok: false,
      error: "Cached browser workspace was cloned from a different remote. Reset this workspace cache or create a new browser workspace to use the new URL.",
      error_code: "remote_mismatch",
    });
    expect(git.cloneRepo.mock.calls).toHaveLength(cloneCallsBefore);
  });

  it("restores the previous fs name when init needs a token for an absent repo", async () => {
    activeFsName.value = "gitim-ws-existing";

    const res = await init({
      workspaceId: "ws_absent",
      remoteUrl: "https://github.com/acme/absent",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      storage: { fsName: "gitim-ws-absent", repoDir: "/repo" },
    });

    expect(res).toEqual({
      ok: false,
      error: "Reconnect token to clone this browser workspace.",
      error_code: "reconnect_required",
    });
    expect(getActiveFsName()).toBe("gitim-ws-existing");
  });

  it("restores previous fs and state when init fails after publishing state", async () => {
    activeFsName.value = "gitim-ws-existing";
    initState({
      workspaceId: "ws_existing",
      repoDir: "/repo",
      remoteUrl: "https://github.com/acme/existing",
      fsName: "gitim-ws-existing",
      corsProxy: "https://proxy.example",
      token: "existing-token",
      handler: "lewis",
      displayName: "Lewis",
    });
    setState({ defaultBranch: "main", headCommit: "existing-head" });
    dirs.set("/repo/.git", []);
    readdirFailures.set("/repo/channels", "late init cache failure");

    const res = await init({
      workspaceId: "ws_new",
      remoteUrl: "https://github.com/acme/new",
      corsProxy: "https://proxy.example",
      token: "new-token",
      handler: "alice",
      storage: { fsName: "gitim-ws-new", repoDir: "/repo" },
    });

    expect(res).toEqual({
      ok: false,
      error: "late init cache failure",
    });
    expect(getActiveFsName()).toBe("gitim-ws-existing");
    expect(getState()).toEqual(expect.objectContaining({
      workspaceId: "ws_existing",
      remoteUrl: "https://github.com/acme/existing",
      fsName: "gitim-ws-existing",
      token: "existing-token",
      headCommit: "existing-head",
      me: { handler: "lewis", display_name: "Lewis" },
    }));
  });

  it("requires reconnect token before browser send when token is missing", async () => {
    initState({
      workspaceId: "ws_cached",
      repoDir: "/repo",
      remoteUrl: "https://github.com/acme/room",
      fsName: "gitim-ws-ws_cached",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      displayName: "Lewis",
    });
    setState({ defaultBranch: "main", headCommit: "base" });

    const res = await send("general", "from offline cache");

    expect(res).toEqual({
      ok: false,
      error: "Reconnect token to send from this browser workspace.",
      error_code: "reconnect_required",
    });
    expect(commits).toHaveLength(0);
  });

  it("returns cached poll state without network when token is missing", async () => {
    initState({
      workspaceId: "ws_cached",
      repoDir: "/repo",
      remoteUrl: "https://github.com/acme/room",
      fsName: "gitim-ws-ws_cached",
      corsProxy: "https://proxy.example",
      token: null,
      handler: "lewis",
      displayName: "Lewis",
    });
    setState({ defaultBranch: "main", headCommit: "cached-head" });

    const res = await poll("cached-head");

    expect(res).toEqual({
      ok: true,
      data: {
        commit_id: "cached-head",
        changes: [],
        sync_enabled: false,
        needs_token: true,
      },
    });
  });

  it("turns auth failures during poll into cached reconnect state", async () => {
    runSyncMock.mockRejectedValueOnce(new Error("HTTP Error: 401 Unauthorized"));
    setState({ headCommit: "cached-head" });

    const res = await poll("cached-head");

    expect(res).toEqual({
      ok: true,
      data: {
        commit_id: "cached-head",
        changes: [],
        sync_enabled: false,
        needs_token: true,
      },
      error_code: "reconnect_required",
    });
    expect(getState().token).toBeNull();
    expect(getState().syncStatus).toBe("reconnect_required");
  });

  it("lists channels from channels/*.meta.yaml and dms from dm/*.thread", async () => {
    const res = await channels();

    expect(res.ok).toBe(true);
    expect(res.data?.channels).toEqual([
      {
        name: "general",
        kind: "channel",
        unreadCount: 0,
        members: ["alice", "lewis"],
        created_by: "alice",
      },
      {
        name: "alice--lewis",
        kind: "dm",
        unreadCount: 0,
        members: ["alice", "lewis"],
      },
      {
        name: "cfo--flame4",
        kind: "dm",
        unreadCount: 0,
        members: ["cfo", "flame4"],
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
        recipients: ["alice"],
      }),
    ]);
    expect(res.data).not.toHaveProperty("messages");
  });

  it("adds channel recipients from creator, parent chain, and mentions", async () => {
    files.set(
      "/repo/channels/general.thread",
      [
        "[L000001][P000000][@alice][20260317T120000Z] hello",
        "[L000002][P000001][@lewis][20260317T120100Z] <@flame4> reply",
        "",
      ].join("\n"),
    );

    const res = await read("general");

    expect(res.ok).toBe(true);
    expect(res.data?.entries).toEqual([
      expect.objectContaining({
        line_number: 1,
        recipients: ["alice"],
      }),
      expect.objectContaining({
        line_number: 2,
        recipients: ["alice", "flame4"],
      }),
    ]);
  });

  it("resolves dm: API names to dm/*.thread", async () => {
    const res = await read("dm:alice,lewis");

    expect(res.ok).toBe(true);
    expect(res.data?.entries).toEqual([
      expect.objectContaining({
        line_number: 1,
        author: "alice",
        body: "private",
        recipients: ["alice", "lewis"],
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

  it("archives active channels into archive/channels and removes them from active lists", async () => {
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: lewis",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    );

    const res = await archiveChannel("general");

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      channel: "general",
      archived_by: "lewis",
      status: "pushed",
    });
    expect(files.has("/repo/channels/general.meta.yaml")).toBe(false);
    expect(files.has("/repo/channels/general.thread")).toBe(false);
    expect(files.get("/repo/archive/channels/general.meta.yaml"))
      .toContain("display_name: General");
    expect(files.get("/repo/archive/channels/general.thread")).toBe(generalThread);
    expect(commits.at(-1)).toEqual({
      filepaths: [
        "archive/channels/general.meta.yaml",
        "archive/channels/general.thread",
        "archive/channels/general/cards/20260317-120000-abc/card.meta.yaml",
        "archive/channels/general/cards/20260317-120000-abc/discussion.thread",
        "channels/general.meta.yaml",
        "channels/general.thread",
        "channels/general/cards/20260317-120000-abc/card.meta.yaml",
        "channels/general/cards/20260317-120000-abc/discussion.thread",
      ],
      message: "archive: #general by @lewis",
    });

    const active = await channels();
    expect(active.data?.channels).toEqual([
      {
        name: "alice--lewis",
        kind: "dm",
        unreadCount: 0,
        members: ["alice", "lewis"],
      },
      {
        name: "cfo--flame4",
        kind: "dm",
        unreadCount: 0,
        members: ["cfo", "flame4"],
      },
    ]);

    const archived = await listArchivedChannels();
    expect(archived.data?.channels).toEqual([
      {
        name: "general",
        kind: "archived_channel",
        members: ["alice", "lewis"],
      },
    ]);

    const archivedRead = await read("general");
    expect(archivedRead.data?.archived).toBe(true);
    expect(archivedRead.data?.entries).toEqual([
      expect.objectContaining({ body: "hello" }),
      expect.objectContaining({ body: "reply" }),
    ]);
  });

  it("pages archived channels when limit is supplied", async () => {
    registerDir("/repo/archive");
    registerDir("/repo/archive/channels");
    for (const name of ["alpha", "beta", "gamma"]) {
      files.set(
        `/repo/archive/channels/${name}.meta.yaml`,
        ["display_name: Test", "members:", "  - lewis", ""].join("\n"),
      );
      registerFile(`/repo/archive/channels/${name}.meta.yaml`);
    }

    const first = await listArchivedChannels({ offset: 0, limit: 2 });
    const second = await listArchivedChannels({ offset: 2, limit: 2 });

    expect(first.data).toEqual({
      channels: [
        { name: "alpha", kind: "archived_channel", members: ["lewis"] },
        { name: "beta", kind: "archived_channel", members: ["lewis"] },
      ],
      has_more: true,
    });
    expect(second.data).toEqual({
      channels: [{ name: "gamma", kind: "archived_channel", members: ["lewis"] }],
      has_more: false,
    });
  });

  it("restores archived channels into active lists", async () => {
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: lewis",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    );
    await archiveChannel("general");

    const res = await unarchiveChannel("general");

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      channel: "general",
      unarchived_by: "lewis",
      status: "pushed",
    });
    expect(files.get("/repo/channels/general.meta.yaml"))
      .toContain("display_name: General");
    expect(files.get("/repo/channels/general.thread")).toBe(generalThread);
    expect(files.has("/repo/archive/channels/general.meta.yaml")).toBe(false);
    expect(files.has("/repo/archive/channels/general.thread")).toBe(false);
    expect(commits.at(-1)).toEqual({
      filepaths: [
        "channels/general.meta.yaml",
        "channels/general.thread",
        "channels/general/cards/20260317-120000-abc/card.meta.yaml",
        "channels/general/cards/20260317-120000-abc/discussion.thread",
        "archive/channels/general.meta.yaml",
        "archive/channels/general.thread",
        "archive/channels/general/cards/20260317-120000-abc/card.meta.yaml",
        "archive/channels/general/cards/20260317-120000-abc/discussion.thread",
      ],
      message: "unarchive: #general by @lewis",
    });

    const active = await channels();
    expect(active.data?.channels).toEqual([
      {
        name: "general",
        kind: "channel",
        unreadCount: 0,
        members: ["alice", "lewis"],
        created_by: "lewis",
      },
      {
        name: "alice--lewis",
        kind: "dm",
        unreadCount: 0,
        members: ["alice", "lewis"],
      },
      {
        name: "cfo--flame4",
        kind: "dm",
        unreadCount: 0,
        members: ["cfo", "flame4"],
      },
    ]);

    const archived = await listArchivedChannels();
    expect(archived.data?.channels).toEqual([]);
  });

  it("only restores cards with archived_via=channel on unarchiveChannel", async () => {
    // Override channel meta to make lewis the creator (required for archiveChannel/unarchiveChannel)
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: lewis",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    );

    // MANUAL_CARD: already archived via archiveCard (lives in archive/channels/general/cards/)
    const manualId = "20260317-110000-man";
    dirs.set("/repo/archive/channels/general/cards", [manualId]);
    dirs.set(`/repo/archive/channels/general/cards/${manualId}`, [
      "card.meta.yaml",
      "discussion.thread",
    ]);
    files.set(
      `/repo/archive/channels/general/cards/${manualId}/card.meta.yaml`,
      [
        "title: Manual card",
        "channel: general",
        "status: todo",
        "labels:",
        "assignee: lewis",
        "created_by: lewis",
        "created_at: 20260317T110000Z",
        "updated_at: 20260317T110000Z",
        "archived_via: manual",
        "",
      ].join("\n"),
    );
    files.set(
      `/repo/archive/channels/general/cards/${manualId}/discussion.thread`,
      "[L000001][P000000][@lewis][20260317T110000Z] manual note\n",
    );

    // archiveChannel stamps the default active card (20260317-120000-abc) with archived_via=channel
    await archiveChannel("general");
    // Now unarchive: should only restore the auto (channel) card, leave manual in archive
    await unarchiveChannel("general");

    // AUTO_CARD (20260317-120000-abc) should be back in active, no archived_via field
    const autoYaml = files.get(
      "/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml",
    )!;
    expect(autoYaml).not.toContain("archived_via");

    // MANUAL_CARD should still be in archive with archived_via: manual
    expect(
      files.has("/repo/channels/general/cards/20260317-110000-man/card.meta.yaml"),
    ).toBe(false);
    const manualYaml = files.get(
      `/repo/archive/channels/general/cards/${manualId}/card.meta.yaml`,
    )!;
    expect(manualYaml).toContain("archived_via: manual");
  });

  it("unarchiveChannel with only manual-archived cards moves channel without restoring any cards", async () => {
    // Override channel meta + put it in the archive location (simulating a channel that was
    // archived but had only manual-archived cards beneath it — filter discards them all,
    // so cardMoves stays empty and the no-cards mkdirp branch must run).
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: lewis",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    );
    await archiveChannel("general");

    // Replace the channel-stamped auto card with a manual-stamped one (simulating that
    // the user manually archived it before the channel archive happened).
    const archivedAutoYaml = files.get(
      "/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml",
    )!;
    files.set(
      "/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml",
      archivedAutoYaml.replace("archived_via: channel", "archived_via: manual"),
    );

    await unarchiveChannel("general");

    // Channel itself should be back in active
    expect(files.has("/repo/channels/general.meta.yaml")).toBe(true);
    expect(files.has("/repo/channels/general.thread")).toBe(true);
    // The manual-stamped card should stay in archive (no restore)
    expect(
      files.has("/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"),
    ).toBe(false);
    expect(
      files.has("/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"),
    ).toBe(true);
  });

  it("rejects invalid channel names before writing files", async () => {
    const res = await send("../evil", "bad");

    expect(res.ok).toBe(false);
    expect(res.error).toContain("invalid channel name");
    expect(files.has("/repo/channels/../evil.thread")).toBe(false);
  });

  it("returns pushed status after send sync succeeds", async () => {
    const res = await send("general", "from browser");

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      line_number: 3,
      status: "pushed",
    });
    expect(runSyncMock).toHaveBeenCalledWith({ forceNewCycle: true });
  });

  it("does not write a sent message while a sync holds the repo lock", async () => {
    let releaseSync!: () => void;
    const sync = withRepoLock(
      () =>
        new Promise<void>((resolve) => {
          releaseSync = resolve;
        }),
    );
    await Promise.resolve();

    const sent = send("general", "from locked mobile send");
    await Promise.resolve();

    expect(files.get("/repo/channels/general.thread")).not.toContain(
      "from locked mobile send",
    );
    expect(commits).toHaveLength(0);

    releaseSync();
    await sync;
    const res = await sent;

    expect(res.ok).toBe(true);
    expect(files.get("/repo/channels/general.thread")).toContain(
      "from locked mobile send",
    );
    expect(commits.at(-1)?.message).toContain("L000003");
  });

  it("keeps the local line number and surfaces sync failure after send", async () => {
    runSyncMock.mockRejectedValueOnce(new Error("HTTP Error: 401 Unauthorized"));

    const res = await send("general", "from mobile");

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      line_number: 3,
      status: "commit_only",
      error: "HTTP Error: 401 Unauthorized",
      error_code: "reconnect_required",
      needs_token: true,
    });
    expect(getState().token).toBeNull();
    expect(getState().syncStatus).toBe("reconnect_required");
    expect(files.get("/repo/channels/general.thread")).toContain("from mobile");
    expect(commits.at(-1)?.message).toContain("L000003");
  });

  it("initializes, shows, and lists browser boards", async () => {
    const initRes = await initBoard();

    expect(initRes.ok).toBe(true);
    expect(initRes.data).toEqual(expect.objectContaining({
      handler: "lewis",
      path: "showboards/lewis/board.md",
      status: "committed",
      commit_id: "new-head",
      sync_status: "pushed",
    }));
    expect(files.get("/repo/showboards/lewis/board.md")).toContain("handler: lewis");
    expect(commits.at(-1)).toEqual({
      filepaths: ["showboards/lewis/board.md"],
      message: "board: init @lewis",
    });

    const showRes = await showBoard("lewis");
    expect(showRes.ok).toBe(true);
    expect(showRes.data).toEqual(expect.objectContaining({
      handler: "lewis",
      path: "showboards/lewis/board.md",
      meta: expect.objectContaining({ handler: "lewis", status: "idle" }),
    }));

    const listRes = await listBoards();
    expect(listRes.ok).toBe(true);
    expect(listRes.data?.boards).toEqual([
      expect.objectContaining({
        handler: "lewis",
        path: "showboards/lewis/board.md",
      }),
    ]);
  });

  it("reports the post-sync commit id for browser board writes", async () => {
    runSyncMock.mockImplementationOnce(async () => {
      setState({ headCommit: "rebased-head" });
    });

    const res = await initBoard();

    expect(res.ok).toBe(true);
    expect(res.data).toEqual(expect.objectContaining({
      handler: "lewis",
      path: "showboards/lewis/board.md",
      status: "committed",
      commit_id: "rebased-head",
      sync_status: "pushed",
    }));
  });

  it("refuses to initialize over an existing browser board", async () => {
    dirs.set("/repo/showboards", ["lewis"]);
    dirs.set("/repo/showboards/lewis", ["board.md"]);
    files.set("/repo/showboards/lewis/board.md", boardMarkdown("lewis", "## 当前状态\n\nKeep me\n"));

    const res = await initBoard();

    expect(res.ok).toBe(false);
    expect(res.error).toContain("already exists");
    expect(files.get("/repo/showboards/lewis/board.md")).toContain("Keep me");
    expect(commits).toHaveLength(0);
  });

  it("rejects browser board publish content with handler mismatch", async () => {
    const res = await publishBoard(boardMarkdown("alice"));

    expect(res.ok).toBe(false);
    expect(res.error).toContain("handler mismatch");
    expect(files.has("/repo/showboards/lewis/board.md")).toBe(false);
    expect(commits).toHaveLength(0);
  });

  it("refreshes browser board publish content timestamp", async () => {
    vi.setSystemTime(new Date("2026-03-17T12:34:56Z"));
    const stale = boardMarkdown("lewis").replace(
      "updated_at: 20260509T120000Z",
      "updated_at: 20200101T000000Z",
    );

    const res = await publishBoard(stale);

    expect(res.ok).toBe(true);
    const written = files.get("/repo/showboards/lewis/board.md");
    expect(written).toContain("handler: lewis");
    expect(written).toContain("updated_at: 20260317T123456Z");
    expect(written).not.toContain("updated_at: 20200101T000000Z");
  });

  it("mutates browser board fields and sections through wasm helpers", async () => {
    dirs.set("/repo/showboards", ["lewis"]);
    dirs.set("/repo/showboards/lewis", ["board.md"]);
    files.set("/repo/showboards/lewis/board.md", boardMarkdown("lewis"));

    await expect(setBoard("summary", "Updated")).resolves.toEqual(
      expect.objectContaining({ ok: true }),
    );
    await expect(setBoardSectionValue("当前状态", "Busy")).resolves.toEqual(
      expect.objectContaining({ ok: true }),
    );
    await expect(appendBoardSectionValue("待确认", "- one")).resolves.toEqual(
      expect.objectContaining({ ok: true }),
    );

    expect(commits.map((commit) => commit.filepaths)).toEqual([
      ["showboards/lewis/board.md"],
      ["showboards/lewis/board.md"],
      ["showboards/lewis/board.md"],
    ]);
  });

  it("reports board changes from poll with empty entries", async () => {
    vi.mocked(await import("./git")).diffTrees.mockResolvedValueOnce([
      "showboards/alice/board.md",
      "showboards/system/board.md",
      "showboards/bad--name/board.md",
    ]);
    vi.mocked(await import("./git")).resolveHead.mockResolvedValueOnce("next-head");

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(res.data?.changes).toContainEqual({
      channel: "alice",
      kind: "board",
      entries: [],
    });
    expect(res.data?.changes).not.toContainEqual(
      expect.objectContaining({ channel: "system", kind: "board" }),
    );
  });

  it("returns reset on stale poll cursor", async () => {
    vi.mocked(await import("./git")).diffTrees.mockRejectedValueOnce(
      new Error("stale cursor"),
    );
    vi.mocked(await import("./git")).resolveHead.mockResolvedValueOnce("next-head");

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      commit_id: "next-head",
      changes: [],
      reset: true,
    });
  });

  it("lists cards from channels/<channel>/cards", async () => {
    const res = await listCards();

    expect(res.ok).toBe(true);
    expect(res.data?.cards).toEqual([
      {
        card_id: "20260317-120000-abc",
        channel: "general",
        title: "Existing card",
        status: "todo",
        labels: ["mobile"],
        assignee: "lewis",
        created_by: "alice",
        created_at: "20260317T120000Z",
        updated_at: "20260317T120000Z",
      },
    ]);
  });

  it("creates a card directory, meta file, discussion thread, and commit", async () => {
    vi.setSystemTime(new Date("2026-03-17T12:34:56Z"));
    vi.spyOn(Math, "random").mockReturnValue(0xabc / 0x1000);

    const res = await createCard("general", "New browser card", {
      labels: ["mobile"],
      assignee: "lewis",
      status: "doing",
    });

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      channel: "general",
      card_id: "20260317-123456-abc",
      title: "New browser card",
      status: "pushed",
    });
    expect(files.get("/repo/channels/general/cards/20260317-123456-abc/card.meta.yaml"))
      .toContain("title: New browser card");
    expect(files.get("/repo/channels/general/cards/20260317-123456-abc/discussion.thread"))
      .toBe("");
    expect(commits.at(-1)).toEqual({
      filepaths: [
        "channels/general/cards/20260317-123456-abc/card.meta.yaml",
        "channels/general/cards/20260317-123456-abc/discussion.thread",
      ],
      message: "card: create 20260317-123456-abc in general by @lewis",
    });
  });

  it("reads a card with meta and discussion entries", async () => {
    const res = await readCard("general", "20260317-120000-abc", { limit: 1 });

    expect(res.ok).toBe(true);
    expect(res.data?.archived).toBe(false);
    expect(res.data?.meta).toEqual(
      expect.objectContaining({
        card_id: "20260317-120000-abc",
        channel: "general",
        title: "Existing card",
      }),
    );
    expect(res.data?.entries).toEqual([
      expect.objectContaining({ line_number: 1, body: "card note" }),
    ]);
  });

  it("appends a card discussion message and commits the discussion thread", async () => {
    vi.setSystemTime(new Date("2026-03-17T12:35:00Z"));

    const res = await sendCardMessage("general", "20260317-120000-abc", "second note");

    expect(res.ok).toBe(true);
    expect(res.data?.line_number).toBe(2);
    expect(files.get("/repo/channels/general/cards/20260317-120000-abc/discussion.thread"))
      .toContain("[L000002][P000000][@lewis][20260317T123500Z] second note");
    expect(commits.at(-1)).toEqual({
      filepaths: ["channels/general/cards/20260317-120000-abc/discussion.thread"],
      message: "card-msg: @lewis -> general/20260317-120000-abc L000002",
    });
  });

  it("updates card metadata and commits the meta file", async () => {
    vi.setSystemTime(new Date("2026-03-17T12:36:00Z"));

    const res = await updateCard("general", "20260317-120000-abc", {
      status: "done",
      labels: ["mobile", "done"],
      assignee: null,
    });

    expect(res.ok).toBe(true);
    const yaml = files.get("/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml");
    expect(yaml).toContain("status: done");
    expect(yaml).toContain("assignee: null");
    expect(yaml).toContain("updated_at: 20260317T123600Z");
    expect(commits.at(-1)).toEqual({
      filepaths: ["channels/general/cards/20260317-120000-abc/card.meta.yaml"],
      message: "card: update 20260317-120000-abc in general by @lewis",
    });
  });

  it("reports card meta and discussion changes from poll", async () => {
    vi.mocked(await import("./git")).diffTrees.mockResolvedValueOnce([
      "channels/general/cards/20260317-120000-abc/card.meta.yaml",
      "channels/general/cards/20260317-120000-abc/discussion.thread",
    ]);
    vi.mocked(await import("./git")).resolveHead.mockResolvedValueOnce("next-head");

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(res.data?.changes).toEqual([
      {
        channel: "card:general/20260317-120000-abc",
        kind: "card_meta",
      },
      {
        channel: "card:general/20260317-120000-abc",
        kind: "card_thread",
        entries: [expect.objectContaining({ line_number: 1, body: "card note" })],
      },
    ]);
  });

  it("reports archived channel changes from poll", async () => {
    vi.mocked(await import("./git")).diffTrees.mockResolvedValueOnce([
      "archive/channels/general.meta.yaml",
    ]);
    vi.mocked(await import("./git")).resolveHead.mockResolvedValueOnce("next-head");

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(res.data?.changes).toEqual([
      {
        channel: "general",
        kind: "channel_meta",
      },
    ]);
  });

  it("reports archived DM changes from poll without resurrecting the deleted active path", async () => {
    files.delete("/repo/dm/alice--lewis.thread");
    dirs.set("/repo/archive", ["dm"]);
    dirs.set("/repo/archive/dm", ["alice--lewis.thread"]);
    files.set("/repo/archive/dm/alice--lewis.thread", dmThread);
    vi.mocked(await import("./git")).diffTrees.mockResolvedValueOnce([
      "dm/alice--lewis.thread",
      "archive/dm/alice--lewis.thread",
    ]);
    vi.mocked(await import("./git")).resolveHead.mockResolvedValueOnce("next-head");

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(res.data?.changes).toEqual([
      {
        channel: "dm:alice,lewis",
        kind: "dm_archived",
        entries: [],
      },
    ]);
  });

  it("does not report archived DM changes to non-participants", async () => {
    setState({ me: { handler: "bob", display_name: "Bob" } });
    dirs.set("/repo/archive", ["dm"]);
    dirs.set("/repo/archive/dm", ["alice--lewis.thread"]);
    files.set("/repo/archive/dm/alice--lewis.thread", dmThread);
    vi.mocked(await import("./git")).diffTrees.mockResolvedValueOnce([
      "archive/dm/alice--lewis.thread",
    ]);
    vi.mocked(await import("./git")).resolveHead.mockResolvedValueOnce("next-head");

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(res.data?.changes).toEqual([]);
  });

  it("poll delegates git state ownership to runSync", async () => {
    const git = vi.mocked(await import("./git"));
    git.resolveHead.mockResolvedValueOnce("remote-head");
    git.diffTrees.mockResolvedValueOnce(["channels/general.thread"]);

    const res = await poll("base");

    expect(res.ok).toBe(true);
    expect(runSyncMock).toHaveBeenCalledWith();
    expect(git.fetchOrigin).not.toHaveBeenCalled();
    expect(git.resetToRemote).not.toHaveBeenCalled();
    expect(res.data?.commit_id).toBe("remote-head");
    expect(res.data?.changes).toEqual([
      {
        channel: "general",
        kind: "new_messages",
        entries: [
          expect.objectContaining({
            line_number: 1,
            body: "hello",
            recipients: ["alice"],
          }),
          expect.objectContaining({
            line_number: 2,
            body: "reply",
            recipients: ["alice"],
          }),
        ],
      },
    ]);
  });

  it("poll reports the current Worker-owned head without mutating sync baseline", async () => {
    const git = vi.mocked(await import("./git"));
    setState({ defaultBranch: "main", headCommit: "remote-base" });
    git.resolveHead.mockResolvedValueOnce("local-unsynced-head");

    const res = await poll("remote-base");

    expect(res.ok).toBe(true);
    expect(runSyncMock).toHaveBeenCalledWith();
    expect(git.fetchOrigin).not.toHaveBeenCalled();
    expect(git.resetToRemote).not.toHaveBeenCalled();
    expect(getState().headCommit).toBe("remote-base");
  });

  it("archives active cards into archive/channels and removes them from active lists", async () => {
    const res = await archiveCard("general", "20260317-120000-abc");

    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      channel: "general",
      card_id: "20260317-120000-abc",
      archived_by: "lewis",
      status: "pushed",
    });
    expect(files.has("/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"))
      .toBe(false);
    expect(files.get("/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"))
      .toContain("title: Existing card");
    expect(commits.at(-1)).toEqual({
      filepaths: [
        "archive/channels/general/cards/20260317-120000-abc/card.meta.yaml",
        "archive/channels/general/cards/20260317-120000-abc/discussion.thread",
        "channels/general/cards/20260317-120000-abc/card.meta.yaml",
        "channels/general/cards/20260317-120000-abc/discussion.thread",
      ],
      message: "card: archive 20260317-120000-abc in general by @lewis",
    });

    const active = await listCards();
    expect(active.data?.cards).toEqual([]);

    const archived = await listArchivedCards();
    expect(archived.data?.cards).toEqual([
      expect.objectContaining({
        card_id: "20260317-120000-abc",
        title: "Existing card",
      }),
    ]);
  });

  it("reads archived cards and restores them into active cards", async () => {
    await archiveCard("general", "20260317-120000-abc");

    const archivedRead = await readCard("general", "20260317-120000-abc");
    expect(archivedRead.ok).toBe(true);
    expect(archivedRead.data?.archived).toBe(true);
    expect(archivedRead.data?.entries).toEqual([
      expect.objectContaining({ body: "card note" }),
    ]);

    const res = await unarchiveCard("general", "20260317-120000-abc");
    expect(res.ok).toBe(true);
    expect(res.data).toEqual({
      channel: "general",
      card_id: "20260317-120000-abc",
      unarchived_by: "lewis",
      status: "pushed",
    });
    expect(files.get("/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"))
      .toContain("title: Existing card");
    expect(files.has("/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"))
      .toBe(false);
    expect(commits.at(-1)?.message)
      .toBe("card: unarchive 20260317-120000-abc in general by @lewis");
  });

  it("stamps archived_via=manual in yaml on archiveCard", async () => {
    await archiveCard("general", "20260317-120000-abc");
    const yaml = files.get(
      "/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"
    )!;
    expect(yaml).toContain("archived_via: manual");
  });

  it("clears archived_via in yaml on unarchiveCard", async () => {
    await archiveCard("general", "20260317-120000-abc");
    await unarchiveCard("general", "20260317-120000-abc");
    const yaml = files.get(
      "/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"
    )!;
    expect(yaml).not.toContain("archived_via");
  });

  it("archiveChannel moves cards subtree and stamps archived_via=channel", async () => {
    // Override channel meta to make lewis the creator (required for archiveChannel)
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: lewis",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    );

    // Seed a second active card (CARD2) alongside the existing CARD1
    const card2Id = "20260317-130000-def";
    dirs.set("/repo/channels/general/cards", ["20260317-120000-abc", card2Id]);
    dirs.set(`/repo/channels/general/cards/${card2Id}`, [
      "card.meta.yaml",
      "discussion.thread",
    ]);
    files.set(
      `/repo/channels/general/cards/${card2Id}/card.meta.yaml`,
      [
        "title: Second card",
        "channel: general",
        "status: todo",
        "labels:",
        "assignee: lewis",
        "created_by: lewis",
        "created_at: 20260317T130000Z",
        "updated_at: 20260317T130000Z",
        "",
      ].join("\n"),
    );
    files.set(
      `/repo/channels/general/cards/${card2Id}/discussion.thread`,
      "[L000001][P000000][@lewis][20260317T130000Z] card2 note\n",
    );

    await archiveChannel("general");

    // Active card paths should be gone
    expect(files.has("/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"))
      .toBe(false);
    expect(files.has(`/repo/channels/general/cards/${card2Id}/card.meta.yaml`))
      .toBe(false);

    // Archive card paths should exist with archived_via: channel
    const yaml1 = files.get(
      "/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"
    )!;
    const yaml2 = files.get(
      `/repo/archive/channels/general/cards/${card2Id}/card.meta.yaml`
    )!;
    expect(yaml1).toContain("archived_via: channel");
    expect(yaml2).toContain("archived_via: channel");
  });

  it("archiveChannel does not overwrite archived_via=manual for already-archived cards", async () => {
    // Override channel meta to make lewis the creator
    files.set(
      "/repo/channels/general.meta.yaml",
      [
        "display_name: General",
        "created_by: lewis",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    );

    // MANUAL_CARD: already archived via archiveCard (lives in archive/channels/general/cards/)
    const manualId = "20260317-110000-man";
    dirs.set("/repo/archive/channels/general/cards", [manualId]);
    dirs.set(`/repo/archive/channels/general/cards/${manualId}`, [
      "card.meta.yaml",
      "discussion.thread",
    ]);
    files.set(
      `/repo/archive/channels/general/cards/${manualId}/card.meta.yaml`,
      [
        "title: Manual card",
        "channel: general",
        "status: todo",
        "labels:",
        "assignee: lewis",
        "created_by: lewis",
        "created_at: 20260317T110000Z",
        "updated_at: 20260317T110000Z",
        "archived_via: manual",
        "",
      ].join("\n"),
    );
    files.set(
      `/repo/archive/channels/general/cards/${manualId}/discussion.thread`,
      "[L000001][P000000][@lewis][20260317T110000Z] manual note\n",
    );

    // Only the default active card (from seedState) remains under channels/general/cards/
    await archiveChannel("general");

    // Manual-archived card should be untouched (archived_via still "manual")
    const manualYaml = files.get(
      `/repo/archive/channels/general/cards/${manualId}/card.meta.yaml`
    )!;
    expect(manualYaml).toContain("archived_via: manual");
    expect(manualYaml).not.toContain("archived_via: channel");

    // The default active card from seedState should be moved to archive and
    // stamped archived_via: channel (mixed-scenario isolation).
    const autoYaml = files.get(
      "/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"
    )!;
    expect(autoYaml).toContain("archived_via: channel");
  });
});

describe("daemon-web read with since + limit semantics", () => {
  beforeEach(() => {
    seedState();
    // Replace the 2-message generalThread with a 10-message fixture so the
    // since + limit branching can be exercised with non-trivial slicing.
    const lines: string[] = [];
    for (let i = 1; i <= 10; i++) {
      const ln = String(i).padStart(6, "0");
      lines.push(`[L${ln}][P000000][@alice][20260511T120000Z] m${i}`);
    }
    files.set("/repo/channels/general.thread", lines.join("\n") + "\n");
  });

  function lineNumbers(res: Awaited<ReturnType<typeof read>>): number[] {
    return ((res.data?.entries as Array<{ line_number: number }>) ?? []).map(
      (e) => e.line_number,
    );
  }

  it("limit only returns the last N entries", async () => {
    const res = await read("general", 3);
    expect(res.ok).toBe(true);
    expect(lineNumbers(res)).toEqual([8, 9, 10]);
  });

  it("since only returns all entries after the cursor", async () => {
    const res = await read("general", undefined, 7);
    expect(res.ok).toBe(true);
    expect(lineNumbers(res)).toEqual([8, 9, 10]);
  });

  it("since + limit head-truncates to the first N entries after the cursor", async () => {
    const res = await read("general", 3, 2);
    expect(res.ok).toBe(true);
    expect(lineNumbers(res)).toEqual([3, 4, 5]);
  });

  it("since beyond max line returns empty", async () => {
    const res = await read("general", 5, 100);
    expect(res.ok).toBe(true);
    expect(lineNumbers(res)).toEqual([]);
  });

  it("since + limit=0 returns empty", async () => {
    const res = await read("general", 0, 2);
    expect(res.ok).toBe(true);
    expect(lineNumbers(res)).toEqual([]);
  });

  it("limit=0 alone returns empty (matches daemon, guards JS slice(-0) edge case)", async () => {
    const res = await read("general", 0);
    expect(res.ok).toBe(true);
    expect(lineNumbers(res)).toEqual([]);
  });
});

describe("daemon-web readCard with since + limit semantics", () => {
  beforeEach(() => {
    seedState();
    // Replace the 1-message card discussion fixture with 10 messages so
    // since + limit head-cut can be exercised meaningfully.
    const lines: string[] = [];
    for (let i = 1; i <= 10; i++) {
      const ln = String(i).padStart(6, "0");
      lines.push(`[L${ln}][P000000][@alice][20260511T120000Z] cm${i}`);
    }
    files.set(
      "/repo/channels/general/cards/20260317-120000-abc/discussion.thread",
      lines.join("\n") + "\n",
    );
  });

  function entryLines(res: Awaited<ReturnType<typeof readCard>>): number[] {
    return ((res.data?.entries as Array<{ line_number: number }>) ?? []).map(
      (e) => e.line_number,
    );
  }

  it("readCard with limit only returns the last N entries (tail-cut)", async () => {
    const res = await readCard("general", "20260317-120000-abc", { limit: 3 });
    expect(res.ok).toBe(true);
    expect(entryLines(res)).toEqual([8, 9, 10]);
  });

  it("readCard with since + limit head-cuts to the first N after the cursor (daemon parity)", async () => {
    const res = await readCard("general", "20260317-120000-abc", {
      limit: 3,
      since: 2,
    });
    expect(res.ok).toBe(true);
    expect(entryLines(res)).toEqual([3, 4, 5]);
  });

  it("readCard with since only returns all entries after the cursor", async () => {
    const res = await readCard("general", "20260317-120000-abc", { since: 7 });
    expect(res.ok).toBe(true);
    expect(entryLines(res)).toEqual([8, 9, 10]);
  });
});

describe("daemon-web listArchivedChannels pagination", () => {
  beforeEach(() => {
    seedState();
    const names = [
      "eng-alpha",
      "eng-beta",
      "eng-delta",
      "eng-gamma",
      "ops",
    ];
    dirs.set("/repo/archive", ["channels"]);
    dirs.set(
      "/repo/archive/channels",
      names.map((name) => `${name}.meta.yaml`),
    );
    for (const name of names) {
      files.set(
        `/repo/archive/channels/${name}.meta.yaml`,
        [
          `display_name: ${name}`,
          "members:",
          "  - lewis",
          "",
        ].join("\n"),
      );
    }
  });

  it("filters by prefix and returns paginated has_more", async () => {
    const res = await listArchivedChannels({
      prefix: "ENG",
      offset: 1,
      limit: 2,
    });

    expect(res.ok).toBe(true);
    expect(res.data?.has_more).toBe(true);
    expect(
      (res.data?.channels as Array<{ name: string }>).map((c) => c.name),
    ).toEqual(["eng-beta", "eng-delta"]);
  });

  it("clamps limit=0 to one row", async () => {
    const res = await listArchivedChannels({ prefix: "eng", limit: 0 });

    expect(res.ok).toBe(true);
    expect(res.data?.has_more).toBe(true);
    expect(res.data?.channels).toEqual([
      expect.objectContaining({ name: "eng-alpha" }),
    ]);
  });
});

describe("daemon-web listArchivedDms pagination", () => {
  beforeEach(() => {
    seedState();
    // Seed 6 archived DMs between lewis and 6 different peers, sorted
    // alphabetically: alice, bob, bobby, carol, dave, eve.
    const peers = ["alice", "bob", "bobby", "carol", "dave", "eve"];
    const fileNames: string[] = [];
    for (const peer of peers) {
      // stem is <min>--<max> in lexicographic order
      const stem = peer < "lewis" ? `${peer}--lewis` : `lewis--${peer}`;
      const rel = `archive/dm/${stem}.thread`;
      files.set(
        `/repo/${rel}`,
        `[L000001][P000000][@${peer}][20260511T120000Z] hi\n`,
      );
      fileNames.push(`${stem}.thread`);
    }
    dirs.set("/repo/archive/dm", fileNames);
    dirs.set("/repo/archive", ["dm"]);
  });

  it("returns has_more=true when more entries exist after limit", async () => {
    const res = await listArchivedDms({ prefix: "", offset: 0, limit: 5 });
    expect(res.ok).toBe(true);
    expect(res.data?.has_more).toBe(true);
    const peers = (res.data?.dms as Array<{ peer: string }>).map((d) => d.peer);
    expect(peers).toEqual(["alice", "bob", "bobby", "carol", "dave"]);
  });

  it("second page has_more=false when only 1 entry left", async () => {
    const res = await listArchivedDms({ prefix: "", offset: 5, limit: 5 });
    expect(res.ok).toBe(true);
    expect(res.data?.has_more).toBe(false);
    const peers = (res.data?.dms as Array<{ peer: string }>).map((d) => d.peer);
    expect(peers).toEqual(["eve"]);
  });

  it("filters by lowercase prefix case-insensitively", async () => {
    const res = await listArchivedDms({ prefix: "BO", offset: 0, limit: 10 });
    expect(res.ok).toBe(true);
    expect(res.data?.has_more).toBe(false);
    const peers = (res.data?.dms as Array<{ peer: string }>).map((d) => d.peer);
    expect(peers).toEqual(["bob", "bobby"]);
  });

  it("limit=0 clamps to 1", async () => {
    const res = await listArchivedDms({ prefix: "", offset: 0, limit: 0 });
    expect(res.ok).toBe(true);
    const dms = res.data?.dms as Array<{ peer: string }>;
    expect(dms.length).toBe(1);
    expect(dms[0].peer).toBe("alice");
    expect(res.data?.has_more).toBe(true);
  });

  it("empty archive dir returns empty + has_more=false", async () => {
    // Wipe the seeded archive contents.
    for (const peer of ["alice", "bob", "bobby", "carol", "dave", "eve"]) {
      const stem = peer < "lewis" ? `${peer}--lewis` : `lewis--${peer}`;
      files.delete(`/repo/archive/dm/${stem}.thread`);
    }
    dirs.set("/repo/archive/dm", []);

    const res = await listArchivedDms({ prefix: "", offset: 0, limit: 5 });
    expect(res.ok).toBe(true);
    expect(res.data?.dms).toEqual([]);
    expect(res.data?.has_more).toBe(false);
  });
});

describe("reconcileOrphanCards", () => {
  beforeEach(seedState);

  it("migrates orphan card dirs under archived channels and stamps archived_via=channel", async () => {
    // Setup: channel meta in archive/ but cards subtree still in channels/
    files.set("/repo/archive/channels/general.meta.yaml", "display_name: General\n");
    files.set("/repo/archive/channels/general.thread", "");
    registerDir("/repo/archive/channels");
    dirs.set("/repo/archive/channels", ["general.meta.yaml", "general.thread"]);

    // Remove the active channel meta to mark it as orphaned (no active meta)
    files.delete("/repo/channels/general.meta.yaml");
    // channels/ listing: no .meta.yaml for general, but the subdirectory "general" exists
    dirs.set("/repo/channels", ["general.thread", "general"]);

    // Leave the cards subtree under channels/ (the orphan state)
    // channels/general/cards/ORPHAN already seeded by seedState as 20260317-120000-abc
    // but seedState uses a different card id; let's use a fresh setup
    const orphanId = "20260317-110000-orphan";
    dirs.set("/repo/channels/general", ["cards"]);
    dirs.set("/repo/channels/general/cards", [orphanId]);
    dirs.set(`/repo/channels/general/cards/${orphanId}`, [
      "card.meta.yaml",
      "discussion.thread",
    ]);
    files.set(
      `/repo/channels/general/cards/${orphanId}/card.meta.yaml`,
      [
        "title: t",
        "channel: general",
        "status: todo",
        "labels:",
        "assignee: null",
        "created_by: alice",
        "created_at: '2026-01-01T00:00:00Z'",
        "updated_at: '2026-01-01T00:00:00Z'",
        "",
      ].join("\n"),
    );
    files.set(`/repo/channels/general/cards/${orphanId}/discussion.thread`, "");

    const n = await reconcileOrphanCards();
    expect(n).toBe(1);

    // Source should be gone
    expect(
      files.has(`/repo/channels/general/cards/${orphanId}/card.meta.yaml`),
    ).toBe(false);
    // Destination should exist with archived_via stamped
    const yaml = files.get(
      `/repo/archive/channels/general/cards/${orphanId}/card.meta.yaml`,
    )!;
    expect(yaml).toContain("archived_via: channel");
    // A commit should have been made
    expect(commits.at(-1)?.message).toBe(
      "chore: reconcile orphan cards under archived channels",
    );
  });

  it("is no-op when no orphans (no commit)", async () => {
    // Default seed: general channel has active meta — not an orphan
    const commitsBefore = commits.length;
    const n = await reconcileOrphanCards();
    expect(n).toBe(0);
    expect(commits.length).toBe(commitsBefore);
  });

  it("does not touch active channels even when archive meta also exists", async () => {
    // Both active and archive meta exist — should NOT be treated as orphan
    files.set("/repo/archive/channels/general.meta.yaml", "display_name: General\n");
    dirs.set("/repo/archive/channels", ["general.meta.yaml"]);

    const commitsBefore = commits.length;
    const n = await reconcileOrphanCards();
    expect(n).toBe(0);
    expect(commits.length).toBe(commitsBefore);
    // Active card untouched
    expect(
      files.has("/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"),
    ).toBe(true);
  });
});
