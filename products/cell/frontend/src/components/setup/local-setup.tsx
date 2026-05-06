import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { LocalBackend } from "../../lib/backend";
import { setBackend } from "../../lib/client";
import { SetupShell } from "./setup-shell";

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
    if (!remoteUrl.trim() || !token.trim() || !handler.trim()) return;

    setLoading(true);
    setError(null);
    setCloneProgress("Connecting...");

    try {
      const backend = new LocalBackend(() => setCloneProgress(null));
      const normalizedHandler = handler.trim().toLowerCase();

      setCloneProgress("Cloning repository...");
      const result = await backend.init({
        remoteUrl: remoteUrl.trim(),
        corsProxy: corsProxy.trim(),
        token,
        handler: normalizedHandler,
      });

      if (!result.ok) {
        setError(result.error ?? "Init failed");
        return;
      }

      await backend.startSync();
      setBackend(backend);
      saveConfig({
        remoteUrl: remoteUrl.trim(),
        corsProxy: corsProxy.trim(),
        token,
        handler: normalizedHandler,
      });
      setLocalReady(true);
      setStatus("ready");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCloneProgress(null);
      setLoading(false);
    }
  }

  return (
    <SetupShell
      step={2}
      title="Browser Mode"
      description="Clone a GitIM repository directly in this browser"
      error={error}
      loading={loading}
      footer={
        <button
          type="button"
          onClick={() => setMode("remote")}
          className="text-text-muted hover:text-foreground transition-colors"
        >
          Use desktop runtime instead
        </button>
      }
    >
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="space-y-2">
          <label htmlFor="local-remote-url" className="text-sm font-medium text-text-secondary">
            Git remote URL
          </label>
          <input
            id="local-remote-url"
            type="url"
            value={remoteUrl}
            onChange={(e) => setRemoteUrl(e.target.value)}
            placeholder="https://github.com/team/im-repo"
            className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
            required
          />
        </div>

        <div className="space-y-2">
          <label htmlFor="local-token" className="text-sm font-medium text-text-secondary">
            Personal access token
          </label>
          <input
            id="local-token"
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="github_pat_..."
            className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
            required
          />
          <p className="text-xs text-text-muted">
            The token stays in memory for this tab.
          </p>
        </div>

        <div className="space-y-2">
          <label htmlFor="local-handler" className="text-sm font-medium text-text-secondary">
            Handler
          </label>
          <input
            id="local-handler"
            type="text"
            value={handler}
            onChange={(e) => setHandler(e.target.value)}
            placeholder="your-github-handle"
            className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
            required
          />
        </div>

        <div className="space-y-2">
          <label htmlFor="local-cors-proxy" className="text-sm font-medium text-text-secondary">
            CORS proxy
          </label>
          <input
            id="local-cors-proxy"
            type="url"
            value={corsProxy}
            onChange={(e) => setCorsProxy(e.target.value)}
            placeholder="https://cors.isomorphic-git.org"
            className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
          />
        </div>

        {cloneProgress && (
          <p className="text-sm text-text-muted animate-pulse">{cloneProgress}</p>
        )}

        <button
          type="submit"
          disabled={loading || !remoteUrl.trim() || !token.trim() || !handler.trim()}
          className="w-full h-10 rounded-lg bg-primary text-primary-foreground text-sm font-semibold hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-lg shadow-primary/20"
        >
          {loading ? "Connecting..." : "Connect"}
        </button>
      </form>
    </SetupShell>
  );
}
