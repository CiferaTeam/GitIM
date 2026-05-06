// API handlers for daemon-web — implements the Backend interface methods.
// Each function mirrors what gitim-runtime returns over HTTP.

import * as gitOps from "./git";
import { readFile, writeFile, readdir, exists, mkdir } from "./storage";
import { getState, setState, type ChannelMeta, type UserMeta } from "./state";
import { parseThread, type ThreadEntry } from "./parser";
import { formatMessage, formatEvent } from "./formatter";
import { runSync } from "./sync";
import {
  channelMetaPath,
  channelNameFromMetaFile,
  dmApiNameFromThreadPath,
  resolveThreadTarget,
  validateChannelName,
} from "./paths";

type ApiResponse = {
  ok: boolean;
  data?: Record<string, unknown>;
  error?: string;
};

function ok(data: Record<string, unknown> = {}): ApiResponse {
  return { ok: true, data };
}

function err(error: string): ApiResponse {
  return { ok: false, error };
}

// --- Init ---

export async function init(config: {
  remoteUrl: string;
  corsProxy: string;
  token: string;
  handler: string;
}): Promise<ApiResponse> {
  const { initState } = await import("./state");
  const dir = "/repo";
  const onAuth = () => ({
    username: config.token,
    password: "x-oauth-basic",
  });

  try {
    // Clone the repo
    const repoExists = await exists(`${dir}/.git`);
    if (!repoExists) {
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
      repoDir: dir,
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

    return ok({ handler: config.handler, display_name: displayName });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

// --- IM handlers ---

export async function health(): Promise<ApiResponse> {
  try {
    getState();
    return ok({
      service: "daemon-web",
      initialized: true,
      workspace: "local",
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
  const onAuth = () => ({ username: s.token, password: "x-oauth-basic" });

  try {
    // Fetch from remote
    await gitOps.fetchOrigin(s.repoDir, s.corsProxy, onAuth);
    const remoteHead = await gitOps.resolveRemoteHead(s.repoDir);
    const localHead = await gitOps.resolveHead(s.repoDir);

    // If remote has new commits, fast-forward (sync handles conflicts separately)
    if (remoteHead !== localHead && localHead === s.headCommit) {
      await gitOps.checkout(s.repoDir, remoteHead);
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
      if (fp.startsWith("channels/") && fp.endsWith(".thread")) {
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
    // For DMs, only show if current user is a participant
    if (isDm) {
      const parts = name.split("--");
      if (!parts.includes(s.me.handler)) continue;
    }
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
    const entries = await readChannelEntries(channel);
    const limited = limit ? entries.slice(-limit) : entries;
    return ok({ channel, entries: limited, archived: false });
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

    // Trigger sync
    runSync().catch(console.error);

    return ok({ line_number: nextLine });
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
    runSync().catch(console.error);

    return ok();
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}

// --- Internal helpers ---

async function readChannelEntries(
  channel: string,
): Promise<ThreadEntry[]> {
  const s = getState();
  const target = resolveThreadTarget(channel);
  const absPath = `${s.repoDir}/${target.threadPath}`;

  if (!(await exists(absPath))) return [];

  const content = await readFile(absPath);
  const file = parseThread(content);
  return file.entries;
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
