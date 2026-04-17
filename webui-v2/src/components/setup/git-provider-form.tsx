import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { SetupShell } from "./setup-shell";
import { Cloud, GitBranch, Server, Clock } from "lucide-react";

const providers = [
  { id: "local", label: "Git Local", description: "Create a bare repo in the workspace", enabled: true, icon: GitBranch },
  { id: "github", label: "GitHub", description: "Clone from an existing GitHub repository", enabled: true, icon: Cloud },
  { id: "gitlab", label: "GitLab", description: "Clone from a GitLab repository", enabled: false, icon: Server },
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
    <SetupShell
      step={3}
      title="Git Provider"
      description="Choose how to initialize version control"
      error={error}
      onBack={() => setStatus("connected")}
      footer={
        workspacePath && (
          <span className="inline-block max-w-full truncate font-mono text-text-secondary">
            {workspacePath}
          </span>
        )
      }
    >
      <div className="space-y-3">
        {providers.map((p) => {
          const Icon = p.icon;
          return (
            <button
              key={p.id}
              data-testid={`git-provider-${p.id}`}
              disabled={!p.enabled || submitting}
              onClick={() => handleSelect(p.id)}
              className={[
                "w-full flex items-center gap-3 text-left px-4 py-3 rounded-xl border transition-all",
                p.enabled
                  ? "border-border bg-card hover:border-primary/50 hover:bg-primary/5 cursor-pointer group"
                  : "border-border/50 bg-card/50 opacity-60 cursor-not-allowed",
              ].join(" ")}
            >
              <div className={[
                "w-10 h-10 rounded-lg flex items-center justify-center shrink-0 transition-colors",
                p.enabled ? "bg-surface group-hover:bg-primary/10" : "bg-surface/50",
              ].join(" ")}>
                <Icon className={[
                  "size-5 transition-colors",
                  p.enabled ? "text-text-secondary group-hover:text-primary" : "text-text-muted",
                ].join(" ")} />
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center justify-between">
                  <span className="text-sm font-semibold">{p.label}</span>
                  {!p.enabled && (
                    <span className="inline-flex items-center gap-1 text-[10px] text-text-muted px-1.5 py-0.5 rounded border border-border/50 bg-background">
                      <Clock className="size-3" />
                      Soon
                    </span>
                  )}
                </div>
                <p className="text-xs text-text-muted mt-0.5">{p.description}</p>
              </div>
            </button>
          );
        })}
      </div>

      {submitting && (
        <p className="mt-4 text-xs text-text-muted text-center">
          Initializing git repository...
        </p>
      )}
    </SetupShell>
  );
}
