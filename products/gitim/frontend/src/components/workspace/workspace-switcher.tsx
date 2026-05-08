import { useState } from "react";
import {
  Check,
  ChevronsUpDown,
  Cloud,
  GitBranch,
  Plus,
  RefreshCcw,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import {
  activateBrowserWorkspace,
  forgetBrowserWorkspaceAndCache,
  resetBrowserWorkspaceCache,
  startOverBrowserWorkspaces,
} from "@/lib/client";
import { getBrowserWorkspace, type BrowserWorkspaceRecord } from "@/lib/browser-workspaces";
import type { WorkspaceSummary } from "@/lib/types";
import { BrowserWorkspaceForm } from "@/components/setup/browser-workspace-form";
import { CreateWorkspaceForm } from "./create-workspace-form";

function ProviderIcon({ provider }: { provider: "local" | "github" }) {
  return provider === "github" ? (
    <Cloud className="size-3.5 text-text-muted" />
  ) : (
    <GitBranch className="size-3.5 text-text-muted" />
  );
}

export function WorkspaceSwitcher() {
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const setActive = useWorkspaceStore((s) => s.setActive);
  const remove = useWorkspaceStore((s) => s.remove);
  const fetchAll = useWorkspaceStore((s) => s.fetchAll);
  const mode = useConnectionStore((s) => s.mode);
  const headCommit = useConnectionStore((s) => s.headCommit);
  const setLocalReady = useConnectionStore((s) => s.setLocalReady);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setHeadCommit = useConnectionStore((s) => s.setHeadCommit);

  const [createOpen, setCreateOpen] = useState(false);
  const [reconnectRecord, setReconnectRecord] = useState<BrowserWorkspaceRecord | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);

  const active = workspaces.find((w) => w.slug === activeSlug);
  const label = active?.workspace_name || active?.slug || "No workspace";
  const isLocalMode = mode === "local";

  async function handleBrowserConnected(
    record: BrowserWorkspaceRecord,
    token: string,
  ): Promise<boolean> {
    const activation = await activateBrowserWorkspace(record.slug, {
      token,
      onSyncReset: () => {
        void fetchAll();
      },
    });
    if (!activation.ok) {
      toast.error(activation.error ?? "Failed to open browser workspace");
      return false;
    }

    await fetchAll();
    setActive(record.slug);
    setLocalReady(true);
    setStatus("ready");
    setCreateOpen(false);
    setReconnectRecord(null);
    return true;
  }

  function demoteLocalBrowserConnection() {
    setLocalReady(false);
    setStatus("disconnected");
    setHeadCommit(null);
  }

  async function handleResetCache(ws: WorkspaceSummary) {
    const confirmed = window.confirm(
      `Reset cache for ${ws.workspace_name}? This clears this workspace's IndexedDB git cache and keeps the browser workspace entry and session token.`,
    );
    if (!confirmed) return;

    const res = await resetBrowserWorkspaceCache(ws.slug);
    if (!res.ok) {
      toast.error(res.error ?? "Failed to reset browser workspace cache");
      return;
    }
    if (res.data?.activeAffected) {
      demoteLocalBrowserConnection();
    }
    await fetchAll();
    toast.success(`Reset cache for ${ws.workspace_name}`);
  }

  async function handleForgetBrowserWorkspace(ws: WorkspaceSummary) {
    const res = await forgetBrowserWorkspaceAndCache(ws.slug);
    if (!res.ok) {
      toast.error(res.error ?? "Failed to forget browser workspace");
      return;
    }
    if (res.data?.activeAffected) {
      demoteLocalBrowserConnection();
    }
    await fetchAll();
    toast.success(`Forgot workspace ${ws.workspace_name}`);
  }

  async function handleStartOver() {
    const confirmed = window.confirm(
      "Start over browser workspaces? This clears all browser workspace entries, session tokens, and IndexedDB git caches for this origin.",
    );
    if (!confirmed) return;

    const res = await startOverBrowserWorkspaces();
    if (!res.ok) {
      toast.error(res.error ?? "Failed to start over browser workspaces");
      return;
    }
    if (res.data?.activeAffected) {
      demoteLocalBrowserConnection();
    }
    await fetchAll();
    toast.success("Cleared browser workspaces");
    setMenuOpen(false);
  }

  return (
    <>
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            data-testid="workspace-switcher-trigger"
            className="w-full min-w-0 gap-1.5 text-foreground hover:bg-surface-hover md:w-auto md:max-w-[240px]"
          >
            {active && <ProviderIcon provider={active.provider} />}
            <span className="truncate text-sm font-medium">{label}</span>
            {headCommit && (
              <span
                className="hidden text-[10px] text-text-muted font-mono shrink-0 sm:inline"
                title={`HEAD ${headCommit}`}
              >
                @{headCommit.slice(0, 7)}
              </span>
            )}
            <ChevronsUpDown className="size-3.5 text-text-muted shrink-0" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-72">
          <DropdownMenuLabel className="text-text-muted text-[10px] uppercase tracking-wider">
            Workspaces
          </DropdownMenuLabel>
          <DropdownMenuSeparator />

          {workspaces.length === 0 && (
            <div className="px-2 py-3 text-xs text-text-muted">
              No workspaces yet.
            </div>
          )}

          {workspaces.map((ws) => (
            <WorkspaceRow
              key={ws.slug}
              ws={ws}
              active={ws.slug === activeSlug}
              localMode={isLocalMode}
              onSelect={() => {
                setActive(ws.slug);
                setMenuOpen(false);
              }}
              onReconnect={() => {
                const record = getBrowserWorkspace(ws.slug);
                if (!record) {
                  toast.error("Browser workspace not found");
                  return;
                }
                setMenuOpen(false);
                setReconnectRecord(record);
              }}
              onResetCache={() => {
                setMenuOpen(false);
                void handleResetCache(ws);
              }}
              onRemove={async () => {
                if (isLocalMode) {
                  await handleForgetBrowserWorkspace(ws);
                } else {
                  const ok = await remove(ws.slug);
                  if (ok) {
                    toast.success(`Removed workspace ${ws.workspace_name}`);
                  } else {
                    const s = useWorkspaceStore.getState();
                    toast.error(s.error ?? "Failed to remove workspace");
                  }
                }
              }}
            />
          ))}

          <DropdownMenuSeparator />
          {isLocalMode && (
            <>
              <DropdownMenuItem
                data-testid="workspace-switcher-start-over"
                onSelect={(e) => {
                  e.preventDefault();
                  void handleStartOver();
                }}
                className="gap-2 cursor-pointer text-destructive focus:text-destructive"
              >
                <RotateCcw className="size-3.5" />
                <span>Start over</span>
              </DropdownMenuItem>
              <DropdownMenuSeparator />
            </>
          )}
          <DropdownMenuItem
            data-testid="workspace-switcher-new"
            onSelect={(e) => {
              e.preventDefault();
              setMenuOpen(false);
              setCreateOpen(true);
            }}
            className="gap-2 cursor-pointer"
          >
            <Plus className="size-3.5" />
            <span>New workspace</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New workspace</DialogTitle>
          </DialogHeader>
          {isLocalMode ? (
            <BrowserWorkspaceForm
              onConnected={async (record, token) => {
                const connected = await handleBrowserConnected(record, token);
                if (connected) toast.success(`Created workspace ${record.workspace_name}`);
                return connected;
              }}
              onCancel={() => setCreateOpen(false)}
            />
          ) : (
            <CreateWorkspaceForm
              onCreated={(ws) => {
                setCreateOpen(false);
                toast.success(`Created workspace ${ws.workspace_name}`);
              }}
              onCancel={() => setCreateOpen(false)}
            />
          )}
        </DialogContent>
      </Dialog>

      <Dialog
        open={reconnectRecord !== null}
        onOpenChange={(open) => {
          if (!open) setReconnectRecord(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Reconnect workspace</DialogTitle>
          </DialogHeader>
          {reconnectRecord && (
            <BrowserWorkspaceForm
              initial={reconnectRecord}
              submitLabel="Reconnect"
              onConnected={async (record, token) => {
                const connected = await handleBrowserConnected(record, token);
                if (connected) toast.success(`Reconnected workspace ${record.workspace_name}`);
                return connected;
              }}
              onCancel={() => setReconnectRecord(null)}
            />
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

interface WorkspaceRowProps {
  ws: WorkspaceSummary;
  active: boolean;
  localMode: boolean;
  onSelect: () => void;
  onReconnect: () => void;
  onResetCache: () => void;
  onRemove: () => void | Promise<void>;
}

function WorkspaceRow({
  ws,
  active,
  localMode,
  onSelect,
  onReconnect,
  onResetCache,
  onRemove,
}: WorkspaceRowProps) {
  const [confirmOpen, setConfirmOpen] = useState(false);

  return (
    <div className="group relative flex items-center">
      <button
        onClick={onSelect}
        data-testid={`workspace-row-${ws.slug}`}
        className={[
          "flex-1 flex items-center gap-2 px-2 py-1.5 rounded-sm text-left text-sm hover:bg-accent hover:text-accent-foreground transition-colors min-w-0",
          active ? "font-semibold" : "",
        ].join(" ")}
      >
        <ProviderIcon provider={ws.provider} />
        <div className="flex-1 min-w-0">
          <div className="truncate">{ws.workspace_name}</div>
          <div className="text-[10px] text-text-muted font-mono truncate">
            {ws.path}
          </div>
        </div>
        {active && <Check className="size-3.5 text-primary shrink-0" />}
      </button>

      {localMode && (
        <>
          <button
            type="button"
            data-testid={`workspace-reconnect-${ws.slug}`}
            onClick={(e) => {
              e.stopPropagation();
              onReconnect();
            }}
            aria-label={`Reconnect workspace ${ws.workspace_name}`}
            className="p-1.5 rounded-sm text-text-muted hover:text-foreground opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
          >
            <RefreshCcw className="size-3.5" />
          </button>
          <button
            type="button"
            data-testid={`workspace-reset-cache-${ws.slug}`}
            onClick={(e) => {
              e.stopPropagation();
              onResetCache();
            }}
            aria-label={`Reset cache for ${ws.workspace_name}`}
            className="p-1.5 rounded-sm text-text-muted hover:text-destructive opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
          >
            <RotateCcw className="size-3.5" />
          </button>
        </>
      )}

      <Popover open={confirmOpen} onOpenChange={setConfirmOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            data-testid={`workspace-delete-${ws.slug}`}
            onClick={(e) => {
              e.stopPropagation();
            }}
            aria-label={`Delete workspace ${ws.workspace_name}`}
            className="p-1.5 rounded-sm text-text-muted hover:text-destructive opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
          >
            <Trash2 className="size-3.5" />
          </button>
        </PopoverTrigger>
        <PopoverContent side="right" align="start" className="w-64 p-3 space-y-2">
          <p className="text-xs">
            {localMode ? "Forget workspace " : "Delete workspace "}
            <span className="font-semibold">{ws.workspace_name}</span>?
          </p>
          <p className="text-[11px] text-text-muted">
            {localMode
              ? "This removes the browser workspace entry, clears its session token, and clears its local browser cache."
              : "This stops any running agents and unregisters the workspace from the runtime. Files on disk are not removed."}
          </p>
          <div className="flex justify-end gap-2 pt-1">
            <Button
              type="button"
              variant="outline"
              size="xs"
              onClick={() => setConfirmOpen(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              size="xs"
              data-testid={`workspace-delete-confirm-${ws.slug}`}
              onClick={async () => {
                setConfirmOpen(false);
                await onRemove();
              }}
            >
              {localMode ? "Forget" : "Delete"}
            </Button>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}
