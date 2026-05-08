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
  stringifyCardMeta: vi.fn((meta: Record<string, unknown>) => {
    const labels = Array.isArray(meta.labels) ? meta.labels : [];
    return [
      `title: ${meta.title}`,
      `channel: ${meta.channel}`,
      `status: ${meta.status}`,
      "labels:",
      ...labels.map((label) => `  - ${label}`),
      `assignee: ${meta.assignee ?? "null"}`,
      `created_by: ${meta.created_by}`,
      `created_at: ${meta.created_at}`,
      `updated_at: ${meta.updated_at}`,
      "",
    ].join("\n");
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
  channels,
  createCard,
  init,
  listArchivedChannels,
  listArchivedCards,
  listCards,
  poll,
  read,
  readCard,
  send,
  sendCardMessage,
  thread,
  unarchiveChannel,
  updateCard,
  joinChannel,
  unarchiveCard,
} from "./handlers";
import { getState, initState, setState } from "./state";
import { getActiveFsName } from "./storage";

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
    vi.mocked(await import("./git")).fetchOrigin.mockRejectedValueOnce(
      new Error("HTTP Error: 401 Unauthorized"),
    );
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
        "channels/general.meta.yaml",
        "channels/general.thread",
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
        "archive/channels/general.meta.yaml",
        "archive/channels/general.thread",
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

  it("fast-forwards poll by updating the local branch", async () => {
    const git = vi.mocked(await import("./git"));
    setState({ defaultBranch: "trunk", headCommit: "local-head" });
    git.resolveRemoteHead.mockResolvedValueOnce("remote-head");
    git.resolveHead
      .mockResolvedValueOnce("local-head")
      .mockResolvedValueOnce("remote-head");

    const res = await poll("local-head");

    expect(res.ok).toBe(true);
    expect(git.resetToRemote).toHaveBeenCalledWith(
      "/repo",
      "refs/remotes/origin/trunk",
    );
    expect(git.checkout).not.toHaveBeenCalled();
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
});
