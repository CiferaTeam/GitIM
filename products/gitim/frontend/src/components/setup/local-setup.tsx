import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import { activateBrowserWorkspace } from "../../lib/client";
import {
  listBrowserWorkspaces,
  loadSessionToken,
  type BrowserWorkspaceRecord,
} from "../../lib/browser-workspaces";
import { BrowserWorkspaceForm } from "./browser-workspace-form";
import { SetupShell } from "./setup-shell";

type SetupView =
  | { kind: "list" }
  | { kind: "new" }
  | { kind: "reconnect"; record: BrowserWorkspaceRecord };

export function LocalSetup() {
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setLocalReady = useConnectionStore((s) => s.setLocalReady);
  const setError = useConnectionStore((s) => s.setError);
  const error = useConnectionStore((s) => s.error);
  const cloneProgress = useConnectionStore((s) => s.cloneProgress);
  const setCloneProgress = useConnectionStore((s) => s.setCloneProgress);
  const setMode = useConnectionStore((s) => s.setMode);
  const fetchWorkspaces = useWorkspaceStore((s) => s.fetchAll);
  const setActive = useWorkspaceStore((s) => s.setActive);

  const [workspaces, setWorkspaces] = useState(() => listBrowserWorkspaces());
  const [view, setView] = useState<SetupView>({ kind: "list" });
  const [loading, setLoading] = useState(false);
  const autoOpenAttempted = useRef(false);

  async function openWorkspace(
    record: BrowserWorkspaceRecord,
    token?: string,
  ): Promise<boolean> {
    const sessionToken = token ?? loadSessionToken(record.id) ?? null;
    if (!sessionToken) {
      setView({ kind: "reconnect", record });
      return false;
    }

    setLoading(true);
    setError(null);
    setCloneProgress("Opening browser workspace...");

    try {
      const activation = await activateBrowserWorkspace(record.slug, {
        token: sessionToken,
        onSyncReset: () => {
          void fetchWorkspaces();
        },
      });
      if (activation.error_code === "activation_superseded") return false;
      if (!activation.ok) {
        setError(activation.error ?? "Failed to activate browser workspace");
        return false;
      }

      setWorkspaces(listBrowserWorkspaces());
      await fetchWorkspaces();
      setActive(record.slug);
      setLocalReady(true);
      setStatus("ready");
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setCloneProgress(null);
      setLoading(false);
    }
  }

  useEffect(() => {
    if (autoOpenAttempted.current) return;
    const firstReady = workspaces.find((record) => loadSessionToken(record.id));
    if (!firstReady) return;

    autoOpenAttempted.current = true;
    void openWorkspace(firstReady);
  }, [workspaces]);

  async function handleConnected(record: BrowserWorkspaceRecord, token: string) {
    if (await openWorkspace(record, token)) {
      setWorkspaces(listBrowserWorkspaces());
    }
  }

  const formInitial = view.kind === "reconnect" ? view.record : undefined;
  const showForm = view.kind === "new" || view.kind === "reconnect" || workspaces.length === 0;

  return (
    <SetupShell
      step={2}
      title="Browser Mode"
      description="Clone a GitIM repository directly in this browser"
      error={error}
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
      <div className="space-y-5">
        {showForm ? (
          <BrowserWorkspaceForm
            initial={formInitial}
            submitLabel={formInitial ? "Reconnect" : "Connect"}
            onConnected={handleConnected}
            onCancel={workspaces.length > 0 ? () => setView({ kind: "list" }) : undefined}
          />
        ) : (
          <>
            <div className="space-y-3">
              {workspaces.map((record) => {
                const hasToken = loadSessionToken(record.id) !== undefined;
                return (
                  <div
                    key={record.id}
                    className="flex items-center justify-between gap-3 rounded-lg border border-border bg-background p-3"
                  >
                    <div className="min-w-0 space-y-1">
                      <p className="truncate text-sm font-medium text-foreground">
                        {record.workspace_name}
                      </p>
                      <p className="truncate text-xs text-text-muted">{record.remoteUrl}</p>
                    </div>
                    <Button
                      type="button"
                      size="sm"
                      variant={hasToken ? "default" : "outline"}
                      disabled={loading}
                      onClick={() => {
                        if (hasToken) {
                          void openWorkspace(record);
                        } else {
                          setView({ kind: "reconnect", record });
                        }
                      }}
                    >
                      {hasToken ? "Open" : "Reconnect"}
                    </Button>
                  </div>
                );
              })}
            </div>

            <Button
              type="button"
              variant="outline"
              className="w-full"
              disabled={loading}
              onClick={() => setView({ kind: "new" })}
            >
              New browser workspace
            </Button>
          </>
        )}

        {cloneProgress && (
          <p className="text-sm text-text-muted animate-pulse">{cloneProgress}</p>
        )}
      </div>
    </SetupShell>
  );
}
