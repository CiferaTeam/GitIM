import { beforeEach, describe, expect, it, vi } from "vitest";

const files = vi.hoisted(() => new Map<string, string>());
const dirs = vi.hoisted(() => new Map<string, string[]>());
const commits = vi.hoisted(() => [] as Array<{ filepaths: string[]; message: string }>);

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

import {
  channels,
  createCard,
  listCards,
  poll,
  read,
  readCard,
  send,
  sendCardMessage,
  thread,
  updateCard,
  joinChannel,
} from "./handlers";
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
  dirs.set("/repo/channels/general/cards", ["20260317-120000-abc"]);
  dirs.set("/repo/channels/general/cards/20260317-120000-abc", [
    "card.meta.yaml",
    "discussion.thread",
  ]);
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
});
