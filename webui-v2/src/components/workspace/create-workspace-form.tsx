import { useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type {
  CreateWorkspaceRequest,
  WorkspaceProvider,
  WorkspaceSummary,
} from "@/lib/types";

const PAT_GENERATE_URL =
  "https://github.com/settings/personal-access-tokens/new?name=GitIM%20runtime";

// Keep in sync with runtime /workspaces error_code enum.
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
  workspace_path_exists:
    "A workspace at this path is already registered. Pick a different path, or switch to it from the workspace list.",
  config_write_failed:
    "Could not write workspace config. Check permissions on the workspace folder.",
  onboard_failed:
    "Repository cloned, but identity setup failed. Try again, or rotate your PAT if the issue persists.",
};

export interface CreateWorkspaceFormProps {
  /** Called after a workspace is successfully created. */
  onCreated?: (ws: WorkspaceSummary) => void;
  /** Cancel button handler; omit to hide the cancel button. */
  onCancel?: () => void;
  /** Optional initial values (used for prefilling in the setup flow). */
  initial?: {
    path?: string;
    workspace_name?: string;
    provider?: WorkspaceProvider;
  };
  /** If true, submit button shows a full-width style suited for setup screens. */
  fullWidth?: boolean;
}

export function CreateWorkspaceForm({
  onCreated,
  onCancel,
  initial,
  fullWidth = false,
}: CreateWorkspaceFormProps) {
  const create = useWorkspaceStore((s) => s.create);
  const clearError = useWorkspaceStore((s) => s.clearError);

  const [path, setPath] = useState(initial?.path ?? "");
  const [name, setName] = useState(initial?.workspace_name ?? "");
  const [provider, setProvider] = useState<WorkspaceProvider>(
    initial?.provider ?? "local",
  );
  const [remoteUrl, setRemoteUrl] = useState("");
  const [token, setToken] = useState("");
  const [acknowledged, setAcknowledged] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  function mapError(code: string | null, fallback: string | null): string | null {
    if (!code) return fallback;
    return ERROR_MESSAGES[code] ?? fallback;
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setLocalError(null);
    clearError();

    const trimmedPath = path.trim();
    if (!trimmedPath) {
      setLocalError("Please enter a workspace path.");
      return;
    }

    let git: CreateWorkspaceRequest["git"];
    if (provider === "github") {
      const trimmedUrl = remoteUrl.trim();
      const trimmedToken = token.trim();
      if (!trimmedUrl || !trimmedToken) {
        setLocalError("Please fill both the remote URL and the PAT.");
        return;
      }
      if (!trimmedUrl.startsWith("https://github.com/")) {
        setLocalError("Remote URL must start with https://github.com/");
        return;
      }
      if (!acknowledged) {
        setLocalError("Please acknowledge the PAT ownership note.");
        return;
      }
      git = { provider: "github", remote_url: trimmedUrl, token: trimmedToken };
    } else {
      git = { provider: "local" };
    }

    const req: CreateWorkspaceRequest = {
      path: trimmedPath,
      git,
    };
    if (name.trim()) req.workspace_name = name.trim();

    setSubmitting(true);
    const created = await create(req);
    setSubmitting(false);

    if (!created) {
      // Store error was populated; useEffect above will sync it.
      const s = useWorkspaceStore.getState();
      setLocalError(mapError(s.errorCode, s.error));
      return;
    }

    onCreated?.(created);
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      <div className="space-y-1.5">
        <label
          htmlFor="ws-path"
          className="text-xs font-medium text-text-secondary"
        >
          Workspace path
        </label>
        <Input
          id="ws-path"
          data-testid="ws-path"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="/path/to/workspace"
          className="font-mono text-sm"
          disabled={submitting}
          autoFocus
        />
      </div>

      <div className="space-y-1.5">
        <label
          htmlFor="ws-name"
          className="text-xs font-medium text-text-secondary"
        >
          Display name <span className="text-text-muted font-normal">(optional)</span>
        </label>
        <Input
          id="ws-name"
          data-testid="ws-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Team Alpha"
          className="text-sm"
          disabled={submitting}
        />
      </div>

      <div className="space-y-1.5">
        <p className="text-xs font-medium text-text-secondary">Git provider</p>
        <div className="flex gap-3">
          <ProviderRadio
            value="local"
            current={provider}
            onChange={setProvider}
            label="Local"
            hint="Create a bare repo in the workspace"
            disabled={submitting}
          />
          <ProviderRadio
            value="github"
            current={provider}
            onChange={setProvider}
            label="GitHub"
            hint="Clone an existing GitHub repo"
            disabled={submitting}
          />
        </div>
      </div>

      {provider === "github" && (
        <div className="space-y-4 rounded-md border border-border bg-surface/40 p-3">
          <div className="space-y-1.5">
            <label
              htmlFor="ws-remote-url"
              className="text-xs font-medium text-text-secondary"
            >
              Remote URL
            </label>
            <Input
              id="ws-remote-url"
              data-testid="ws-remote-url"
              value={remoteUrl}
              onChange={(e) => setRemoteUrl(e.target.value)}
              placeholder="https://github.com/org/repo"
              className="font-mono text-sm"
              disabled={submitting}
            />
          </div>

          <div className="space-y-1.5">
            <label
              htmlFor="ws-token"
              className="text-xs font-medium text-text-secondary"
            >
              Personal Access Token
            </label>
            <Input
              id="ws-token"
              data-testid="ws-token"
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="ghp_... or github_pat_..."
              className="font-mono text-sm"
              autoComplete="off"
              disabled={submitting}
            />
            <a
              href={PAT_GENERATE_URL}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-block text-[11px] text-primary hover:underline"
            >
              Generate PAT on GitHub
            </a>
          </div>

          <label className="flex items-start gap-2 text-[11px] text-text-secondary leading-relaxed cursor-pointer">
            <input
              data-testid="ws-github-ack"
              type="checkbox"
              checked={acknowledged}
              onChange={(e) => setAcknowledged(e.target.checked)}
              disabled={submitting}
              className="mt-0.5 accent-primary cursor-pointer"
            />
            <span>
              I understand all agents will commit as this PAT owner on GitHub
              (agent authorship preserved in commit author field but GitHub
              contribution graph attributes to the PAT owner).
            </span>
          </label>
        </div>
      )}

      {localError && (
        <p
          data-testid="ws-create-error"
          className="text-xs text-destructive"
        >
          {localError}
        </p>
      )}

      <div className={fullWidth ? "space-y-2" : "flex justify-end gap-2"}>
        {onCancel && (
          <Button
            type="button"
            variant="outline"
            onClick={onCancel}
            disabled={submitting}
            className={fullWidth ? "w-full" : ""}
          >
            Cancel
          </Button>
        )}
        <Button
          type="submit"
          data-testid="ws-create-submit"
          disabled={
            submitting ||
            !path.trim() ||
            (provider === "github" && !acknowledged)
          }
          className={fullWidth ? "w-full" : ""}
        >
          {submitting ? (
            <span className="inline-flex items-center gap-1.5">
              <Loader2 className="size-3.5 animate-spin" />
              Creating...
            </span>
          ) : (
            "Create workspace"
          )}
        </Button>
      </div>
    </form>
  );
}

interface ProviderRadioProps {
  value: WorkspaceProvider;
  current: WorkspaceProvider;
  onChange: (v: WorkspaceProvider) => void;
  label: string;
  hint: string;
  disabled?: boolean;
}

function ProviderRadio({
  value,
  current,
  onChange,
  label,
  hint,
  disabled,
}: ProviderRadioProps) {
  const active = current === value;
  return (
    <button
      type="button"
      onClick={() => onChange(value)}
      disabled={disabled}
      className={[
        "flex-1 text-left px-3 py-2 rounded-md border transition-colors",
        active
          ? "border-primary bg-primary/10 text-foreground"
          : "border-border hover:border-border-strong bg-background text-text-secondary",
        disabled ? "opacity-50 cursor-not-allowed" : "cursor-pointer",
      ].join(" ")}
    >
      <div className="text-sm font-semibold">{label}</div>
      <div className="text-[11px] text-text-muted mt-0.5">{hint}</div>
    </button>
  );
}
