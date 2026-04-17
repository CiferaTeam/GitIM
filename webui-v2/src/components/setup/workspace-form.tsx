import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { SetupShell } from "./setup-shell";

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
    <SetupShell
      step={2}
      title="Workspace"
      description="Choose a directory for this session"
      error={error}
      onBack={() => setStatus("disconnected")}
      footer={
        runtimeVersion ? (
          <span className="inline-flex items-center gap-1.5 px-2 py-1 rounded-full bg-surface border border-border text-text-secondary">
            Runtime v{runtimeVersion}
          </span>
        ) : undefined
      }
    >
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="space-y-2">
          <label
            htmlFor="workspace-input"
            className="text-sm font-medium text-text-secondary"
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
            className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm font-mono placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
            autoFocus
          />
        </div>

        {needsConfirm ? (
          <div className="flex gap-3">
            <button
              data-testid="workspace-confirm-button"
              type="button"
              disabled={submitting}
              onClick={() => submitWorkspace(input.trim(), true)}
              className="flex-1 h-10 rounded-lg bg-primary text-primary-foreground text-sm font-semibold hover:bg-primary/90 disabled:opacity-50 transition-colors shadow-lg shadow-primary/20"
            >
              Confirm
            </button>
            <button
              type="button"
              onClick={() => { setNeedsConfirm(false); setError(null); }}
              className="flex-1 h-10 rounded-lg border border-border bg-card text-sm font-semibold hover:bg-surface-hover transition-colors"
            >
              Cancel
            </button>
          </div>
        ) : (
          <button
            data-testid="workspace-button"
            type="submit"
            disabled={submitting || !input.trim()}
            className="w-full h-10 rounded-lg bg-primary text-primary-foreground text-sm font-semibold hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-lg shadow-primary/20"
          >
            {submitting ? "Setting up..." : "Open Workspace"}
          </button>
        )}
      </form>
    </SetupShell>
  );
}
