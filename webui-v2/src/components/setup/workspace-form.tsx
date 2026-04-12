import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

export function WorkspaceForm() {
  const baseUrl = useConnectionStore((s) => s.baseUrl);
  const runtimeVersion = useConnectionStore((s) => s.runtimeVersion);
  const setWorkspacePath = useConnectionStore((s) => s.setWorkspacePath);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);

  const [input, setInput] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [needsConfirm, setNeedsConfirm] = useState(false);

  async function submitWorkspace(path: string, confirm: boolean) {
    setSubmitting(true);
    setError(null);
    setNeedsConfirm(false);

    try {
      const res = await fetch(`${baseUrl()}/workspace`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path, confirm }),
      });
      const data = await res.json();

      if (!data.ok) {
        if (data.needs_confirm) {
          setNeedsConfirm(true);
          setError(data.error);
          return;
        }
        setError(data.error ?? "Failed to set workspace");
        return;
      }

      setWorkspacePath(path);
      setStatus("workspace_set");
    } catch {
      setError("Failed to connect to runtime");
    } finally {
      setSubmitting(false);
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const path = input.trim();
    if (!path) {
      setError("Please enter a workspace path");
      return;
    }
    await submitWorkspace(path, false);
  }

  return (
    <div className="flex flex-col items-center justify-center h-screen bg-background text-foreground">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-xl font-bold tracking-tight">GitIM</h1>
          <p className="text-sm text-muted-foreground">
            Set a workspace directory for this session
          </p>
          {runtimeVersion && (
            <p className="text-xs text-text-muted">
              Runtime v{runtimeVersion}
            </p>
          )}
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <label
              htmlFor="workspace-input"
              className="text-xs font-medium text-text-secondary"
            >
              Workspace Path
            </label>
            <input
              id="workspace-input"
              data-testid="workspace-input"
              type="text"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="/path/to/workspace"
              className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm font-mono placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
              autoFocus
            />
          </div>

          {error && (
            <p data-testid="workspace-error" className="text-xs text-error">
              {error}
            </p>
          )}

          {needsConfirm ? (
            <div className="flex gap-2">
              <button
                data-testid="workspace-confirm-button"
                type="button"
                disabled={submitting}
                onClick={() => submitWorkspace(input.trim(), true)}
                className="flex-1 h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
              >
                Confirm
              </button>
              <button
                type="button"
                onClick={() => { setNeedsConfirm(false); setError(null); }}
                className="flex-1 h-9 rounded-md border border-input text-sm font-medium hover:bg-muted transition-colors"
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              data-testid="workspace-button"
              type="submit"
              disabled={submitting}
              className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
            >
              {submitting ? "Setting up..." : "Open Workspace"}
            </button>
          )}
        </form>
      </div>
    </div>
  );
}
