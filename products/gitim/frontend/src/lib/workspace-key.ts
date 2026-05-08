import type { ConnectionMode } from "@/hooks/use-connection-store";
import type { WorkspaceSummary } from "./types";

const RUNTIME_ACTIVE_KEY = "gitim-active-workspace";
const BROWSER_ACTIVE_KEY = "gitim-active-browser-workspace";

function cleanKeyPart(value: string): string {
  return value.replace(/\//g, "-");
}

export function activeWorkspaceStorageKey(mode: ConnectionMode): string {
  return mode === "local" ? BROWSER_ACTIVE_KEY : RUNTIME_ACTIVE_KEY;
}

export function workspaceIdentity(
  mode: ConnectionMode,
  workspace: Pick<WorkspaceSummary, "slug" | "id">,
): string {
  if (mode === "local") {
    return `browser:${workspace.id ?? workspace.slug}`;
  }
  return `runtime:${workspace.slug}`;
}

export function cursorWorkspaceKey(
  mode: ConnectionMode,
  workspace: Pick<WorkspaceSummary, "slug" | "id">,
): string {
  return "gitim:cursor:" + cleanKeyPart(workspaceIdentity(mode, workspace));
}
