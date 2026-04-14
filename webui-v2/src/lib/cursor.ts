function workspaceToKey(workspace: string): string {
  return "gitim:cursor:" + workspace.replace(/\//g, "-");
}

export function loadCursor(workspace: string): string | undefined {
  const key = workspaceToKey(workspace);
  return localStorage.getItem(key) ?? undefined;
}

export function saveCursor(workspace: string, commitId: string): void {
  const key = workspaceToKey(workspace);
  localStorage.setItem(key, commitId);
}

export function clearCursor(workspace: string): void {
  const key = workspaceToKey(workspace);
  localStorage.removeItem(key);
}
