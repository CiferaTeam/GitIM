// API handlers for daemon-web — implements the Backend interface methods.
// Each function mirrors what gitim-runtime returns over HTTP.

import * as gitOps from "./git";
import {
  readFile,
  writeFile,
  readdir,
  exists,
  mkdir,
  stat,
  removeFile,
  removeDir,
  configureFs,
  type StorageConfig,
} from "./storage";
import { getState, setState, type ChannelMeta, type UserMeta } from "./state";
import { parseThread, type ThreadEntry } from "./parser";
import { formatMessage, formatEvent } from "./formatter";
import { runSync } from "./sync";
import initWasm, {
  parseCardMeta,
  stringifyCardMeta,
  validateCardId,
  validateCardLabels,
} from "gitim-wasm";
import {
  channelMetaPath,
  channelNameFromMetaFile,
  dmApiNameFromThreadPath,
  resolveThreadTarget,
  validateChannelName,
} from "./paths";
import type { Card, CardStatus } from "../lib/types";
import { tokenAuth } from "./auth";

type ApiResponse = {
  ok: boolean;
  data?: Record<string, unknown>;
  error?: string;
};

type RawCardMeta = Omit<Card, "card_id">;

export interface CreateCardOptions {
  labels?: string[];
  assignee?: string | null;
  status?: CardStatus;
}

export interface ListCardsQuery {
  channel?: string | null;
  labels?: string[];
  status?: CardStatus | null;
  assignee?: string | null;
}

export interface ReadCardQuery {
  limit?: number;
  since?: number;
}

export interface UpdateCardPatch {
  status?: CardStatus;
  labels?: string[];
  assignee?: string | null;
}

interface LocatedCard {
  relDir: string;
  absDir: string;
  archived: boolean;
}

let wasmReady: Promise<void> | null = null;

function ok(data: Record<string, unknown> = {}): ApiResponse {
  return { ok: true, data };
}

function err(error: string): ApiResponse {
  return { ok: false, error };
}

function errCode(error: string, error_code: string): ApiResponse & { error_code: string } {
  return { ok: false, error, error_code };
}

function reconnectRequired(): ApiResponse & { error_code: string } {
  return errCode(
    "Reconnect token to send from this browser workspace.",
    "reconnect_required",
  );
}

function errorMessage(e: unknown): string {
  return String((e as Error).message ?? e);
}

async function syncAfterCommit(): Promise<{ status: "pushed" | "commit_only"; error?: string }> {
  try {
    await runSync({ forceNewCycle: true });
    return { status: "pushed" };
  } catch (e) {
    return { status: "commit_only", error: errorMessage(e) };
  }
}

async function ensureWasmReady(): Promise<void> {
  wasmReady ??= initWasm().then(() => undefined);
  await wasmReady;
}

// --- Browser runtime preflight ---

