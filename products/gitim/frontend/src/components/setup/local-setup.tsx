import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshCcw, RotateCcw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import {
  activateBrowserWorkspace,
  forgetBrowserWorkspaceAndCache,
  resetBrowserWorkspaceCache,
  startOverBrowserWorkspaces,
} from "../../lib/client";
import {
  listBrowserWorkspaces,
  loadSessionToken,
  type BrowserWorkspaceRecord,
} from "../../lib/browser-workspaces";
import { activeWorkspaceStorageKey } from "../../lib/workspace-key";
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
  const [cacheAction, setCacheAction] = useState<string | null>(null);
  const autoOpenAttempted = useRef(false);

  const refreshWorkspaceList = useCallback(() => {
    setWorkspaces(listBrowserWorkspaces());
  }, []);

  const openWorkspace = useCallback(async function openWorkspace(
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

      refreshWorkspaceList();
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
  }, [
    fetchWorkspaces,
    refreshWorkspaceList,
    setActive,
    setCloneProgress,
    setError,
    setLocalReady,
    setStatus,
  ]);

  useEffect(() => {
    if (autoOpenAttempted.current) return;
    const storedActiveSlug = localStorage.getItem(activeWorkspaceStorageKey("local"));
    const storedReady = storedActiveSlug
      ? workspaces.find(
          (record) =>
            record.slug === storedActiveSlug && loadSessionToken(record.id),
        )
      : undefined;
    const firstReady =
      storedReady ?? workspaces.find((record) => loadSessionToken(record.id));
    if (!firstReady) return;

    autoOpenAttempted.current = true;
    void openWorkspace(firstReady);
  }, [workspaces, openWorkspace]);

  async function handleConnected(
    record: BrowserWorkspaceRecord,
    token: string,
  ): Promise<boolean> {
    if (await openWorkspace(record, token)) {
      refreshWorkspaceList();
      return true;
    }
    return false;
  }

  function demoteLocalBrowserConnection() {
    setLocalReady(false);
    setStatus("disconnected");
  }

  async function handleResetCache(record: BrowserWorkspaceRecord) {
    const confirmed = window.confirm(
      `Reset cache for ${record.workspace_name}? This clears this workspace's IndexedDB git cache and keeps the workspace entry and session token.`,
    );
    if (!confirmed) return;

    setCacheAction(record.id);
    setError(null);
    setCloneProgress("Resetting browser workspace cache...");
    try {
      const res = await resetBrowserWorkspaceCache(record.slug);
      if (!res.ok) {
        setError(res.error ?? "Failed to reset browser workspace cache");
        return;
      }
      if (res.data?.activeAffected) demoteLocalBrowserConnection();
      await fetchWorkspaces();
      refreshWorkspaceList();
    } finally {
      setCloneProgress(null);
      setCacheAction(null);
    }
  }

  async function handleForget(record: BrowserWorkspaceRecord) {
    const confirmed = window.confirm(
      `Forget ${record.workspace_name}? This clears the workspace entry, session token, and IndexedDB git cache for this browser workspace.`,
    );
    if (!confirmed) return;

    setCacheAction(record.id);
    setError(null);
    setCloneProgress("Forgetting browser workspace...");
    try {
      const res = await forgetBrowserWorkspaceAndCache(record.slug);
      if (!res.ok) {
        setError(res.error ?? "Failed to forget browser workspace");
        return;
      }
      if (res.data?.activeAffected) demoteLocalBrowserConnection();
      await fetchWorkspaces();
      refreshWorkspaceList();
      if (view.kind === "reconnect" && view.record.id === record.id) {
        setView({ kind: "list" });
      }
    } finally {
      setCloneProgress(null);
      setCacheAction(null);
    }
  }

  async function handleStartOver() {
    const confirmed = window.confirm(
      "Start over browser workspaces? This clears all browser workspace entries, session tokens, and IndexedDB git caches for this origin.",
    );
    if (!confirmed) return;

    setCacheAction("all");
    setError(null);
    setCloneProgress("Clearing browser workspaces...");
    try {
      const res = await startOverBrowserWorkspaces();
      if (!res.ok) {
        setError(res.error ?? "Failed to start over browser workspaces");
        return;
      }
      if (res.data?.activeAffected) demoteLocalBrowserConnection();
      await fetchWorkspaces();
      refreshWorkspaceList();
      setView({ kind: "new" });
    } finally {
      setCloneProgress(null);
      setCacheAction(null);
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
                const rowBusy = loading || cacheAction !== null;
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
                    <div className="flex shrink-0 items-center gap-1">
                      <Button
                        type="button"
                        size="icon-sm"
                        variant="ghost"
                        disabled={rowBusy}
                        title={`Reset cache for ${record.workspace_name}`}
                        aria-label={`Reset cache for ${record.workspace_name}`}
                        onClick={() => void handleResetCache(record)}
                      >
                        <RefreshCcw className="size-3.5" />
                      </Button>
                      <Button
                        type="button"
                        size="icon-sm"
                        variant="ghost"
                        disabled={rowBusy}
                        title={`Forget ${record.workspace_name}`}
                        aria-label={`Forget ${record.workspace_name}`}
                        onClick={() => void handleForget(record)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                      <Button
                        type="button"
                        size="sm"
                        variant={hasToken ? "default" : "outline"}
                        disabled={rowBusy}
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
                  </div>
                );
              })}
            </div>

            <div className="flex gap-2">
              <Button
                type="button"
                variant="outline"
                className="flex-1"
                disabled={loading || cacheAction !== null}
                onClick={() => setView({ kind: "new" })}
              >
                New browser workspace
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                disabled={loading || cacheAction !== null}
                title="Start over"
                aria-label="Start over"
                onClick={() => void handleStartOver()}
              >
                <RotateCcw className="size-4" />
              </Button>
            </div>
          </>
        )}

        {cloneProgress && (
          <p className="text-sm text-text-muted animate-pulse">{cloneProgress}</p>
        )}
      </div>
    </SetupShell>
  );
}
