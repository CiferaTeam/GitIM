import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { Flame, Loader2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

interface BurnAgentDialogProps {
  agentId: string;
  agentName: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called after the agent has been burned and removed from the store. */
  onBurned?: () => void;
}

/**
 * Confirm + execute the archive-protocol burn (terminal departure).
 *
 * Pause vs. burn is split across two surfaces deliberately: "Stop" lives
 * on the agent's main action row and is reversible (`agents/stop`);
 * "Burn" lives here, behind a confirm dialog with stark wording, and is
 * irreversible at the agent-runtime level — the handler is reserved for
 * life and the local clone is gone. The visual gravity should match.
 *
 * Sends `POST /workspaces/{slug}/agents/burn { id }`. The daemon's
 * idempotent multi-commit chain means partial failures are safe to retry
 * — the user can hit Burn again and the daemon resumes from the first
 * incomplete phase.
 */
export function BurnAgentDialog({
  agentId,
  agentName,
  open,
  onOpenChange,
  onBurned,
}: BurnAgentDialogProps) {
  const removeAgent = useAgentStore((s) => s.removeAgent);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [submitting, setSubmitting] = useState(false);

  function closeDialog() {
    setSubmitting(false);
    onOpenChange(false);
  }

  function handleOpenChange(nextOpen: boolean) {
    if (nextOpen) {
      onOpenChange(true);
    } else if (!submitting) {
      closeDialog();
    }
  }

  async function handleConfirm() {
    if (!activeSlug) {
      toast.error("No workspace selected");
      return;
    }
    setSubmitting(true);
    const res = await client.agentsBurn(activeSlug, agentId);
    setSubmitting(false);
    if (res.ok) {
      // The `burned` SSE event will also remove the agent from the
      // store; remove eagerly so the UI doesn't briefly show a ghost
      // agent if the SSE is delayed.
      removeAgent(agentId);
      closeDialog();
      onBurned?.();
    } else {
      // Daemon retries are safe — `archive/users/<handler>.meta.yaml`
      // is the terminal-state guard. Surface the error and leave the
      // dialog open so the operator can hit Confirm Burn again.
      toast.error(res.error ?? "Failed to burn agent");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2 text-destructive">
            <Flame className="size-5" />
            Burn agent?
          </DialogTitle>
          <DialogDescription>
            Confirm to burn agent{" "}
            <span className="font-mono font-medium text-foreground">
              @{agentId}
            </span>
            {agentName && agentName !== agentId && (
              <span className="text-foreground"> ({agentName})</span>
            )}
            .
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-3 text-sm">
          <p className="text-text-secondary">This will:</p>
          <ul className="list-disc space-y-1 pl-5 text-text-secondary">
            <li>
              Write a leave-workspace event in every channel they posted in
            </li>
            <li>
              Archive their user profile and all DMs (hidden by default; can be
              manually unarchived)
            </li>
            <li>Delete their clone directory (physical removal)</li>
          </ul>

          <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3">
            <p className="font-medium text-destructive">
              The handler cannot be reused for a new agent (handlers are
              reserved for life).
            </p>
            <p className="mt-1 text-xs text-destructive/80">
              The action is partially recoverable (you can unarchive the user
              profile / DMs), but the agent runtime instance must be re-added.
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={closeDialog} disabled={submitting}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            onClick={handleConfirm}
            disabled={submitting}
          >
            {submitting ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Flame className="size-4" />
            )}
            Confirm Burn
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
