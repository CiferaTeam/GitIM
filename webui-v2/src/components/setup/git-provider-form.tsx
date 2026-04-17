import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

const providers = [
  { id: "local", label: "Git Local", description: "Create a bare repo in the workspace", enabled: true },
  { id: "github", label: "GitHub", description: "Clone from an existing GitHub repository", enabled: true },
] as const;

export function GitProviderForm() {
  const baseUrl = useConnectionStore((s) => s.baseUrl);
  const workspacePath = useConnectionStore((s) => s.workspacePath);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);

  const [submitting, setSubmitting] = useState(false);

  async function handleSelect(provider: string) {
    setError(null);

    if (provider === "github") {
      setStatus("github_setup");
      return;
    }

    setSubmitting(true);
    try {
      const res = await fetch(`${baseUrl()}/git/init`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ provider }),
      });
      const data = await res.json();

      if (!data.ok) {
        setError(data.error ?? "Failed to initialize git");
        return;
      }

      setStatus("ready");
    } catch {
      setError("Failed to connect to runtime");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex flex-col items-center justify-center h-screen bg-background text-foreground">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-xl font-bold tracking-tight">GitIM</h1>
          <p className="text-sm text-muted-foreground">
            Choose a git provider for this workspace
          </p>
          {workspacePath && (
            <p className="text-xs text-text-muted font-mono truncate">
              {workspacePath}
            </p>
          )}
        </div>

        <div className="space-y-3">
          {providers.map((p) => (
            <button
              key={p.id}
              data-testid={`git-provider-${p.id}`}
              disabled={!p.enabled || submitting}
              onClick={() => handleSelect(p.id)}
              className={`w-full text-left px-4 py-3 rounded-md border transition-colors ${
                p.enabled
                  ? "border-input hover:border-ring hover:bg-muted cursor-pointer"
                  : "border-input/50 opacity-50 cursor-not-allowed"
              }`}
            >
              <div className="flex items-center justify-between">
                <span className="text-sm font-medium">{p.label}</span>
                {!p.enabled && (
                  <span className="text-xs text-text-muted px-1.5 py-0.5 rounded border border-input/50">
                    Coming Soon
                  </span>
                )}
              </div>
              <p className="text-xs text-text-muted mt-1">{p.description}</p>
            </button>
          ))}
        </div>

        {error && (
          <p data-testid="git-init-error" className="text-xs text-error text-center">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
