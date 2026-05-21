export interface ChatScopeUiState {
  scrollTop: number | null;
  unreadCount: number;
  hasMention: boolean;
  firstUnreadLine: number | null;
  updatedAt: number;
}

const CHAT_UI_STORAGE_PREFIX = "gitim:ui:v2:";

function workspacePrefix(workspaceKey: string): string {
  return `${CHAT_UI_STORAGE_PREFIX}${encodeURIComponent(workspaceKey)}:`;
}

function activeScopeStorageKey(workspaceKey: string): string {
  return `${workspacePrefix(workspaceKey)}activeScope`;
}

function scopeStorageKey(workspaceKey: string, scopeKey: string): string {
  return `${workspacePrefix(workspaceKey)}scope:${encodeURIComponent(scopeKey)}`;
}

function nowMs(): number {
  return Date.now();
}

function emptyScopeState(): ChatScopeUiState {
  return {
    scrollTop: null,
    unreadCount: 0,
    hasMention: false,
    firstUnreadLine: null,
    updatedAt: 0,
  };
}

function readStoredScopeState(
  workspaceKey: string,
  scopeKey: string,
): ChatScopeUiState | null {
  try {
    const raw = localStorage.getItem(scopeStorageKey(workspaceKey, scopeKey));
    if (!raw) return null;
    return parseScopeState(JSON.parse(raw));
  } catch {
    return emptyScopeState();
  }
}

function parseScopeState(raw: unknown): ChatScopeUiState {
  if (!raw || typeof raw !== "object") return emptyScopeState();
  const obj = raw as Record<string, unknown>;
  const scrollTop =
    typeof obj.scrollTop === "number" && Number.isFinite(obj.scrollTop)
      ? Math.max(0, obj.scrollTop)
      : null;
  const unreadCount =
    typeof obj.unreadCount === "number" && Number.isFinite(obj.unreadCount)
      ? Math.max(0, Math.floor(obj.unreadCount))
      : 0;
  const firstUnreadLine =
    unreadCount > 0 &&
    typeof obj.firstUnreadLine === "number" &&
    Number.isFinite(obj.firstUnreadLine) &&
    obj.firstUnreadLine > 0
      ? Math.floor(obj.firstUnreadLine)
      : null;
  const updatedAt =
    typeof obj.updatedAt === "number" && Number.isFinite(obj.updatedAt)
      ? Math.max(0, obj.updatedAt)
      : 0;
  return {
    scrollTop,
    unreadCount,
    hasMention: unreadCount > 0 && obj.hasMention === true,
    firstUnreadLine,
    updatedAt,
  };
}

function writeScopeState(
  workspaceKey: string,
  scopeKey: string,
  state: ChatScopeUiState,
): void {
  localStorage.setItem(
    scopeStorageKey(workspaceKey, scopeKey),
    JSON.stringify(parseScopeState(state)),
  );
}

export function chatScopeKeyForName(
  name: string,
  kind?: "channel" | "dm",
): string {
  const prefix = kind ?? (name.includes("--") ? "dm" : "channel");
  return `${prefix}:${name}`;
}

export function chatScopeKeyForChannel(channel: {
  name: string;
  kind?: "channel" | "dm";
}): string {
  return chatScopeKeyForName(channel.name, channel.kind);
}

export function chatScopeName(scopeKey: string | null): string | null {
  if (!scopeKey) return null;
  if (scopeKey.startsWith("channel:")) return scopeKey.slice("channel:".length);
  if (scopeKey.startsWith("dm:")) return scopeKey.slice("dm:".length);
  return scopeKey || null;
}

export function readActiveChatScope(workspaceKey: string | null): string | null {
  if (!workspaceKey) return null;
  return localStorage.getItem(activeScopeStorageKey(workspaceKey));
}

export function writeActiveChatScope(
  workspaceKey: string | null,
  scopeKey: string | null,
): void {
  if (!workspaceKey) return;
  const key = activeScopeStorageKey(workspaceKey);
  if (!scopeKey) {
    localStorage.removeItem(key);
    return;
  }
  localStorage.setItem(key, scopeKey);
}

export function readChatScopeState(
  workspaceKey: string | null,
  scopeKey: string | null,
): ChatScopeUiState {
  if (!workspaceKey || !scopeKey) return emptyScopeState();
  return readStoredScopeState(workspaceKey, scopeKey) ?? emptyScopeState();
}

export function readChatScopeScrollTop(
  workspaceKey: string | null,
  scopeKey: string | null,
): number | null {
  return readChatScopeState(workspaceKey, scopeKey).scrollTop;
}

export function writeChatScopeScrollTop(
  workspaceKey: string | null,
  scopeKey: string | null,
  scrollTop: number,
): void {
  if (!workspaceKey || !scopeKey || !Number.isFinite(scrollTop)) return;
  const state = readChatScopeState(workspaceKey, scopeKey);
  writeScopeState(workspaceKey, scopeKey, {
    ...state,
    scrollTop: Math.max(0, scrollTop),
    updatedAt: nowMs(),
  });
}

export function incrementChatScopeUnread(
  workspaceKey: string | null,
  scopeKey: string | null,
  change: {
    count: number;
    hasMention: boolean;
    firstUnreadLine: number | null;
  },
): void {
  if (!workspaceKey || !scopeKey) return;
  if (!Number.isFinite(change.count) || change.count <= 0) return;
  const state = readChatScopeState(workspaceKey, scopeKey);
  const nextUnreadCount = state.unreadCount + Math.floor(change.count);
  writeScopeState(workspaceKey, scopeKey, {
    ...state,
    unreadCount: nextUnreadCount,
    hasMention: state.hasMention || change.hasMention,
    firstUnreadLine:
      state.firstUnreadLine ??
      (change.firstUnreadLine && change.firstUnreadLine > 0
        ? Math.floor(change.firstUnreadLine)
        : null),
    updatedAt: nowMs(),
  });
}

export function clearChatScopeUnread(
  workspaceKey: string | null,
  scopeKey: string | null,
): void {
  if (!workspaceKey || !scopeKey) return;
  const state = readChatScopeState(workspaceKey, scopeKey);
  if (state.unreadCount === 0 && !state.hasMention && state.firstUnreadLine === null) {
    return;
  }
  writeScopeState(workspaceKey, scopeKey, {
    ...state,
    unreadCount: 0,
    hasMention: false,
    firstUnreadLine: null,
    updatedAt: nowMs(),
  });
}

export function mergeChatUnreadIntoChannels<
  T extends {
    name: string;
    kind?: "channel" | "dm";
    unreadCount?: number;
    hasMention?: boolean;
  },
>(workspaceKey: string | null, channels: T[]): T[] {
  return channels.map((channel) => {
    const unread = readChatScopeState(
      workspaceKey,
      chatScopeKeyForChannel(channel),
    );
    if (unread.unreadCount <= 0) return channel;
    return {
      ...channel,
      unreadCount: unread.unreadCount,
      hasMention: unread.hasMention,
    };
  });
}

export function clearChatUiState(workspaceKey: string | null): void {
  if (!workspaceKey) return;
  const prefix = workspacePrefix(workspaceKey);
  const keys: string[] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const key = localStorage.key(i);
    if (key?.startsWith(prefix)) keys.push(key);
  }
  for (const key of keys) {
    localStorage.removeItem(key);
  }
}
