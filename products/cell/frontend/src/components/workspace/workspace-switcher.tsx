import { useState } from "react";
import { Check, ChevronsUpDown, Cloud, GitBranch, Plus, Trash2 } from "lucide-react";
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
import type { WorkspaceSummary } from "@/lib/types";
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
  const headCommit = useConnectionStore((s) => s.headCommit);

  const [createOpen, setCreateOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);

  const active = workspaces.find((w) => w.slug === activeSlug);
  const label = active?.workspace_name || active?.slug || "No workspace";

  return (
    <>
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            data-testid="workspace-switcher-trigger"
            className="gap-1.5 text-foreground hover:bg-surface-hover max-w-[240px]"
          >
            {active && <ProviderIcon provider={active.provider} />}
            <span className="truncate text-sm font-medium">{label}</span>
            {headCommit && (
              <span
                className="text-[10px] text-text-muted font-mono shrink-0"
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
              onSelect={() => {
                setActive(ws.slug);
                setMenuOpen(false);
              }}
              onRemove={async () => {
                const ok = await remove(ws.slug);
                if (ok) {
                  toast.success(`Removed workspace ${ws.workspace_name}`);
                } else {
                  const s = useWorkspaceStore.getState();
                  toast.error(s.error ?? "Failed to remove workspace");
                }
              }}
            />
          ))}

          <DropdownMenuSeparator />
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
          <CreateWorkspaceForm
            onCreated={(ws) => {
              setCreateOpen(false);
              toast.success(`Created workspace ${ws.workspace_name}`);
            }}
            onCancel={() => setCreateOpen(false)}
          />
        </DialogContent>
      </Dialog>
    </>
  );
}

interface WorkspaceRowProps {
  ws: WorkspaceSummary;
  active: boolean;
  onSelect: () => void;
  onRemove: () => void | Promise<void>;
}

function WorkspaceRow({ ws, active, onSelect, onRemove }: WorkspaceRowProps) {
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
            Delete workspace{" "}
            <span className="font-semibold">{ws.workspace_name}</span>?
          </p>
          <p className="text-[11px] text-text-muted">
            This stops any running agents and unregisters the workspace from
            the runtime. Files on disk are not removed.
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
              Delete
            </Button>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}
