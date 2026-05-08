import { type FormEvent, useState } from "react";
import { Button } from "@/components/ui/button";
import { inferBrowserIdentity } from "../../lib/browser-identity";
import {
  clearSessionToken,
  createBrowserWorkspace,
  forgetBrowserWorkspace,
  loadSessionToken,
  loadBrowserWorkspaces,
  saveSessionToken,
  saveBrowserWorkspaces,
  updateBrowserWorkspace,
  type BrowserWorkspaceRecord,
} from "../../lib/browser-workspaces";

interface BrowserWorkspaceFormProps {
  initial?: BrowserWorkspaceRecord;
  submitLabel?: string;
  onConnected: (record: BrowserWorkspaceRecord, token: string) => Promise<boolean> | boolean;
  onCancel?: () => void;
}

const DEFAULT_CORS_PROXY = "https://cors.isomorphic-git.org";

export function BrowserWorkspaceForm({
  initial,
  submitLabel = initial ? "Reconnect" : "Connect",
  onConnected,
  onCancel,
}: BrowserWorkspaceFormProps) {
  const [workspaceName, setWorkspaceName] = useState(initial?.workspace_name ?? "");
  const [remoteUrl, setRemoteUrl] = useState(initial?.remoteUrl ?? "");
  const [token, setToken] = useState("");
  const [corsProxy, setCorsProxy] = useState(initial?.corsProxy ?? DEFAULT_CORS_PROXY);
  const [inferredHandler, setInferredHandler] = useState(initial?.handler ?? null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (!remoteUrl.trim() || !token.trim()) return;

    setLoading(true);
    setError(null);
    let savedRecord: BrowserWorkspaceRecord | undefined;
    const previousSessionToken = initial ? loadSessionToken(initial.id) : undefined;

    try {
      const identity = await inferBrowserIdentity({
        remoteUrl: remoteUrl.trim(),
        token: token.trim(),
      });
      setInferredHandler(identity.handler);

      const input = {
        remoteUrl: remoteUrl.trim(),
        corsProxy: corsProxy.trim() || undefined,
        handler: identity.handler,
        workspaceName: workspaceName.trim() || identity.displayName || identity.handler,
      };
      const record = initial
        ? updateBrowserWorkspace(initial.id, input)
        : createBrowserWorkspace(input);

      if (!record) {
        throw new Error("Browser workspace was not found");
      }
      savedRecord = record;

      saveSessionToken(record.id, token.trim());
      const connected = await onConnected(record, token.trim());
      if (!connected) {
        rollbackWorkspace(record, initial, previousSessionToken);
      }
    } catch (err) {
      if (savedRecord) {
        rollbackWorkspace(savedRecord, initial, previousSessionToken);
      }
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      <div className="space-y-2">
        <label htmlFor="browser-workspace-name" className="text-sm font-medium text-text-secondary">
          Workspace name
        </label>
        <input
          id="browser-workspace-name"
          type="text"
          value={workspaceName}
          onChange={(event) => setWorkspaceName(event.target.value)}
          placeholder="Team workspace"
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
        />
      </div>

      <div className="space-y-2">
        <label htmlFor="browser-remote-url" className="text-sm font-medium text-text-secondary">
          Git remote URL
        </label>
        <input
          id="browser-remote-url"
          type="url"
          value={remoteUrl}
          onChange={(event) => setRemoteUrl(event.target.value)}
          placeholder="https://github.com/team/im-repo"
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
          required
        />
      </div>

      <div className="space-y-2">
        <label htmlFor="browser-token" className="text-sm font-medium text-text-secondary">
          Personal access token
        </label>
        <input
          id="browser-token"
          type="password"
          value={token}
          onChange={(event) => setToken(event.target.value)}
          placeholder="github_pat_..."
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
          required
        />
        <p className="text-xs leading-relaxed text-text-muted">
          The token is kept in this tab session so refresh can reconnect. Closing
          the tab clears it.
        </p>
      </div>

      <div className="space-y-2">
        <label htmlFor="browser-cors-proxy" className="text-sm font-medium text-text-secondary">
          CORS proxy
        </label>
        <input
          id="browser-cors-proxy"
          type="url"
          value={corsProxy}
          onChange={(event) => setCorsProxy(event.target.value)}
          placeholder={DEFAULT_CORS_PROXY}
          className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
        />
        <p className="text-xs leading-relaxed text-text-muted">
          The proxy can see git traffic and authorization headers. Use a trusted
          proxy for private repositories.
        </p>
      </div>

      {inferredHandler && !loading && (
        <p className="text-sm text-text-muted">Signed in as @{inferredHandler}</p>
      )}

      {error && (
        <div className="p-3 rounded-lg bg-destructive/10 border border-destructive/20">
          <p className="text-xs text-destructive">{error}</p>
        </div>
      )}

      <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
        {onCancel && (
          <Button
            type="button"
            variant="outline"
            onClick={onCancel}
            disabled={loading}
            className="w-full sm:w-auto"
          >
            Cancel
          </Button>
        )}
        <Button
          type="submit"
          disabled={loading || !remoteUrl.trim() || !token.trim()}
          className="w-full sm:w-auto"
        >
          {loading ? "Connecting..." : submitLabel}
        </Button>
      </div>
    </form>
  );
}

function rollbackWorkspace(
  record: BrowserWorkspaceRecord,
  previous?: BrowserWorkspaceRecord,
  previousSessionToken?: string,
): void {
  if (!previous) {
    forgetBrowserWorkspace(record.id);
    return;
  }

  restoreBrowserWorkspace(previous);
  if (previousSessionToken !== undefined) {
    saveSessionToken(record.id, previousSessionToken);
  } else {
    clearSessionToken(record.id);
  }
}

function restoreBrowserWorkspace(record: BrowserWorkspaceRecord): void {
  saveBrowserWorkspaces(
    loadBrowserWorkspaces().map((workspace) =>
      workspace.id === record.id ? record : workspace,
    ),
  );
}
