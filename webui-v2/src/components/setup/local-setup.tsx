// Local mode setup — clone form for mobile.
// User provides: git remote URL, personal access token, handler (username).

import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { LocalBackend } from "../../lib/backend";
import { setBackend } from "../../lib/client";

const LOCAL_CONFIG_KEY = "gitim-local-config";

interface LocalConfig {
  remoteUrl: string;
  corsProxy: string;
  token: string;
  handler: string;
}

function loadSavedConfig(): Partial<LocalConfig> {
  try {
    const raw = localStorage.getItem(LOCAL_CONFIG_KEY);
    return raw ? (JSON.parse(raw) as Partial<LocalConfig>) : {};
  } catch {
    return {};
  }
}

function saveConfig(config: LocalConfig): void {
  // Save URL and handler, but NOT token (security)
  localStorage.setItem(
    LOCAL_CONFIG_KEY,
    JSON.stringify({
      remoteUrl: config.remoteUrl,
      corsProxy: config.corsProxy,
      handler: config.handler,
    }),
  );
}

export function LocalSetup() {
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setLocalReady = useConnectionStore((s) => s.setLocalReady);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);
  const cloneProgress = useConnectionStore((s) => s.cloneProgress);
  const setCloneProgress = useConnectionStore((s) => s.setCloneProgress);
  const setMode = useConnectionStore((s) => s.setMode);

  const saved = loadSavedConfig();
  const [remoteUrl, setRemoteUrl] = useState(saved.remoteUrl ?? "");
  const [corsProxy, setCorsProxy] = useState(
    saved.corsProxy ?? "https://cors.isomorphic-git.org",
  );
  const [token, setToken] = useState("");
  const [handler, setHandler] = useState(saved.handler ?? "");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!remoteUrl || !token || !handler) return;

    setLoading(true);
    setError(null);
    setCloneProgress("Connecting...");

    try {
      const backend = new LocalBackend(() => {
        // On sync reset, the app.tsx poll loop will handle reload
        setCloneProgress(null);
      });

      setCloneProgress("Cloning repository...");

      const result = await backend.init({
        remoteUrl,
        corsProxy,
        token,
        handler: handler.toLowerCase(),
      });

      if (!result.ok) {
        setError(result.error ?? "Init failed");
        setLoading(false);
        setCloneProgress(null);
        return;
      }

      // Start sync loop
      await backend.startSync();

      // Switch the global backend
      setBackend(backend);
      saveConfig({ remoteUrl, corsProxy, token, handler });

      setCloneProgress(null);
      setLocalReady(true);
      setStatus("ready");
    } catch (err) {
      setError(String((err as Error).message ?? err));
      setCloneProgress(null);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="flex items-center justify-center min-h-screen bg-background p-4">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-md space-y-4 rounded-lg border border-border bg-card p-6 shadow-sm"
      >
        <div className="text-center">
          <h1 className="text-lg font-semibold text-foreground">
            GitIM Local Mode
          </h1>
          <p className="text-sm text-muted-foreground mt-1">
            Connect directly to a git remote — no server needed.
          </p>
        </div>

        <div className="space-y-2">
          <label className="block text-sm font-medium text-foreground">
            Git Remote URL
          </label>
          <input
            type="url"
            value={remoteUrl}
            onChange={(e) => setRemoteUrl(e.target.value)}
            placeholder="https://github.com/team/im-repo"
            className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
            required
          />
        </div>

        <div className="space-y-2">
          <label className="block text-sm font-medium text-foreground">
            Personal Access Token
          </label>
          <input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="ghp_..."
            className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
            required
          />
          <p className="text-xs text-muted-foreground">
            Token is only stored in memory — never persisted.
          </p>
        </div>

        <div className="space-y-2">
          <label className="block text-sm font-medium text-foreground">
            Your Handler
          </label>
          <input
            type="text"
            value={handler}
            onChange={(e) => setHandler(e.target.value)}
            placeholder="your-github-handle"
            className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
            required
          />
        </div>

        <div className="space-y-2">
          <label className="block text-sm font-medium text-foreground">
            CORS Proxy
          </label>
          <input
            type="url"
            value={corsProxy}
            onChange={(e) => setCorsProxy(e.target.value)}
            placeholder="https://cors.isomorphic-git.org"
            className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
          />
          <p className="text-xs text-muted-foreground">
            Required for browser git access. Use your own proxy in production.
          </p>
        </div>

        {error && (
          <p className="text-sm text-destructive">{error}</p>
        )}

        {cloneProgress && (
          <p className="text-sm text-muted-foreground animate-pulse">
            {cloneProgress}
          </p>
        )}

        <button
          type="submit"
          disabled={loading || !remoteUrl || !token || !handler}
          className="w-full rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {loading ? "Connecting..." : "Connect"}
        </button>

        <button
          type="button"
          onClick={() => setMode("remote")}
          className="w-full text-center text-sm text-muted-foreground hover:text-foreground"
        >
          Switch to Remote Mode
        </button>
      </form>
    </div>
  );
}
