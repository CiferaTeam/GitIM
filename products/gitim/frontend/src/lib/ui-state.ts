export type UsageBreakdown = "provider" | "handler";

export interface UiState {
  boardHandler: string | null;
  cardsShowArchived: boolean;
  /** Drives the grouping dimension of `WorkspaceUsageHeader`'s breakdown
   *  row. Persisted per workspace so a 30-agent workspace and a 3-agent
   *  workspace can each settle on their own preferred view. */
  usageBreakdown: UsageBreakdown;
}

export const DEFAULT_UI_STATE: UiState = {
  boardHandler: null,
  cardsShowArchived: false,
  usageBreakdown: "provider",
};

const UI_STATE_STORAGE_PREFIX = "gitim-ui-state:";

function uiStateStorageKey(workspaceKey: string): string {
  return `${UI_STATE_STORAGE_PREFIX}${workspaceKey}`;
}

function defaultUiState(): UiState {
  return { ...DEFAULT_UI_STATE };
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
