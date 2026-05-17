export type UsageBreakdown = "provider" | "handler";

export interface UiState {
  channel: string | null;
  boardHandler: string | null;
  cardsShowArchived: boolean;
  /** Drives the grouping dimension of `WorkspaceUsageHeader`'s breakdown
   *  row. Persisted per workspace so a 30-agent workspace and a 3-agent
   *  workspace can each settle on their own preferred view. */
  usageBreakdown: UsageBreakdown;
}

export const DEFAULT_UI_STATE: UiState = {
  channel: null,
  boardHandler: null,
  cardsShowArchived: false,
  usageBreakdown: "provider",
};

const UI_STATE_STORAGE_PREFIX = "gitim-ui-state:";

function uiStateStorageKey(workspaceKey: string): string {
  return `${UI_STATE_STORAGE_PREFIX}${workspaceKey}`;
}

export function readUiState(workspaceKey: string | null): UiState {
  if (!workspaceKey) return { ...DEFAULT_UI_STATE };
  try {
    const raw = localStorage.getItem(uiStateStorageKey(workspaceKey));
    if (!raw) return { ...DEFAULT_UI_STATE };
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return { ...DEFAULT_UI_STATE };
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
    };
  } catch {
    return { ...DEFAULT_UI_STATE };
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
