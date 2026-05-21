export type UsageBreakdown = "provider" | "handler";

export interface UnreadChannelState {
  unreadCount: number;
  hasMention: boolean;
}

export interface UiState {
  channel: string | null;
  boardHandler: string | null;
  cardsShowArchived: boolean;
  /** Drives the grouping dimension of `WorkspaceUsageHeader`'s breakdown
   *  row. Persisted per workspace so a 30-agent workspace and a 3-agent
   *  workspace can each settle on their own preferred view. */
  usageBreakdown: UsageBreakdown;
  unreadByChannel: Record<string, UnreadChannelState>;
  messageScrollByScope: Record<string, number>;
}

export const DEFAULT_UI_STATE: UiState = {
  channel: null,
  boardHandler: null,
  cardsShowArchived: false,
  usageBreakdown: "provider",
  unreadByChannel: {},
  messageScrollByScope: {},
};

const UI_STATE_STORAGE_PREFIX = "gitim-ui-state:";

function uiStateStorageKey(workspaceKey: string): string {
  return `${UI_STATE_STORAGE_PREFIX}${workspaceKey}`;
}

function defaultUiState(): UiState {
  return {
    ...DEFAULT_UI_STATE,
    unreadByChannel: {},
    messageScrollByScope: {},
  };
}

function readUnreadByChannel(raw: unknown): Record<string, UnreadChannelState> {
  if (!raw || typeof raw !== "object") return {};
  const result: Record<string, UnreadChannelState> = {};
  for (const [channel, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!channel || !value || typeof value !== "object") continue;
    const entry = value as Record<string, unknown>;
    const unreadCount = entry.unreadCount;
    if (
      typeof unreadCount !== "number" ||
      !Number.isFinite(unreadCount) ||
      unreadCount <= 0
    ) {
      continue;
    }
    result[channel] = {
      unreadCount,
      hasMention: entry.hasMention === true,
    };
  }
  return result;
}

function readMessageScrollByScope(raw: unknown): Record<string, number> {
  if (!raw || typeof raw !== "object") return {};
  const result: Record<string, number> = {};
  for (const [scopeKey, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!scopeKey || typeof value !== "number" || !Number.isFinite(value)) continue;
    result[scopeKey] = Math.max(0, value);
  }
  return result;
}

export function readUiState(workspaceKey: string | null): UiState {
  if (!workspaceKey) return defaultUiState();
  try {
    const raw = localStorage.getItem(uiStateStorageKey(workspaceKey));
    if (!raw) return defaultUiState();
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return defaultUiState();
    const obj = parsed as Record<string, unknown>;
    return {
      channel: typeof obj.channel === "string" ? obj.channel : DEFAULT_UI_STATE.channel,
      boardHandler:
        typeof obj.boardHandler === "string" ? obj.boardHandler : DEFAULT_UI_STATE.boardHandler,
      cardsShowArchived:
        typeof obj.cardsShowArchived === "boolean"
          ? obj.cardsShowArchived
          : DEFAULT_UI_STATE.cardsShowArchived,
      usageBreakdown:
        obj.usageBreakdown === "provider" || obj.usageBreakdown === "handler"
          ? obj.usageBreakdown
          : DEFAULT_UI_STATE.usageBreakdown,
      unreadByChannel: readUnreadByChannel(obj.unreadByChannel),
      messageScrollByScope: readMessageScrollByScope(obj.messageScrollByScope),
    };
  } catch {
    return defaultUiState();
  }
}

export function writeUiState(workspaceKey: string, patch: Partial<UiState>): void {
  const current = readUiState(workspaceKey);
  const next: UiState = { ...current, ...patch };
  localStorage.setItem(uiStateStorageKey(workspaceKey), JSON.stringify(next));
}

export function clearUiState(workspaceKey: string): void {
  localStorage.removeItem(uiStateStorageKey(workspaceKey));
}

export function mergeUnreadIntoChannels<
  T extends {
    name: string;
    unreadCount?: number;
    hasMention?: boolean;
  },
>(workspaceKey: string | null, channels: T[]): T[] {
  const unreadByChannel = readUiState(workspaceKey).unreadByChannel;
  return channels.map((channel) => {
    const unread = unreadByChannel[channel.name];
    if (!unread) return channel;
    return {
      ...channel,
      unreadCount: unread.unreadCount,
      hasMention: unread.hasMention,
    };
  });
}

export function incrementStoredUnread(
  workspaceKey: string | null,
  channel: string,
  mentioned: boolean,
): void {
  if (!workspaceKey) return;
  const state = readUiState(workspaceKey);
  const current = state.unreadByChannel[channel];
  writeUiState(workspaceKey, {
    unreadByChannel: {
      ...state.unreadByChannel,
      [channel]: {
        unreadCount: (current?.unreadCount ?? 0) + 1,
        hasMention: (current?.hasMention ?? false) || mentioned,
      },
    },
  });
}

export function clearStoredUnread(
  workspaceKey: string | null,
  channel: string,
): void {
  if (!workspaceKey) return;
  const state = readUiState(workspaceKey);
  if (!state.unreadByChannel[channel]) return;
  const next = { ...state.unreadByChannel };
  delete next[channel];
  writeUiState(workspaceKey, { unreadByChannel: next });
}

export function readMessageScrollTop(
  workspaceKey: string | null,
  scopeKey: string | null,
): number | null {
  if (!workspaceKey || !scopeKey) return null;
  const value = readUiState(workspaceKey).messageScrollByScope[scopeKey];
  return typeof value === "number" ? value : null;
}

export function writeMessageScrollTop(
  workspaceKey: string | null,
  scopeKey: string | null,
  scrollTop: number,
): void {
  if (!workspaceKey || !scopeKey || !Number.isFinite(scrollTop)) return;
  const state = readUiState(workspaceKey);
  writeUiState(workspaceKey, {
    messageScrollByScope: {
      ...state.messageScrollByScope,
      [scopeKey]: Math.max(0, scrollTop),
    },
  });
}
