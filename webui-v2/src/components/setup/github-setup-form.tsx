import { useEffect, useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

const ROTATING_MESSAGES = [
  "Verifying token…",
  "Checking repo access…",
  "Cloning repo…",
  "Initializing workspace…",
] as const;

const PAT_GENERATE_URL =
  "https://github.com/settings/personal-access-tokens/new?name=GitIM%20runtime";

// Keep in sync with runtime git_init error_code enum.
const ERROR_MESSAGES: Record<string, string> = {
  missing_token: "Please fill both fields.",
  missing_remote_url: "Please fill both fields.",
  invalid_token: "Token was rejected. Make sure the PAT is valid and not expired.",
  insufficient_scope:
    "Token is missing required scopes. Fine-grained PAT needs Contents R/W + Metadata R on this repo. Classic PAT needs 'repo'.",
  token_lacks_repo_access:
    "Token is valid but has no access to this repository. Grant it access in PAT settings, or check the URL.",
  network_error: "Cannot reach GitHub. Check your internet connection.",
  rate_limited: "GitHub rate limit reached. Wait a few minutes and try again.",
  clone_failed: "Failed to clone the repository. See runtime logs for details.",
  cloud_sync_path_rejected:
    "Workspace is inside a cloud-sync folder (iCloud/Dropbox/Google Drive/OneDrive). Move it elsewhere to keep your PAT local.",
  provider_not_supported:
    "GitHub mode is not available in this runtime. (On Windows, this is not yet supported.)",
};

export function GithubSetupForm() {
  const baseUrl = useConnectionStore((s) => s.baseUrl);
  const workspacePath = useConnectionStore((s) => s.workspacePath);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);

  const [remoteUrl, setRemoteUrl] = useState("");
  const [token, setToken] = useState("");
  const [acknowledged, setAcknowledged] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [rotatingIdx, setRotatingIdx] = useState(0);

  useEffect(() => {
    if (!submitting) {
      setRotatingIdx(0);
      return;
    }
    const timer = setInterval(() => {
      setRotatingIdx((i) => Math.min(i + 1, ROTATING_MESSAGES.length - 1));
    }, 1500);
    return () => clearInterval(timer);
  }, [submitting]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();

    const trimmedUrl = remoteUrl.trim();
    const trimmedToken = token.trim();

    if (!trimmedUrl || !trimmedToken) {
      setError("Please fill both fields.");
      return;
    }
    if (!trimmedUrl.startsWith("https://github.com/")) {
      setError("Remote URL must start with https://github.com/");
      return;
    }

    setSubmitting(true);
    setError(null);

    try {
      const res = await fetch(`${baseUrl()}/git/init`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          provider: "github",
          remote_url: trimmedUrl,
          token: trimmedToken,
        }),
      });
      const data = await res.json();

      if (!data.ok) {
        const code = data.error_code as string | undefined;
        const mapped = code ? ERROR_MESSAGES[code] : undefined;
        setError(mapped ?? data.error ?? "Failed to initialize GitHub workspace");
        return;
      }

      setStatus("ready");
    } catch {
      setError("Failed to connect to runtime");
    } finally {
      setSubmitting(false);
    }
  }

  function handleBack() {
    setError(null);
    setStatus("workspace_set");
  }

  return (
    <div className="flex flex-col items-center justify-center h-screen bg-background text-foreground">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-xl font-bold tracking-tight">GitIM</h1>
          <p className="text-sm text-muted-foreground">
            Connect a GitHub repository
          </p>
          {workspacePath && (
            <p className="text-xs text-text-muted font-mono truncate">
              {workspacePath}
            </p>
          )}
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <label
              htmlFor="github-remote-url"
              className="text-xs font-medium text-text-secondary"
            >
              Remote URL
            </label>
            <input
              id="github-remote-url"
              data-testid="github-remote-url"
              type="text"
              value={remoteUrl}
              onChange={(e) => setRemoteUrl(e.target.value)}
              placeholder="https://github.com/org/repo"
              disabled={submitting}
              className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm font-mono placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
              autoFocus
            />
          </div>

          <div className="space-y-2">
            <label
              htmlFor="github-token"
              className="text-xs font-medium text-text-secondary"
            >
              Personal Access Token
            </label>
            <input
              id="github-token"
              data-testid="github-token"
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="ghp_... or github_pat_..."
              disabled={submitting}
              autoComplete="off"
              className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm font-mono placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring disabled:opacity-50"
            />
          </div>

          <label className="flex items-start gap-2 text-xs text-text-secondary leading-relaxed cursor-pointer">
            <input
              data-testid="github-ack"
              type="checkbox"
              checked={acknowledged}
              onChange={(e) => setAcknowledged(e.target.checked)}
              disabled={submitting}
              className="mt-0.5 accent-primary cursor-pointer"
            />
            <span>
              I understand all agents will commit as this PAT owner on GitHub
              (agent authorship preserved in commit author field but GitHub
              contribution graph attributes to PAT owner)
            </span>
          </label>

          {error && (
            <p data-testid="github-setup-error" className="text-xs text-error">
              {error}
            </p>
          )}

          <div className="space-y-2">
            <button
              data-testid="github-connect-button"
              type="submit"
              disabled={submitting || !acknowledged}
              className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
            >
              {submitting ? ROTATING_MESSAGES[rotatingIdx] : "Connect"}
            </button>

            <a
              href={PAT_GENERATE_URL}
              target="_blank"
              rel="noopener noreferrer"
              className="block w-full h-9 leading-9 text-center rounded-md border border-input text-sm font-medium hover:bg-muted transition-colors"
            >
              Generate PAT on GitHub ↗
            </a>

            <button
              type="button"
              onClick={handleBack}
              disabled={submitting}
              className="w-full h-9 text-xs text-text-muted hover:text-text-secondary disabled:opacity-50 transition-colors"
            >
              ← Back to providers
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