export async function preflight(): Promise<ApiResponse> {
  try {
    await ensureWasmReady();
    const oid = await gitOps.hashEmptyBlob();
    if (oid !== "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391") {
      return err("browser git hashing is unavailable");
    }
    await stat("/");
    return ok({ runtime: "browser", storage: "ready", git: "ready" });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

// --- Init ---

export async function init(config: {
  workspaceId?: string;
  remoteUrl: string;
  corsProxy: string;
  token: string | null;
  handler: string;
  storage?: StorageConfig;
}): Promise<ApiResponse> {
  const { initState } = await import("./state");
  const storage = config.storage ?? { fsName: "gitim", repoDir: "/repo" as const };
  const workspaceId = config.workspaceId ?? "local";
  const dir = storage.repoDir;
  configureFs(storage.fsName);

  try {
    const repoExists = await exists(`${dir}/.git`);
    if (!repoExists && !config.token) {
      return errCode(
        "Reconnect token to clone this browser workspace.",
        "reconnect_required",
      );
    }
    if (!repoExists && config.token) {
      const onAuth = tokenAuth(config.token);
      await gitOps.cloneRepo(config.remoteUrl, dir, config.corsProxy, onAuth);
    }

    // Detect default branch
    const branch = await gitOps.getCurrentBranch(dir);

    // Read user meta to get display_name
    let displayName = config.handler;
    const userMetaPath = `${dir}/users/${config.handler}.meta.yaml`;
    if (await exists(userMetaPath)) {
      const content = await readFile(userMetaPath);
      const meta = parseYaml(content);
      if (meta.display_name) displayName = meta.display_name as string;
    }

    const s = initState({
      workspaceId,
      repoDir: dir,
      remoteUrl: config.remoteUrl,
      fsName: storage.fsName,
      corsProxy: config.corsProxy,
      token: config.token,
      handler: config.handler,
      displayName,
    });
    s.defaultBranch = branch;

    // Cache initial state
    const head = await gitOps.resolveHead(dir);
    setState({ headCommit: head });
    await refreshChannelsCache();
    await refreshUsersCache();

    return ok({
      handler: config.handler,
      display_name: displayName,
      sync_enabled: !!config.token,
      needs_token: !config.token,
    });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

// --- IM handlers ---

export async function health(): Promise<ApiResponse> {
  try {
    const s = getState();
    return ok({
      service: "daemon-web",
      initialized: true,
      workspace: s.workspaceId,
      sync_enabled: !!s.token,
      needs_token: !s.token,
    });
  } catch {
    return ok({ service: "daemon-web", initialized: false });
  }
}

export async function me(): Promise<ApiResponse> {
  const s = getState();
  return ok({
    handler: s.me.handler,
    display_name: s.me.display_name,
    guest: false,
  });
}

export async function poll(since?: string): Promise<ApiResponse> {
  const s = getState();
  if (!s.token) {
    return ok({
      commit_id: s.headCommit,
      changes: [],
      sync_enabled: false,
      needs_token: true,
    });
  }
  const onAuth = tokenAuth(s.token);

  try {
    // Fetch from remote
    await gitOps.fetchOrigin(s.repoDir, s.corsProxy, onAuth);
    const remoteHead = await gitOps.resolveRemoteHead(s.repoDir);
    const localHead = await gitOps.resolveHead(s.repoDir);

    // If remote has new commits, fast-forward (sync handles conflicts separately)
    if (remoteHead !== localHead && localHead === s.headCommit) {
      await gitOps.resetToRemote(
        s.repoDir,
        `refs/remotes/origin/${s.defaultBranch}`,
      );
      setState({ headCommit: remoteHead });
    }

    const currentHead = await gitOps.resolveHead(s.repoDir);

    if (!since || since === currentHead) {
      return ok({ commit_id: currentHead, changes: [] });
    }

    // Diff to find changed files
    let changedFiles: string[];
    try {
      changedFiles = await gitOps.diffTrees(s.repoDir, since, currentHead);
    } catch {
      // Stale cursor — return all channels as changed
      changedFiles = [];
      await refreshChannelsCache();
      const changes = [];
      for (const [name] of s.channels) {
        const entries = await readChannelEntries(name);
        changes.push({ channel: name, kind: "new_messages", entries });
      }
      return ok({ commit_id: currentHead, changes });
    }

    // Build changes from diff
    const changes = [];
    let metaChanged = false;

    for (const fp of changedFiles) {
      const cardChange = cardChangeFromPath(fp);
      if (cardChange) {
        const scope = `card:${cardChange.channel}/${cardChange.cardId}`;
        if (cardChange.file === "meta") {
          changes.push({ channel: scope, kind: "card_meta" });
        } else {
          const entries = await readCardEntries(cardChange.channel, cardChange.cardId);
          changes.push({ channel: scope, kind: "card_thread", entries });
        }
      } else if (fp.startsWith("channels/") && fp.endsWith(".thread")) {
        const channelName = fp
          .replace("channels/", "")
          .replace(".thread", "");
        const entries = await readChannelEntries(channelName);
        changes.push({ channel: channelName, kind: "new_messages", entries });
      } else if (fp.startsWith("dm/") && fp.endsWith(".thread")) {
        const dmName = dmApiNameFromThreadPath(fp);
        if (!dmName) continue;
        const entries = await readChannelEntries(dmName);
        changes.push({ channel: dmName, kind: "new_messages", entries });
      } else if (fp.startsWith("archive/channels/")) {
        const name = fp.replace("archive/channels/", "");
        if (name.includes("/")) continue;
        const channelName = name
          .replace(".meta.yaml", "")
          .replace(".thread", "");
        if (validateChannelName(channelName)) continue;
        changes.push({ channel: channelName, kind: "channel_meta" });
        metaChanged = true;
      } else if (fp.includes("meta.yaml")) {
        metaChanged = true;
      }
    }

    if (metaChanged) {
      await refreshChannelsCache();
      await refreshUsersCache();
    }

    setState({ headCommit: currentHead });
    return ok({ commit_id: currentHead, changes });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function channels(): Promise<ApiResponse> {
  const s = getState();
  await refreshChannelsCache();

  const channelList: Array<{
    name: string;
    kind: string;
    unreadCount: number;
    members: string[];
  }> = [];

  for (const [name, meta] of s.channels) {
    const isDm = name.includes("--");
    // For channels, only show if current user is a member
    if (!isDm && meta.members.length > 0 && !meta.members.includes(s.me.handler)) {
      continue;
    }

    channelList.push({
      name,
      kind: isDm ? "dm" : "channel",
      unreadCount: 0,
      members: meta.members,
    });
  }

  return ok({ channels: channelList });
}

export async function read(
  channel: string,
  limit?: number,
): Promise<ApiResponse> {
  try {
    const { entries, archived } = await readChannelEntriesWithArchive(channel);
    const limited = limit ? entries.slice(-limit) : entries;
    return ok({ channel, entries: limited, archived });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function send(
  channel: string,
  body: string,
  _author?: string,
  replyTo?: number,
): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    const target = resolveThreadTarget(channel);
    const filePath = target.threadPath;
    const absPath = `${s.repoDir}/${filePath}`;

    if (target.kind === "channel") {
      const metaPath = `${s.repoDir}/${channelMetaPath(target.name)}`;
      if (!(await exists(metaPath))) {
        return err(`channel '${target.name}' not found`);
      }
      const meta = parseYaml(await readFile(metaPath)) as unknown as ChannelMeta;
      if (meta.members.length > 0 && !meta.members.includes(s.me.handler)) {
        return err("not_member");
      }
    } else {
      if (!target.members.includes(s.me.handler)) {
        return err("not_dm_participant");
      }
      for (const member of target.members) {
        if (!(await exists(`${s.repoDir}/users/${member}.meta.yaml`))) {
          return err(`unknown DM participant: ${member}`);
        }
      }
    }

    // Read existing content
    let existing = "";
    if (await exists(absPath)) {
      existing = await readFile(absPath);
    } else {
      await mkdir(`${s.repoDir}/${target.kind === "dm" ? "dm" : "channels"}`);
    }

    // Find next line number
    const file = parseThread(existing);
    const maxLine =
      file.entries.length > 0
        ? Math.max(...file.entries.map((e) => e.line_number))
        : 0;
    const nextLine = maxLine + 1;

    // Generate timestamp
    const now = new Date();
    const pad = (n: number, len = 2) => String(n).padStart(len, "0");
    const timestamp =
      `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}` +
      `T${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}Z`;

    const line = formatMessage(
      nextLine,
      replyTo ?? 0,
      s.me.handler,
      timestamp,
      body,
    );

    // Append to file
    let newContent = existing;
    if (newContent && !newContent.endsWith("\n")) newContent += "\n";
    newContent += line;
    await writeFile(absPath, newContent);

    // Commit
    await gitOps.addAndCommit(
      s.repoDir,
      [filePath],
      `msg: @${s.me.handler} -> ${target.name} L${String(nextLine).padStart(6, "0")}`,
      s.me.handler,
    );

    const sync = await syncAfterCommit();

    return ok({ line_number: nextLine, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function thread(
  channel: string,
  line: number,
): Promise<ApiResponse> {
  try {
    const entries = await readChannelEntries(channel);
    // Build thread tree: root message + all replies pointing to it
    const root = entries.find((e) => e.line_number === line);
    if (!root) return ok({ messages: [] });

    const threadMessages = [root];
    const collectReplies = (parentLine: number) => {
      for (const e of entries) {
        if (e.point_to === parentLine) {
          threadMessages.push(e);
          collectReplies(e.line_number);
        }
      }
    };
    collectReplies(line);

    return ok({ entries: threadMessages });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function users(): Promise<ApiResponse> {
  const s = getState();
  await refreshUsersCache();
  const userList = Array.from(s.users.keys());
  return ok({ users: userList });
}

export async function joinChannel(channel: string): Promise<ApiResponse> {
  const s = getState();
  if (!s.token) return reconnectRequired();
  const invalidChannel = validateChannelName(channel);
  if (invalidChannel) return err(invalidChannel);
  const metaPath = `${s.repoDir}/${channelMetaPath(channel)}`;

  try {
    if (!(await exists(metaPath))) {
      return err(`channel '${channel}' not found`);
    }

    const content = await readFile(metaPath);
    const meta = parseYaml(content) as unknown as ChannelMeta;

    if (meta.members.includes(s.me.handler)) {
      return ok({ already_member: true });
    }

    meta.members.push(s.me.handler);
    meta.members.sort();

    const newYaml = stringifyYaml(meta);
    await writeFile(metaPath, newYaml);

    // Write join event to thread
    const threadPath = `channels/${channel}.thread`;
    const absThreadPath = `${s.repoDir}/${threadPath}`;
    let existing = "";
    if (await exists(absThreadPath)) {
      existing = await readFile(absThreadPath);
    }
    const file = parseThread(existing);
    const maxLine =
      file.entries.length > 0
        ? Math.max(...file.entries.map((e) => e.line_number))
        : 0;
    const nextLine = maxLine + 1;

    const now = new Date();
    const pad = (n: number, len = 2) => String(n).padStart(len, "0");
    const timestamp =
      `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}` +
      `T${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}Z`;

    const event = formatEvent(nextLine, s.me.handler, timestamp, "join", {
      members: [s.me.handler],
    });

    let newContent = existing;
    if (newContent && !newContent.endsWith("\n")) newContent += "\n";
    newContent += event;
    await writeFile(absThreadPath, newContent);

    // Commit both files
    await gitOps.addAndCommit(
      s.repoDir,
      [channelMetaPath(channel), threadPath],
      `join: @${s.me.handler} -> ${channel}`,
      s.me.handler,
    );

    await refreshChannelsCache();
    const sync = await syncAfterCommit();

    return ok(sync);
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function archiveChannel(channel: string): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    const invalidChannel = validateChannelName(channel);
    if (invalidChannel) return err(invalidChannel);

    const metaRelPath = channelMetaPath(channel);
    const metaAbsPath = `${s.repoDir}/${metaRelPath}`;
    if (!(await exists(metaAbsPath))) {
      return err(`channel '${channel}' does not exist`);
    }

    const meta = parseYaml(await readFile(metaAbsPath)) as unknown as ChannelMeta;
    if (meta.created_by !== s.me.handler) {
      return err("only channel creator can archive");
    }

    const archiveMetaPath = `${s.repoDir}/archive/channels/${channel}.meta.yaml`;
    if (await exists(archiveMetaPath)) {
      return err(`channel '${channel}' is already archived`);
    }

    await moveChannelFiles(
      channel,
      {
        metaRelPath,
        threadRelPath: `channels/${channel}.thread`,
      },
      {
        metaRelPath: `archive/channels/${channel}.meta.yaml`,
        threadRelPath: `archive/channels/${channel}.thread`,
      },
      `archive: #${channel} by @${s.me.handler}`,
    );

    await refreshChannelsCache();
    const sync = await syncAfterCommit();
    return ok({ channel, archived_by: s.me.handler, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function unarchiveChannel(channel: string): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    const invalidChannel = validateChannelName(channel);
    if (invalidChannel) return err(invalidChannel);

    const archiveMetaRelPath = `archive/channels/${channel}.meta.yaml`;
    const archiveMetaAbsPath = `${s.repoDir}/${archiveMetaRelPath}`;
    if (!(await exists(archiveMetaAbsPath))) {
      return err(`archive source does not exist for channel '${channel}'`);
    }

    const meta = parseYaml(await readFile(archiveMetaAbsPath)) as unknown as ChannelMeta;
    if (meta.created_by !== s.me.handler) {
      return err("only channel creator can unarchive");
    }

    const activeMetaRelPath = channelMetaPath(channel);
    if (await exists(`${s.repoDir}/${activeMetaRelPath}`)) {
      return err(`channel '${channel}' already exists in active location; unarchive aborted`);
    }

    await moveChannelFiles(
      channel,
      {
        metaRelPath: archiveMetaRelPath,
        threadRelPath: `archive/channels/${channel}.thread`,
      },
      {
        metaRelPath: activeMetaRelPath,
        threadRelPath: `channels/${channel}.thread`,
      },
      `unarchive: #${channel} by @${s.me.handler}`,
    );

    await refreshChannelsCache();
    const sync = await syncAfterCommit();
    return ok({ channel, unarchived_by: s.me.handler, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function listArchivedChannels(): Promise<ApiResponse> {
  try {
    const s = getState();
    const archiveChannelsDir = `${s.repoDir}/archive/channels`;
    if (!(await exists(archiveChannelsDir))) return ok({ channels: [] });

    const items = await readdir(archiveChannelsDir);
    const archivedChannels: Array<{
      name: string;
      kind: string;
      members: string[];
    }> = [];

    for (const item of items) {
      const channelName = channelNameFromMetaFile(item);
      if (!channelName) continue;

      const meta = parseYaml(
        await readFile(`${archiveChannelsDir}/${item}`),
      ) as unknown as ChannelMeta;
      const members = meta.members ?? [];
      if (members.length > 0 && !members.includes(s.me.handler)) {
        continue;
      }

      archivedChannels.push({
        name: channelName,
        kind: "archived_channel",
        members,
      });
    }

    archivedChannels.sort((a, b) => a.name.localeCompare(b.name));
    return ok({ channels: archivedChannels });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

// --- Card handlers ---

export async function listCards(
  query: ListCardsQuery = {},
): Promise<ApiResponse> {
  try {
    await ensureWasmReady();
    await refreshChannelsCache();
    const s = getState();

    const channelNames = query.channel
      ? [query.channel]
      : Array.from(s.channels.keys()).filter((name) => !name.includes("--"));
    const cards: Card[] = [];

    for (const channel of channelNames) {
      if (!channel) continue;
      const invalidChannel = validateChannelName(channel);
      if (invalidChannel) return err(invalidChannel);

      const cardsDir = `${s.repoDir}/channels/${channel}/cards`;
      if (!(await exists(cardsDir))) continue;

      const cardIds = await readdir(cardsDir);
      for (const cardId of cardIds) {
        const metaPath = `${cardsDir}/${cardId}/card.meta.yaml`;
        if (!(await exists(metaPath))) continue;

        try {
          const card = await readCardMeta(channel, cardId, metaPath);
          if (matchesCardQuery(card, query)) cards.push(card);
        } catch {
          continue;
        }
      }
    }

    cards.sort((a, b) => a.channel.localeCompare(b.channel) || a.card_id.localeCompare(b.card_id));
    return ok({ cards });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function createCard(
  channel: string,
  title: string,
  opts: CreateCardOptions = {},
): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    await ensureWasmReady();
    const invalidChannel = validateChannelName(channel);
    if (invalidChannel) return err(invalidChannel);

    const channelMeta = `${s.repoDir}/${channelMetaPath(channel)}`;
    if (!(await exists(channelMeta))) return err(`channel '${channel}' not found`);
    const meta = parseYaml(await readFile(channelMeta)) as unknown as ChannelMeta;
    if (meta.members.length > 0 && !meta.members.includes(s.me.handler)) {
      return err("not_member");
    }

    await refreshUsersCache();
    if (opts.assignee && !s.users.has(opts.assignee)) {
      return err(`assignee invalid: unknown user: ${opts.assignee}`);
    }
    if (opts.labels) validateCardLabels(opts.labels);

    const cardId = generateCardId();
    const now = utcTimestamp();
    const card: RawCardMeta = {
      title: title.trim(),
      channel,
      status: opts.status ?? "todo",
      labels: opts.labels ?? [],
      assignee: opts.assignee ?? null,
      created_by: s.me.handler,
      created_at: now,
      updated_at: now,
    };
    const yaml = stringifyCardMeta(card) as string;

    const relDir = `channels/${channel}/cards/${cardId}`;
    const absDir = `${s.repoDir}/${relDir}`;
    await mkdirp(absDir);
    await writeFile(`${absDir}/card.meta.yaml`, yaml);
    await writeFile(`${absDir}/discussion.thread`, "");

    await gitOps.addAndCommit(
      s.repoDir,
      [`${relDir}/card.meta.yaml`, `${relDir}/discussion.thread`],
      `card: create ${cardId} in ${channel} by @${s.me.handler}`,
      s.me.handler,
    );

    const sync = await syncAfterCommit();
    return ok({ channel, card_id: cardId, title: card.title, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function readCard(
  channel: string,
  cardId: string,
  query: ReadCardQuery = {},
): Promise<ApiResponse> {
  try {
    await ensureWasmReady();
    const located = await locateCard(channel, cardId);
    const card = await readCardMeta(channel, cardId, `${located.absDir}/card.meta.yaml`);
    const entries = await readCardEntries(channel, cardId, query.limit, query.since);
    return ok({
      channel,
      card_id: cardId,
      archived: located.archived,
      meta: card,
      entries,
    });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function sendCardMessage(
  channel: string,
  cardId: string,
  body: string,
  replyTo?: number,
): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    const located = await locateActiveCard(channel, cardId);
    const threadPath = `${located.absDir}/discussion.thread`;
    const existing = (await exists(threadPath)) ? await readFile(threadPath) : "";
    const file = parseThread(existing);
    const maxLine =
      file.entries.length > 0
        ? Math.max(...file.entries.map((e) => e.line_number))
        : 0;
    const nextLine = maxLine + 1;
    const line = formatMessage(
      nextLine,
      replyTo ?? 0,
      s.me.handler,
      utcTimestamp(),
      body,
    );
    let nextContent = existing;
    if (nextContent && !nextContent.endsWith("\n")) nextContent += "\n";
    nextContent += line;
    await writeFile(threadPath, nextContent);

    const relPath = `${located.relDir}/discussion.thread`;
    await gitOps.addAndCommit(
      s.repoDir,
      [relPath],
      `card-msg: @${s.me.handler} -> ${channel}/${cardId} L${String(nextLine).padStart(6, "0")}`,
      s.me.handler,
    );

    const sync = await syncAfterCommit();
    return ok({ line_number: nextLine, channel, card_id: cardId, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function updateCard(
  channel: string,
  cardId: string,
  patch: UpdateCardPatch,
): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    await ensureWasmReady();
    const located = await locateActiveCard(channel, cardId);
    if (
      patch.status === undefined &&
      patch.labels === undefined &&
      patch.assignee === undefined
    ) {
      return err("must provide at least one field to update");
    }
    await refreshUsersCache();
    if (patch.assignee && !s.users.has(patch.assignee)) {
      return err(`assignee invalid: unknown user: ${patch.assignee}`);
    }
    if (patch.labels) validateCardLabels(patch.labels);

    const metaPath = `${located.absDir}/card.meta.yaml`;
    const card = await readCardMeta(channel, cardId, metaPath);
    const next: RawCardMeta = {
      title: card.title,
      channel: card.channel,
      status: patch.status ?? card.status,
      labels: patch.labels ?? card.labels,
      assignee: patch.assignee !== undefined ? patch.assignee : card.assignee,
      created_by: card.created_by,
      created_at: card.created_at,
      updated_at: utcTimestamp(),
    };
    await writeFile(metaPath, stringifyCardMeta(next) as string);

    const relPath = `${located.relDir}/card.meta.yaml`;
    await gitOps.addAndCommit(
      s.repoDir,
      [relPath],
      `card: update ${cardId} in ${channel} by @${s.me.handler}`,
      s.me.handler,
    );

    const sync = await syncAfterCommit();
    return ok({
      channel,
      card_id: cardId,
      status: next.status,
      labels: next.labels,
      assignee: next.assignee,
      sync_status: sync.status,
      sync_error: sync.error,
    });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function archiveCard(
  channel: string,
  cardId: string,
): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    await ensureWasmReady();
    const located = await locateActiveCard(channel, cardId);
    const card = await readCardMeta(channel, cardId, `${located.absDir}/card.meta.yaml`);
    const permissionError = checkCardArchivePermission(card, s.me.handler, "archive");
    if (permissionError) return err(permissionError);

    const targetRelDir = `archive/channels/${channel}/cards/${cardId}`;
    const targetAbsDir = `${s.repoDir}/${targetRelDir}`;
    await moveCardDirectory(
      located,
      { relDir: targetRelDir, absDir: targetAbsDir, archived: true },
      `card: archive ${cardId} in ${channel} by @${s.me.handler}`,
    );

    const sync = await syncAfterCommit();
    return ok({ channel, card_id: cardId, archived_by: s.me.handler, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function unarchiveCard(
  channel: string,
  cardId: string,
): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    await ensureWasmReady();
    const located = await locateCard(channel, cardId);
    if (!located.archived) return err(`card '${cardId}' is not archived`);
    const channelMeta = `${s.repoDir}/${channelMetaPath(channel)}`;
    if (!(await exists(channelMeta))) {
      return err(`cannot unarchive card: channel '${channel}' is not active`);
    }
    const card = await readCardMeta(channel, cardId, `${located.absDir}/card.meta.yaml`);
    const permissionError = checkCardArchivePermission(card, s.me.handler, "unarchive");
    if (permissionError) return err(permissionError);

    const targetRelDir = `channels/${channel}/cards/${cardId}`;
    const targetAbsDir = `${s.repoDir}/${targetRelDir}`;
    await moveCardDirectory(
      located,
      { relDir: targetRelDir, absDir: targetAbsDir, archived: false },
      `card: unarchive ${cardId} in ${channel} by @${s.me.handler}`,
    );

    const sync = await syncAfterCommit();
    return ok({ channel, card_id: cardId, unarchived_by: s.me.handler, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

export async function listArchivedCards(
  channel?: string,
): Promise<ApiResponse> {
  try {
    await ensureWasmReady();
    const s = getState();
    const archiveChannelsDir = `${s.repoDir}/archive/channels`;
    if (!(await exists(archiveChannelsDir))) return ok({ cards: [] });

    const channelNames = channel ? [channel] : await readdir(archiveChannelsDir);
    const cards: Card[] = [];
    for (const name of channelNames) {
      const invalidChannel = validateChannelName(name);
      if (invalidChannel) return err(invalidChannel);

      const cardsDir = `${archiveChannelsDir}/${name}/cards`;
      if (!(await exists(cardsDir))) continue;
      const cardIds = await readdir(cardsDir);
      for (const cardId of cardIds) {
        const metaPath = `${cardsDir}/${cardId}/card.meta.yaml`;
        if (!(await exists(metaPath))) continue;
        try {
          cards.push(await readCardMeta(name, cardId, metaPath));
        } catch {
          continue;
        }
      }
    }

    cards.sort((a, b) => a.channel.localeCompare(b.channel) || a.card_id.localeCompare(b.card_id));
    return ok({ cards });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

// --- Internal helpers ---

async function readChannelEntries(
  channel: string,
): Promise<ThreadEntry[]> {
  return (await readChannelEntriesWithArchive(channel)).entries;
}

async function readChannelEntriesWithArchive(
  channel: string,
): Promise<{ entries: ThreadEntry[]; archived: boolean }> {
  const s = getState();
  const target = resolveThreadTarget(channel);
  let absPath = `${s.repoDir}/${target.threadPath}`;
  let archived = false;

  if (target.kind === "channel" && !(await exists(absPath))) {
    const archivePath = `${s.repoDir}/archive/channels/${target.name}.thread`;
    if (await exists(archivePath)) {
      absPath = archivePath;
      archived = true;
    }
  }

  if (!(await exists(absPath))) return { entries: [], archived };

  const content = await readFile(absPath);
  const file = parseThread(content);
  return { entries: file.entries, archived };
}

async function readCardEntries(
  channel: string,
  cardId: string,
  limit?: number,
  since?: number,
): Promise<ThreadEntry[]> {
  const located = await locateCard(channel, cardId);
  const threadPath = `${located.absDir}/discussion.thread`;
  if (!(await exists(threadPath))) return [];
  const content = await readFile(threadPath);
  const file = parseThread(content);
  let entries = file.entries;
  if (since != null) entries = entries.filter((e) => e.line_number > since);
  if (limit != null) entries = entries.slice(-limit);
  return entries;
}

async function locateActiveCard(
  channel: string,
  cardId: string,
): Promise<LocatedCard> {
  const located = await locateCard(channel, cardId);
  if (located.archived) {
    throw new Error(`card '${cardId}' is archived`);
  }
  return located;
}

async function locateCard(
  channel: string,
  cardId: string,
): Promise<LocatedCard> {
  await ensureWasmReady();
  const s = getState();
  const invalidChannel = validateChannelName(channel);
  if (invalidChannel) throw new Error(invalidChannel);
  validateCardId(cardId);
  const relDir = `channels/${channel}/cards/${cardId}`;
  const absDir = `${s.repoDir}/${relDir}`;
  if (await exists(`${absDir}/card.meta.yaml`)) {
    return { relDir, absDir, archived: false };
  }

  const archivedRelDir = `archive/channels/${channel}/cards/${cardId}`;
  const archivedAbsDir = `${s.repoDir}/${archivedRelDir}`;
  if (await exists(`${archivedAbsDir}/card.meta.yaml`)) {
    return { relDir: archivedRelDir, absDir: archivedAbsDir, archived: true };
  }

  throw new Error(`card '${cardId}' not found in channel '${channel}'`);
}

async function readCardMeta(
  channel: string,
  cardId: string,
  metaPath: string,
): Promise<Card> {
  const meta = parseCardMeta(await readFile(metaPath)) as RawCardMeta;
  return {
    card_id: cardId,
    channel: meta.channel || channel,
    title: meta.title,
    status: meta.status,
    labels: meta.labels ?? [],
    assignee: meta.assignee ?? null,
    created_by: meta.created_by,
    created_at: meta.created_at,
    updated_at: meta.updated_at,
  };
}

function matchesCardQuery(card: Card, query: ListCardsQuery): boolean {
  if (query.status && card.status !== query.status) return false;
  if (query.assignee && card.assignee !== query.assignee) return false;
  if (query.labels && query.labels.length > 0) {
    const labels = new Set(card.labels);
    for (const label of query.labels) {
      if (!labels.has(label)) return false;
    }
  }
  return true;
}

async function mkdirp(path: string): Promise<void> {
  const parts = path.split("/").filter(Boolean);
  let current = path.startsWith("/") ? "" : ".";
  for (const part of parts) {
    current = current === "" ? `/${part}` : `${current}/${part}`;
    if (!(await exists(current))) await mkdir(current);
  }
}

async function moveCardDirectory(
  from: LocatedCard,
  to: LocatedCard,
  message: string,
): Promise<void> {
  const s = getState();
  const meta = await readFile(`${from.absDir}/card.meta.yaml`);
  const thread = (await exists(`${from.absDir}/discussion.thread`))
    ? await readFile(`${from.absDir}/discussion.thread`)
    : "";

  await mkdirp(to.absDir);
  await writeFile(`${to.absDir}/card.meta.yaml`, meta);
  await writeFile(`${to.absDir}/discussion.thread`, thread);

  await gitOps.addRemoveAndCommit(
    s.repoDir,
    [`${to.relDir}/card.meta.yaml`, `${to.relDir}/discussion.thread`],
    [`${from.relDir}/card.meta.yaml`, `${from.relDir}/discussion.thread`],
    message,
    s.me.handler,
  );

  await removeTrackedFile(`${from.absDir}/card.meta.yaml`);
  await removeTrackedFile(`${from.absDir}/discussion.thread`);
  try {
    await removeDir(from.absDir);
  } catch {
    // Empty directory cleanup is best-effort; git tracks files, not directories.
  }
}

async function moveChannelFiles(
  channel: string,
  from: { metaRelPath: string; threadRelPath: string },
  to: { metaRelPath: string; threadRelPath: string },
  message: string,
): Promise<void> {
  const s = getState();
  const fromMetaAbsPath = `${s.repoDir}/${from.metaRelPath}`;
  const fromThreadAbsPath = `${s.repoDir}/${from.threadRelPath}`;
  const toMetaAbsPath = `${s.repoDir}/${to.metaRelPath}`;
  const toThreadAbsPath = `${s.repoDir}/${to.threadRelPath}`;

  const meta = await readFile(fromMetaAbsPath);
  if (!(await exists(fromThreadAbsPath))) {
    throw new Error(`thread file for channel '${channel}' does not exist`);
  }
  const thread = await readFile(fromThreadAbsPath);

  await mkdirp(parentPath(toMetaAbsPath));
  await writeFile(toMetaAbsPath, meta);
  await writeFile(toThreadAbsPath, thread);

  await gitOps.addRemoveAndCommit(
    s.repoDir,
    [to.metaRelPath, to.threadRelPath],
    [from.metaRelPath, from.threadRelPath],
    message,
    s.me.handler,
  );

  await removeTrackedFile(fromMetaAbsPath);
  await removeTrackedFile(fromThreadAbsPath);
}

async function removeTrackedFile(path: string): Promise<void> {
  try {
    await removeFile(path);
  } catch {
    // isomorphic-git stages deletion separately from storage cleanup.
  }
}

function parentPath(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx <= 0 ? "/" : path.slice(0, idx);
}

function checkCardArchivePermission(
  card: Card,
  handler: string,
  action: "archive" | "unarchive",
): string | null {
  if (card.created_by === handler || card.assignee === handler) return null;
  return `only creator or assignee can ${action}`;
}

function generateCardId(): string {
  const now = new Date();
  const pad = (n: number, len = 2) => String(n).padStart(len, "0");
  const ts =
    `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}` +
    `-${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}`;
  const rand = Math.floor(Math.random() * 0x1000)
    .toString(16)
    .padStart(3, "0");
  return `${ts}-${rand}`;
}

function utcTimestamp(): string {
  const now = new Date();
  const pad = (n: number, len = 2) => String(n).padStart(len, "0");
  return (
    `${now.getUTCFullYear()}${pad(now.getUTCMonth() + 1)}${pad(now.getUTCDate())}` +
    `T${pad(now.getUTCHours())}${pad(now.getUTCMinutes())}${pad(now.getUTCSeconds())}Z`
  );
}

function cardChangeFromPath(
  path: string,
): { channel: string; cardId: string; file: "meta" | "thread" } | null {
  const match = path.match(
    /^(?:archive\/)?channels\/([^/]+)\/cards\/([^/]+)\/(card\.meta\.yaml|discussion\.thread)$/,
  );
  if (!match) return null;
  return {
    channel: match[1],
    cardId: match[2],
    file: match[3] === "card.meta.yaml" ? "meta" : "thread",
  };
}

async function refreshChannelsCache(): Promise<void> {
  const s = getState();
  const channelsMap = new Map<string, ChannelMeta>();

  // Scan channels/
  const channelsDir = `${s.repoDir}/channels`;
  if (await exists(channelsDir)) {
    const items = await readdir(channelsDir);
    for (const item of items) {
      const channelName = channelNameFromMetaFile(item);
      if (!channelName) continue;
      const metaPath = `${channelsDir}/${item}`;
      if (await exists(metaPath)) {
        const content = await readFile(metaPath);
        const meta = parseYaml(content) as unknown as ChannelMeta;
        channelsMap.set(channelName, meta);
      }
    }
  }

  // Scan dm/
  const dmDir = `${s.repoDir}/dm`;
  if (await exists(dmDir)) {
    const items = await readdir(dmDir);
    for (const item of items) {
      if (!item.endsWith(".thread")) continue;
      const dmName = item.replace(".thread", "");
      let target: ReturnType<typeof resolveThreadTarget>;
      try {
        target = resolveThreadTarget(dmName);
      } catch {
        continue;
      }
      if (target.kind !== "dm") continue;
      channelsMap.set(dmName, {
        display_name: dmName,
        created_by: "",
        created_at: "",
        introduction: "",
        members: [...target.members],
      });
    }
  }

  setState({ channels: channelsMap });
}

async function refreshUsersCache(): Promise<void> {
  const s = getState();
  const usersMap = new Map<string, UserMeta>();

  const usersDir = `${s.repoDir}/users`;
  if (await exists(usersDir)) {
    const items = await readdir(usersDir);
    for (const item of items) {
      if (!item.endsWith(".meta.yaml")) continue;
      const handler = item.replace(".meta.yaml", "");
      const content = await readFile(`${usersDir}/${item}`);
      const meta = parseYaml(content) as unknown as UserMeta;
      usersMap.set(handler, meta);
    }
  }

  setState({ users: usersMap });
}

// Minimal YAML parser — only handles flat key: value and list items
function parseYaml(yaml: string): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  let currentKey: string | null = null;
  let currentList: string[] | null = null;

  for (const line of yaml.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;

    // List item: "  - value"
    if (trimmed.startsWith("- ") && currentKey && currentList) {
      currentList.push(trimmed.slice(2).trim());
      continue;
    }

    // Key-value pair
    const colonIdx = trimmed.indexOf(":");
    if (colonIdx === -1) continue;

    // Save previous list if any
    if (currentKey && currentList) {
      result[currentKey] = currentList;
      currentList = null;
    }

    const key = trimmed.slice(0, colonIdx).trim();
    const value = trimmed.slice(colonIdx + 1).trim();

    if (value === "" || value === "[]") {
      // Start of a list or empty value
      currentKey = key;
      currentList = [];
      if (value === "[]") {
        result[key] = [];
        currentList = null;
        currentKey = null;
      }
    } else {
      currentKey = null;
      currentList = null;
      // Strip quotes
      result[key] = value.replace(/^["']|["']$/g, "");
    }
  }

  // Save trailing list
  if (currentKey && currentList) {
    result[currentKey] = currentList;
  }

  return result;
}

// Minimal YAML stringifier for ChannelMeta
function stringifyYaml(obj: object): string {
  let yaml = "";
  for (const [key, value] of Object.entries(obj)) {
    if (Array.isArray(value)) {
      yaml += `${key}:\n`;
      for (const item of value) {
        yaml += `  - ${item}\n`;
      }
    } else {
      yaml += `${key}: ${value}\n`;
    }
  }
  return yaml;
}
